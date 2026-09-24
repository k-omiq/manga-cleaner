"""Modal app: CPU gateway, GPU worker and weights seeding.

The provisioner exports the MC_MODAL_* variables (see settings.py), imports this
module and deploys `app`. Each image carries the same variables, so a container that
imports this module again rebuilds exactly the app that was deployed.

- `gateway` serves /mc/v1 on a small CPU container behind Modal proxy auth. It keeps
  job documents in a modal.Dict, so health and model-info never start a GPU.
- `Worker` loads the pinned pipeline from the weights volume once per GPU container
  and renders one job per call. It scales to zero after the idle window.
- `seed_weights` downloads the pinned snapshot into the weights volume on a CPU
  container. GPU containers never download.

Only this package's Python files are uploaded (include_source=False plus one
filtered add_local_dir), never anything else from the machine that deploys.
"""

from __future__ import annotations

import time
from pathlib import Path
from typing import Any, Dict

import modal

from deploy.cloud.common.deployment import (
    JOB_TIMEOUT_SECONDS,
    SEED_TIMEOUT_SECONDS,
    WORKER_STARTUP_TIMEOUT_SECONDS,
)
from deploy.cloud.common.manifest import get_production_model_info, production_limits
from deploy.cloud.common.weights import run_seed
from deploy.cloud.common.worker import WorkerRuntime
from deploy.cloud.modal.backend import ModalDictStore, ModalWorkerDispatcher, worker_render
from deploy.cloud.modal.settings import WEIGHTS_MOUNT, ModalSettings

SETTINGS = ModalSettings.from_env()

PYTHON_VERSION = "3.12"
TORCH_INDEX_URL = "https://download.pytorch.org/whl/cu129"
TORCH_REQUIREMENT = "torch==2.13.0"
GPU_REQUIREMENTS = (
    "diffusers==0.39.0",
    "sdnq==0.2.4",
    "transformers==5.15.0",
    "accelerate==1.14.0",
    "safetensors==0.8.0",
    "huggingface-hub==1.24.0",
    "pillow==12.3.0",
    "numpy==2.4.6",
)
SEED_REQUIREMENTS = ("huggingface-hub==1.24.0",)

# deploy/cloud/modal/app.py -> deploy/
DEPLOY_ROOT = Path(__file__).resolve().parents[2]
REMOTE_DEPLOY_ROOT = "/root/deploy"  # /root is on PYTHONPATH in Modal containers

_SHIPPED_DIRS = {("cloud", "common"), ("cloud", "modal")}


def _not_shipped(relative: Path) -> bool:
    """add_local_dir ignore predicate: ship the package inits plus common/ and modal/ sources."""
    parts = relative.parts
    if relative.suffix != ".py":
        return True
    if parts in {("__init__.py",), ("cloud", "__init__.py")}:
        return False
    return not (len(parts) == 3 and parts[:2] in _SHIPPED_DIRS)


def _finish(image: modal.Image, extra_env: Dict[str, str] | None = None) -> modal.Image:
    env = {**SETTINGS.to_env(), **(extra_env or {})}
    # add_local_dir must be the last step: files are attached at container start.
    return image.env(env).add_local_dir(DEPLOY_ROOT, REMOTE_DEPLOY_ROOT, ignore=_not_shipped)


gateway_image = _finish(modal.Image.debian_slim(python_version=PYTHON_VERSION))

seed_image = _finish(
    modal.Image.debian_slim(python_version=PYTHON_VERSION).pip_install(*SEED_REQUIREMENTS),
    {"HF_HUB_DISABLE_TELEMETRY": "1", "HF_HUB_DISABLE_PROGRESS_BARS": "1"},
)

gpu_image = _finish(
    modal.Image.debian_slim(python_version=PYTHON_VERSION)
    .pip_install(TORCH_REQUIREMENT, index_url=TORCH_INDEX_URL)
    .pip_install(*GPU_REQUIREMENTS),
    {"HF_HUB_OFFLINE": "1", "HF_HUB_DISABLE_TELEMETRY": "1"},
)

weights_volume = modal.Volume.from_name(SETTINGS.volume_name, environment_name=SETTINGS.environment)
jobs_dict = modal.Dict.from_name(SETTINGS.dict_name, environment_name=SETTINGS.environment)

app = modal.App(SETTINGS.app_name, include_source=False)


@app.function(
    image=seed_image,
    volumes={WEIGHTS_MOUNT: weights_volume},
    cpu=2.0,
    memory=4096,
    timeout=SEED_TIMEOUT_SECONDS,
    max_containers=1,
)
def seed_weights() -> Dict[str, Any]:
    # Idempotent: a seeded volume returns at once and a partial download resumes.
    # run_seed retries internally, publishes progress to the job Dict for the
    # provisioner and returns failures as a typed result instead of raising.
    return run_seed(WEIGHTS_MOUNT, ModalDictStore(jobs_dict), commit=weights_volume.commit)


@app.cls(
    image=gpu_image,
    gpu=SETTINGS.gpu,
    volumes={WEIGHTS_MOUNT: weights_volume},
    cpu=2.0,
    memory=12288,
    timeout=JOB_TIMEOUT_SECONDS,
    startup_timeout=WORKER_STARTUP_TIMEOUT_SECONDS,
    scaledown_window=SETTINGS.idle_seconds,
    max_containers=1,
)
class Worker:
    @modal.enter()
    def load(self) -> None:
        # A missing snapshot is reported per job, not raised here: the container stays
        # up and loads on its next job once the volume is seeded.
        self.runtime = WorkerRuntime(WEIGHTS_MOUNT, reload=weights_volume.reload, marker_attempts=2)
        self.runtime.load()

    @modal.method()
    def render(self, handle: str, metadata_json: str, image_png: bytes, hint_png: bytes) -> Dict[str, Any]:
        return worker_render(self.runtime, ModalDictStore(jobs_dict), handle, metadata_json, image_png, hint_png)


@app.function(
    image=gateway_image,
    cpu=0.25,
    memory=512,
    max_containers=1,
)
@modal.concurrent(max_inputs=32)
@modal.asgi_app(requires_proxy_auth=True)
def gateway():
    from deploy.cloud.common.api import CloudGateway
    from deploy.cloud.common.jobs import DispatchJobBackend

    limits = production_limits()
    store = ModalDictStore(jobs_dict)
    dispatcher = ModalWorkerDispatcher(
        spawn=Worker().render.spawn,
        function_call_from_id=modal.FunctionCall.from_id,
        exceptions=modal.exception,
        store=store,
    )
    gateway_app = CloudGateway(
        provider="modal",
        model_info=get_production_model_info("modal"),
        limits=limits,
        backend=DispatchJobBackend("modal", store, dispatcher, limits, clock=time.time),
        trust_edge_auth=True,
    )
    return gateway_app.as_asgi_app()
