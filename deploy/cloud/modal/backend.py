"""Modal pieces of the dispatch backend.

`ModalDictStore` keeps the gateway's job documents in a modal.Dict, and
`ModalWorkerDispatcher` hands jobs to the GPU worker class through `spawn` and reads
their outcome back through `FunctionCall`. Nothing here imports modal: app.py passes
the SDK objects in, so the offline tests drive this code with fakes.
"""

from __future__ import annotations

import json
import logging
import time
from typing import Any, Callable, Dict, Optional

from deploy.cloud.common.contract import JobRequestMetadata
from deploy.cloud.common.jobs import WorkerOutcome, run_key
from deploy.cloud.common.redact import redact_text
from deploy.cloud.common.worker import WorkerRuntime, begin_run, outcome_from_result

logger = logging.getLogger("deploy.cloud.modal")


class ModalDictStore:
    """KeyValueStore on a modal.Dict (values are small plain dicts)."""

    def __init__(self, modal_dict: Any):
        self._dict = modal_dict

    def get(self, key: str) -> Optional[Dict[str, Any]]:
        value = self._dict.get(key, None)
        return dict(value) if isinstance(value, dict) else None

    def put(self, key: str, value: Dict[str, Any]) -> None:
        self._dict.put(key, dict(value))

    def put_if_absent(self, key: str, value: Dict[str, Any]) -> bool:
        return bool(self._dict.put(key, dict(value), skip_if_exists=True))

    def delete(self, key: str) -> None:
        self._dict.pop(key, None)


class ModalWorkerDispatcher:
    """WorkerDispatcher over a Modal class method.

    `spawn` is `Worker().render.spawn`, `function_call_from_id` is
    `modal.FunctionCall.from_id` and `exceptions` is `modal.exception`.
    """

    def __init__(
        self,
        spawn: Callable[..., Any],
        function_call_from_id: Callable[[str], Any],
        exceptions: Any,
        store: Any,
    ):
        self._spawn = spawn
        self._from_id = function_call_from_id
        self._exc = exceptions
        self._store = store
        # Provider-side hiccups: the call's state is unknown, so the caller keeps the
        # last known status and asks again on the next poll.
        self._transient = tuple(
            getattr(exceptions, name)
            for name in ("ConnectionError", "ClientClosed", "ServiceError", "InternalError", "ResourceExhaustedError")
            if isinstance(getattr(exceptions, name, None), type)
        ) + (OSError,)

    def dispatch(self, handle: str, meta: JobRequestMetadata, image_png: bytes, hint_png: bytes) -> str:
        call = self._spawn(handle, json.dumps(meta.to_dict(), sort_keys=True), image_png, hint_png)
        return str(call.object_id)

    def poll(self, handle: str, native_id: str) -> WorkerOutcome:
        exc = self._exc
        try:
            result = self._from_id(native_id).get(timeout=0)
        except exc.OutputExpiredError:
            return WorkerOutcome("failed", error_code="result_expired", message="The provider no longer holds this job's result")
        except exc.FunctionTimeoutError:
            return WorkerOutcome("failed", error_code="worker_timeout", message="The GPU worker ran past its time limit")
        except TimeoutError:
            # The builtin TimeoutError (not modal's) means no output yet. It is also an
            # OSError, so it must be handled before the transient group.
            started = self._run_started(handle)
            return WorkerOutcome("running" if started else "pending")
        except self._transient:
            raise
        except Exception as error:
            # Remote exceptions, crashed or cancelled inputs: the call is over and failed.
            return WorkerOutcome("failed", error_code="worker_failed", message=f"GPU worker failed: {error}")
        return outcome_from_result(result)

    def _run_started(self, handle: str) -> bool:
        try:
            return self._store.get(run_key(handle)) is not None
        except Exception as error:
            logger.error("Run marker read failed for %s: %s", handle, redact_text(str(error)))
            return False

    def fetch_result(self, handle: str, native_id: str) -> Optional[bytes]:
        outcome = self.poll(handle, native_id)
        return outcome.png if outcome.state == "completed" else None

    def cancel(self, handle: str, native_id: str) -> None:
        # terminate_containers=False keeps the warm container and its loaded model;
        # Modal interrupts the running input instead.
        self._from_id(native_id).cancel(terminate_containers=False)


def worker_render(
    runtime: WorkerRuntime,
    store: Any,
    handle: str,
    metadata_json: str,
    image_png: bytes,
    hint_png: bytes,
    clock: Callable[[], float] = time.time,
) -> Dict[str, Any]:
    """Body of the GPU worker method: mark the run, honour a cancel, render."""
    cancelled = begin_run(store, handle, clock)
    if cancelled is not None:
        return cancelled
    return runtime.render(metadata_json, image_png, hint_png)
