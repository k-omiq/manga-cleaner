"""Pinned Recipe and Model Manifest Plumbing.

Maintains verified manifests, pinned model snapshots, and integrity checking
without downloading weights or incurring GPU execution during offline testing.
"""

from __future__ import annotations

import dataclasses
import hashlib
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Dict, List, Optional, Set, Union

from deploy.cloud.common.contract import (
    PROTOCOL_VERSION,
    ContractValidationError,
    ModelInfoResponse,
    RenderRecipe,
    ServiceLimits,
    provisional_fixture_limits,
)

DEFAULT_PREPROCESSING_VERSION: str = "1.0.0"

# Standard test/fixture IDs
RECIPE_TEST_SDNQ: str = "test-sdnq-v1"
MODEL_TEST_FLUX: str = "test-flux-schnell"
REVISION_TEST_FLUX: str = "0123456789abcdef0123456789abcdef01234567"

# Production FLUX SDNQ identifiers
RECIPE_PROD_SDNQ: str = "sdnq-v1"
MODEL_PROD_FLUX: str = "bghira/flux-1-schnell-sdnq-v1"
REVISION_PROD_FLUX: str = "0123456789abcdef0123456789abcdef01234567"


@dataclass(frozen=True)
class ManifestFileRecord:
    relative_path: str
    sha256: str
    size_bytes: int

    def to_dict(self) -> Dict[str, Any]:
        return dataclasses.asdict(self)


@dataclass(frozen=True)
class ModelManifest:
    model_id: str
    model_revision: str
    recipe_id: str
    preprocessing_version: str
    files: List[ManifestFileRecord]
    total_bytes: int
    native_mask_conditioning: bool = False

    def to_dict(self) -> Dict[str, Any]:
        return {
            "model_id": self.model_id,
            "model_revision": self.model_revision,
            "recipe_id": self.recipe_id,
            "preprocessing_version": self.preprocessing_version,
            "files": [f.to_dict() for f in self.files],
            "total_bytes": self.total_bytes,
            "native_mask_conditioning": self.native_mask_conditioning,
        }


# Canonical pinned test manifest
TEST_FLUX_MANIFEST = ModelManifest(
    model_id=MODEL_TEST_FLUX,
    model_revision=REVISION_TEST_FLUX,
    recipe_id=RECIPE_TEST_SDNQ,
    preprocessing_version=DEFAULT_PREPROCESSING_VERSION,
    files=[
        ManifestFileRecord(
            relative_path="flux1-schnell-sdnq.safetensors",
            sha256="0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            size_bytes=5368709120,
        ),
        ManifestFileRecord(
            relative_path="config.json",
            sha256="fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210",
            size_bytes=1024,
        ),
    ],
    total_bytes=5368710144,
    native_mask_conditioning=False,
)

# Registry of pinned recipes allowed on worker gateways
PINNED_RECIPES: Dict[str, RenderRecipe] = {
    RECIPE_TEST_SDNQ: RenderRecipe(
        recipe_id=RECIPE_TEST_SDNQ,
        preprocessing_version=DEFAULT_PREPROCESSING_VERSION,
        model_id=MODEL_TEST_FLUX,
        model_revision=REVISION_TEST_FLUX,
        native_mask_conditioning=False,
    ),
    RECIPE_PROD_SDNQ: RenderRecipe(
        recipe_id=RECIPE_PROD_SDNQ,
        preprocessing_version=DEFAULT_PREPROCESSING_VERSION,
        model_id=MODEL_PROD_FLUX,
        model_revision=REVISION_PROD_FLUX,
        native_mask_conditioning=False,
    ),
}

PINNED_MANIFESTS: Dict[str, ModelManifest] = {
    f"{MODEL_TEST_FLUX}@{REVISION_TEST_FLUX}": TEST_FLUX_MANIFEST,
}


def get_pinned_recipe(recipe_id: str) -> Optional[RenderRecipe]:
    """Lookup a pinned recipe by recipe_id."""
    return PINNED_RECIPES.get(recipe_id)


def is_recipe_supported(recipe: RenderRecipe) -> bool:
    """Verify whether a RenderRecipe exactly matches a pinned allowed recipe."""
    pinned = PINNED_RECIPES.get(recipe.recipe_id)
    if pinned is None:
        return False
    return (
        pinned.recipe_id == recipe.recipe_id
        and pinned.preprocessing_version == recipe.preprocessing_version
        and pinned.model_id == recipe.model_id
        and pinned.model_revision == recipe.model_revision
        and pinned.native_mask_conditioning == recipe.native_mask_conditioning
        and recipe.native_mask_conditioning is False
    )


def verify_model_manifest(
    model_dir: Union[str, Path],
    manifest: ModelManifest,
) -> Dict[str, Any]:
    """Verify existing local files against the declared model manifest without downloading.

    Returns a report dictionary:
    {
        "valid": bool,
        "verified_files": list[str],
        "missing_files": list[str],
        "corrupted_files": list[str],
        "total_verified_bytes": int
    }
    """
    dir_path = Path(model_dir)
    verified_files: List[str] = []
    missing_files: List[str] = []
    corrupted_files: List[str] = []
    total_bytes = 0

    if not dir_path.is_dir():
        return {
            "valid": False,
            "verified_files": [],
            "missing_files": [f.relative_path for f in manifest.files],
            "corrupted_files": [],
            "total_verified_bytes": 0,
        }

    for file_rec in manifest.files:
        target_file = dir_path / file_rec.relative_path
        if not target_file.is_file():
            missing_files.append(file_rec.relative_path)
            continue

        stat = target_file.stat()
        if stat.st_size != file_rec.size_bytes:
            corrupted_files.append(file_rec.relative_path)
            continue

        # Compute SHA-256
        hasher = hashlib.sha256()
        with open(target_file, "rb") as f:
            for chunk in iter(lambda: f.read(65536), b""):
                hasher.update(chunk)

        computed_sha = hasher.hexdigest()
        if computed_sha.lower() != file_rec.sha256.lower():
            corrupted_files.append(file_rec.relative_path)
            continue

        verified_files.append(file_rec.relative_path)
        total_bytes += stat.st_size

    is_valid = (len(missing_files) == 0) and (len(corrupted_files) == 0)
    return {
        "valid": is_valid,
        "verified_files": verified_files,
        "missing_files": missing_files,
        "corrupted_files": corrupted_files,
        "total_verified_bytes": total_bytes,
    }


def get_default_model_info(
    provider: str,
    recipe_id: str = RECIPE_TEST_SDNQ,
    limits: Optional[ServiceLimits] = None,
) -> ModelInfoResponse:
    """Construct a ModelInfoResponse bound to a pinned recipe."""
    recipe = PINNED_RECIPES.get(recipe_id)
    if recipe is None:
        recipe = PINNED_RECIPES[RECIPE_TEST_SDNQ]

    return ModelInfoResponse(
        protocol_version=PROTOCOL_VERSION,
        provider=provider,
        model_id=recipe.model_id,
        model_revision=recipe.model_revision,
        recipe_id=recipe.recipe_id,
        preprocessing_version=recipe.preprocessing_version,
        native_mask_conditioning=False,
        limits=limits or provisional_fixture_limits(),
    )
