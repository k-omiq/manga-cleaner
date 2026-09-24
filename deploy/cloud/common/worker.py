"""GPU worker runtime shared by the Modal and Beam workers.

One instance lives per GPU container. It loads the pinned pipeline from the weights
volume once and then renders jobs. Every failure comes back as a typed result instead
of an exception, so the provider records the job as finished and the gateway can show
the user a stable error code. A container whose load failed tries again on its next
job, which is how it recovers once the weights volume is seeded.
"""

from __future__ import annotations

import json
import logging
import re
import time
from typing import Any, Callable, Dict, Optional, Tuple

from deploy.cloud.common.contract import ContractValidationError, JobRequestMetadata, ServiceLimits
from deploy.cloud.common.flux import SdnqFluxRunner, WorkerRenderError, render_job
from deploy.cloud.common.jobs import WorkerOutcome, job_key, run_key
from deploy.cloud.common.manifest import production_limits
from deploy.cloud.common.redact import redact_text
from deploy.cloud.common.weights import WeightsError, require_snapshot

logger = logging.getLogger("deploy.cloud.worker")

MAX_MESSAGE_CHARS = 1024
_ERROR_CODE = re.compile(r"^[a-z][a-z0-9_]{0,63}$")


def failure(error_code: str, message: str) -> Dict[str, Any]:
    """The typed failure a worker returns; messages are redacted and bounded."""
    text = redact_text(str(message)) or error_code
    return {"ok": False, "error_code": error_code, "message": text[:MAX_MESSAGE_CHARS]}


def outcome_from_result(result: Any) -> WorkerOutcome:
    """Turn what a worker returned into a WorkerOutcome, trusting nothing about its shape."""
    if not isinstance(result, dict):
        return WorkerOutcome("failed", error_code="invalid_output", message="The GPU worker returned an unexpected value")
    if result.get("ok") is True:
        png, digest = result.get("png"), result.get("digest")
        if not isinstance(png, (bytes, bytearray)) or not isinstance(digest, str):
            return WorkerOutcome("failed", error_code="invalid_output", message="The GPU worker returned no image")
        return WorkerOutcome("completed", png=bytes(png), digest=digest)
    code = result.get("error_code")
    if not isinstance(code, str) or not _ERROR_CODE.match(code):
        code = "worker_failed"
    message = result.get("message")
    text = redact_text(message) if isinstance(message, str) and message else "The GPU worker failed"
    return WorkerOutcome("failed", error_code=code, message=text[:MAX_MESSAGE_CHARS])


def begin_run(store: Any, handle: str, clock: Callable[[], float] = time.time) -> Optional[Dict[str, Any]]:
    """Mark a job as started on the GPU, then honour a cancel that arrived first.

    Returns the typed "cancelled" failure when the job must not render, else None.
    The marker is written before the cancel flag is read: a gateway that finds no
    marker after setting the flag may report the job cancelled, because this read
    will then see the flag. Store access is best effort; a store hiccup must not fail
    a render the user is waiting for, it only delays the "running" status.
    """
    try:
        store.put(run_key(handle), {"started_at": clock()})
    except Exception as error:
        logger.error("Run marker write failed for %s: %s", handle, redact_text(str(error)))
    try:
        doc = store.get(job_key(handle))
    except Exception as error:
        logger.error("Job document read failed for %s: %s", handle, redact_text(str(error)))
        doc = None
    if doc is not None and doc.get("cancel_requested"):
        return failure("cancelled", "The job was cancelled before it started")
    return None


def parse_metadata(metadata_json: str) -> JobRequestMetadata:
    try:
        payload = json.loads(metadata_json)
    except (TypeError, ValueError) as exc:
        raise ContractValidationError(f"Job metadata is not JSON: {exc}") from exc
    return JobRequestMetadata.from_dict(payload)


class WorkerRuntime:
    def __init__(
        self,
        weights_root: str,
        reload: Optional[Callable[[], Any]] = None,
        runner_factory: Callable[[str], Any] = SdnqFluxRunner,
        limits: Optional[ServiceLimits] = None,
        marker_attempts: int = 1,
        marker_delay_seconds: float = 0.0,
        sleep: Callable[[float], Any] = time.sleep,
    ):
        self.weights_root = weights_root
        self.reload = reload
        self.runner_factory = runner_factory
        self.limits = limits or production_limits()
        self.marker_attempts = marker_attempts
        self.marker_delay_seconds = marker_delay_seconds
        self.sleep = sleep
        self.runner: Any = None
        self.load_error: Optional[Tuple[str, str]] = None

    def load(self) -> bool:
        """Load the pipeline; on failure remember the typed reason and return False."""
        if self.runner is not None:
            return True
        try:
            snapshot = require_snapshot(
                self.weights_root,
                reload=self.reload,
                attempts=self.marker_attempts,
                delay_seconds=self.marker_delay_seconds,
                sleep=self.sleep,
            )
            runner = self.runner_factory(str(snapshot))
            runner.load()
        except WeightsError as exc:
            self.load_error = (exc.error_code, str(exc))
        except WorkerRenderError as exc:
            self.load_error = (exc.error_code, str(exc))
        except Exception as exc:
            self.load_error = ("model_load_failed", f"Failed to load the model: {exc}")
        else:
            self.runner = runner
            self.load_error = None
            return True
        logger.error("Worker load failed: %s", redact_text(self.load_error[1]))
        return False

    def render(self, metadata_json: str, image_png: bytes, hint_png: bytes) -> Dict[str, Any]:
        """Render one job. Returns {"ok": True, "png", "digest"} or a typed failure."""
        try:
            meta = parse_metadata(metadata_json)
        except ContractValidationError as exc:
            return failure("invalid_request", str(exc))
        if not self.load():
            code, message = self.load_error or ("model_load_failed", "Model not loaded")
            return failure(code, message)
        try:
            rendered = render_job(self.runner, meta, bytes(image_png), bytes(hint_png), self.limits)
        except WorkerRenderError as exc:
            return failure(exc.error_code, str(exc))
        except Exception as exc:
            logger.error("Unexpected render failure: %s", redact_text(str(exc)))
            return failure("inference_failed", f"Unexpected render failure: {exc}")
        return {"ok": True, "png": rendered.png, "digest": rendered.digest}
