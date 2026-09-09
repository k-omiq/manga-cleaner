"""stable-diffusion.cpp / ggml backend stub.

As specified by rule 9 and cleaner-core::sidecar::backend, the ggml allocator
exposes no programmatic memory or cache setters in Python. The backend declines
open requests with 501 unbounded_backend rather than presenting illusory memory bounds.
"""

from __future__ import annotations

from typing import Any, Dict, List, Optional, Tuple

from ..protocol import Basis, ErrorKind, ImageBuffer, ModelMetadata, SidecarError
from .base import BackendBase


class SdcppBackend(BackendBase):
    """Backend for stable-diffusion.cpp (GGUF via ggml)."""

    def __init__(self, weights_dir: Optional[str] = None) -> None:
        self.weights_dir = weights_dir

    @property
    def backend_id(self) -> str:
        return "sdcpp"

    def get_model_metadata(self) -> ModelMetadata:
        return ModelMetadata(
            id="flux2-klein-4b-gguf",
            quantization="q4_k_m",
            source="ggml/stable-diffusion.cpp",
        )

    def get_weights_bytes(self) -> Optional[int]:
        return None

    def get_working_set_bytes(self) -> Optional[int]:
        # GGML does not declare bounded resident allocations on unified memory
        return None

    def get_basis(self) -> Basis:
        return Basis.DECLARED

    def get_applied_guards(self) -> List[str]:
        return []

    def open(
        self,
        model_id: str,
        limits: Dict[str, int],
        require: List[str],
    ) -> Dict[str, Any]:
        """Decline open request with 501 unbounded_backend."""
        raise SidecarError(
            501,
            ErrorKind.UNBOUNDED_BACKEND,
            "The stable-diffusion.cpp / ggml backend exposes no programmatic allocator "
            "or cache setter in Python, and therefore cannot bound its resident memory usage. "
            "Declined under Rule 9 memory safety contract.",
        )

    def render(
        self,
        image: ImageBuffer,
        hint: Optional[ImageBuffer],
        prompt: str,
        steps: int,
        seed: int,
        guidance: float,
        deadline_ms: int,
    ) -> Tuple[ImageBuffer, int]:
        raise SidecarError(412, ErrorKind.NOT_OPEN, "Backend sdcpp cannot be opened.")

    def release(self) -> None:
        pass
