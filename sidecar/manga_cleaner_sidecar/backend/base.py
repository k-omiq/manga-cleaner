"""Abstract base class definition for sidecar inference backends."""

from __future__ import annotations

import abc
from typing import Any, Dict, List, Optional, Tuple

from ..protocol import Basis, ImageBuffer, ModelMetadata


class BackendBase(abc.ABC):
    """Abstract interface that all sidecar backends must implement."""

    @property
    @abc.abstractmethod
    def backend_id(self) -> str:
        """The identifier string of this backend (e.g. 'mflux', 'sdcpp')."""
        raise NotImplementedError

    @abc.abstractmethod
    def get_model_metadata(self) -> ModelMetadata:
        """Return the metadata for the active or configured model."""
        raise NotImplementedError

    @abc.abstractmethod
    def get_weights_bytes(self) -> Optional[int]:
        """Return the size of the model weights on disk in bytes."""
        raise NotImplementedError

    @abc.abstractmethod
    def get_working_set_bytes(self) -> Optional[int]:
        """Return the estimated or measured working set in bytes."""
        raise NotImplementedError

    @abc.abstractmethod
    def get_basis(self) -> Basis:
        """Return whether working set is declared or measured."""
        raise NotImplementedError

    @abc.abstractmethod
    def get_applied_guards(self) -> List[str]:
        """Return the list of memory guards and constraints currently in force."""
        raise NotImplementedError

    @abc.abstractmethod
    def open(
        self,
        model_id: str,
        limits: Dict[str, int],
        require: List[str],
    ) -> Dict[str, Any]:
        """Load model weights and apply memory bounding guards.

        Raises SidecarError if a required guard cannot be satisfied, weights are missing,
        or an allocation failure occurs.
        """
        raise NotImplementedError

    @abc.abstractmethod
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
        """Execute inpainting inference on the input image.

        Returns a tuple of (output_image_buffer, elapsed_milliseconds).
        """
        raise NotImplementedError

    @abc.abstractmethod
    def release(self) -> None:
        """Unload model weights, reclaim caches, and return memory to floor."""
        raise NotImplementedError
