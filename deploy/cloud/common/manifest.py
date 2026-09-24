"""Pinned Recipe and Model Manifest Plumbing.

Maintains verified manifests, pinned model snapshots, and integrity checking
without downloading weights or incurring GPU execution during offline testing.
"""

from __future__ import annotations

import dataclasses
import hashlib
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Dict, List, Optional, Set, Tuple, Union

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

# Production recipe. The local engine is the single source of truth for every value
# below: crates/cleaner-core/src/engines/flux.rs holds PROMPT, STEPS, SEED and
# GUIDANCE, and sidecar/manga_cleaner_sidecar/backend/sdnq.py holds WORK_LONG_SIDE
# and LATENT_STRIDE. deploy/cloud/tests/test_recipe_parity.py parses both files so
# a change on either side fails the suite instead of silently drifting.
RECIPE_PROD_SDNQ: str = "mc-flux2-klein-edit-v1"
MODEL_PROD_FLUX: str = "Disty0/FLUX.2-klein-4B-SDNQ-4bit-dynamic"
REVISION_PROD_FLUX: str = "45e9cc76cb70f84473ce5c6c2e2282d0ef3c6ecd"

# The prompt belongs to the recipe on the server and never travels on the wire.
RECIPE_PROMPT: str = "Remove all text."
# Crops whose long side is below this are upscaled before inference, as locally.
WORK_LONG_SIDE: int = 768


@dataclass(frozen=True)
class PinnedSampling:
    """Sampling values a recipe fixes. A request that differs is not the same render."""

    seed: int
    steps: int
    guidance_scaled: int


# Wire values: guidance is sent times 100 (GUIDANCE 1.0 -> 100) and divided back on the server.
PINNED_SAMPLING: Dict[str, PinnedSampling] = {
    RECIPE_PROD_SDNQ: PinnedSampling(seed=1, steps=4, guidance_scaled=100),
}

# Files of the pinned snapshot, exactly as the Hub lists them at REVISION_PROD_FLUX.
# The Hub does not publish per-file SHA-256 for the small files, so the seed job checks
# the file set and every size; the commit hash pins the content itself.
PROD_SNAPSHOT_FILES: Tuple[Tuple[str, int], ...] = (
    (".gitattributes", 1580),
    ("README.md", 2338),
    ("model_index.json", 498),
    ("scheduler/scheduler_config.json", 486),
    ("text_encoder/config.json", 15367),
    ("text_encoder/generation_config.json", 218),
    ("text_encoder/model.safetensors", 2824093272),
    ("text_encoder/quantization_config.json", 13146),
    ("tokenizer/added_tokens.json", 707),
    ("tokenizer/chat_template.jinja", 4168),
    ("tokenizer/merges.txt", 1671853),
    ("tokenizer/special_tokens_map.json", 613),
    ("tokenizer/tokenizer.json", 11422654),
    ("tokenizer/tokenizer_config.json", 5404),
    ("tokenizer/vocab.json", 2776833),
    ("transformer/config.json", 7328),
    ("transformer/diffusion_pytorch_model.safetensors", 2467785416),
    ("transformer/quantization_config.json", 6496),
    ("vae/config.json", 925),
    ("vae/diffusion_pytorch_model.safetensors", 168120878),
)
PROD_SNAPSHOT_TOTAL_BYTES: int = 5475930180


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


def get_pinned_sampling(recipe_id: str) -> Optional[PinnedSampling]:
    """Sampling values a recipe fixes, or None when the recipe leaves them to the request."""
    return PINNED_SAMPLING.get(recipe_id)


def production_limits() -> ServiceLimits:
    """Limits a deployed gateway advertises.

    Same payload bounds as the fixture limits; the worker deadline matches the provider
    job timeout (600 s) so the client does not give up on a job the worker still runs.
    """
    return dataclasses.replace(provisional_fixture_limits(), worker_timeout_seconds=600)


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


def get_production_model_info(provider: str) -> ModelInfoResponse:
    """Model info a deployed gateway serves on GET /model-info."""
    return get_default_model_info(provider, RECIPE_PROD_SDNQ, production_limits())
