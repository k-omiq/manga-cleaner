"""Backend registration and factory for Manga Cleaner sidecar.

Three ids, two of which can actually render:

* ``mflux`` - MLX, Apple Silicon only.
* ``sdnq`` - torch + diffusers + sdnq, on CUDA, Intel XPU or Metal. The
  Windows and Linux answer, and a second Apple one.
* ``sdcpp`` - declared and declines every open, because ggml exposes no
  allocator setter in Python (rule 9).

**A virtual environment may hold either backend's dependencies, or both.** The
Rust side finds an install by looking for a `pyvenv.cfg` and an interpreter and
cannot tell which packages are in it, so which backends are *runnable* is a
question only this process can answer - :func:`importable_backends` is the
answer, reported on `GET /v1/health` so a chosen backend that cannot be imported
is refused before a model load is attempted rather than during one.
"""

from __future__ import annotations

from typing import Callable, Dict, List, Optional

from ..protocol import ErrorKind, SidecarError
from .base import BackendBase
from .mflux import MfluxBackend
from .sdcpp import SdcppBackend
from .sdnq import SdnqBackend

__all__ = [
    "BackendBase",
    "MfluxBackend",
    "SdcppBackend",
    "SdnqBackend",
    "get_backend",
    "importable_backends",
    "BACKENDS",
]

#: Every id `POST /v1/open` accepts, and the class behind it.
BACKENDS: Dict[str, type] = {
    "mflux": MfluxBackend,
    "sdnq": SdnqBackend,
    "sdcpp": SdcppBackend,
}

#: Whether this environment could run each backend, asked without importing the
#: heavy runtime. `sdcpp` is absent on purpose: it has no import to check and no
#: open it would accept, so listing it as importable would promise a render.
_PROBES: Dict[str, Callable[[], bool]] = {
    "mflux": lambda: __import__(
        "manga_cleaner_sidecar.backend.mflux", fromlist=["importable"]
    ).importable(),
    "sdnq": lambda: __import__(
        "manga_cleaner_sidecar.backend.sdnq", fromlist=["importable"]
    ).importable(),
}


def importable_backends() -> List[str]:
    """The backend ids whose dependencies are present, in preference order."""
    found = []
    for name, probe in _PROBES.items():
        try:
            if probe():
                found.append(name)
        except Exception:
            continue
    return found


def get_backend(name: str, weights_dir: Optional[str] = None) -> BackendBase:
    """Instantiate and return the requested backend engine."""
    name_lower = (name or "").lower().strip()
    backend = BACKENDS.get(name_lower)
    if backend is None:
        raise SidecarError(
            422,
            ErrorKind.BAD_REQUEST,
            f"Unknown backend '{name}'. Supported backends: "
            + ", ".join(f"'{k}'" for k in BACKENDS),
        )
    return backend(weights_dir=weights_dir)  # type: ignore[call-arg]
