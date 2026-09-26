"""Scale-to-zero runtime and CPU gateway view of seeded analysis graphs."""
from __future__ import annotations

from typing import Any, Callable, Iterable

from deploy.cloud.common.analysis import AnalysisWorker
from deploy.cloud.common.analysis_seed import GRAPH_FILES, installed_graphs, normalize_analysis_models


class OnDemandAnalysis:
    """Load ONNX sessions only in a GPU worker and never fall back to CPU."""

    def __init__(self, root: str, selected: Iterable[str],
                 session_factory: Callable[[str], Any] | None = None,
                 reload: Callable[[], Any] | None = None):
        self.root = root
        self.selected = normalize_analysis_models(tuple(selected))
        self.reload = reload
        self._session_factory = session_factory or self._cuda_session
        self._sessions: dict[str, Any] = {}
        self._worker: AnalysisWorker | None = None

    @staticmethod
    def _cuda_session(path: str) -> Any:
        # torch loads CUDA shared libraries bundled in the image. ONNX Runtime
        # must still report CUDA explicitly; an unavailable provider is not a
        # successful GPU deployment and must not be hidden by CPU fallback.
        import torch  # noqa: F401
        import onnxruntime as ort
        if "CUDAExecutionProvider" not in ort.get_available_providers():
            raise RuntimeError("ONNX Runtime CUDA provider is unavailable")
        session = ort.InferenceSession(path, providers=["CUDAExecutionProvider"])
        if "CUDAExecutionProvider" not in session.get_providers():
            raise RuntimeError("ONNX graph did not initialize on CUDA")
        return session

    def _session(self, path: str) -> Any:
        if path not in self._sessions:
            self._sessions[path] = self._session_factory(path)
        return self._sessions[path]

    def _load(self) -> AnalysisWorker:
        if self.reload is not None:
            self.reload()
        graphs, revisions = installed_graphs(self.root, self.selected, verify_hash=True)
        if not graphs:
            raise RuntimeError("No selected, verified analysis graphs are installed")
        verified_digests = {path: sha for capability, paths in graphs.items()
                            for path, (_, _, sha) in zip(paths, GRAPH_FILES[capability])}
        self._worker = AnalysisWorker(graphs, revisions, self._session, verified_digests)
        return self._worker

    def analyze(self, request: dict[str, Any], tile_png: bytes) -> dict[str, Any]:
        worker = self._worker or self._load()
        return worker.analyze(request, tile_png)
