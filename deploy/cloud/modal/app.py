"""Modal Cloud Deployment Template and Adapter.

Provides the production-shaped Modal app template with CPU ASGI control gateway
and GPU diffusion worker, plus an offline test adapter that exercises the
application-owned `/mc/v1` contract without importing paid SDKs or incurring costs.
"""

from __future__ import annotations

import logging
import uuid
from typing import Any, Callable, Dict, Optional, Tuple

from deploy.cloud.common.contract import (
    PROTOCOL_VERSION,
    CloudProvider,
    JobAcceptedResponse,
    JobExecutionStatus,
    JobRequestMetadata,
    JobStatusResponse,
    ModelInfoResponse,
    ServiceLimits,
    TypedError,
    provisional_fixture_limits,
)
from deploy.cloud.common.api import CloudGateway
from deploy.cloud.common.flux import FluxWorker
from deploy.cloud.common.handle_mapping import InMemoryHandleRegistry, JobRecord
from deploy.cloud.common.manifest import get_default_model_info
from deploy.cloud.common.redact import redact_text

logger = logging.getLogger("deploy.cloud.modal")

# Lazy, optional import of the official Modal SDK
try:
    import modal  # type: ignore
    _MODAL_SDK_AVAILABLE: bool = True
except ImportError:
    modal = None
    _MODAL_SDK_AVAILABLE: bool = False


class ModalAppAdapter:
    """Offline-testable Modal provider adapter implementing `/mc/v1`."""

    def __init__(
        self,
        token_id: Optional[str] = None,
        token_secret: Optional[str] = None,
        model_info: Optional[ModelInfoResponse] = None,
        limits: Optional[ServiceLimits] = None,
        handle_registry: Optional[InMemoryHandleRegistry] = None,
        worker: Optional[FluxWorker] = None,
        auth_validator: Optional[Callable[[Dict[str, str]], bool]] = None,
        auto_execute: bool = True,
    ):
        self.provider = CloudProvider.MODAL
        self.token_id = token_id
        self.token_secret = token_secret
        self.auth_validator = auth_validator
        self.limits = limits or provisional_fixture_limits()
        self.model_info = model_info or get_default_model_info("modal", limits=self.limits)
        self.handle_registry = handle_registry or InMemoryHandleRegistry()
        self.worker = worker or FluxWorker(provider="modal", limits=self.limits)
        self.auto_execute = auto_execute

        # Instantiate shared CPU Gateway bound to Modal proxy auth
        self.gateway = CloudGateway(
            provider=self.provider,
            model_info=self.model_info,
            limits=self.limits,
            handle_registry=self.handle_registry,
            worker=self.worker,
            auth_validator=self.auth_validator,
            proxy_token_id=self.token_id,
            proxy_token_secret=self.token_secret,
            auto_execute=self.auto_execute,
        )

    def handle_request(
        self,
        method: str,
        path: str,
        headers: Dict[str, str],
        body: bytes,
    ) -> Tuple[int, Dict[str, str], bytes]:
        """Dispatch HTTP request through the Modal-configured CPU gateway."""
        return self.gateway.handle_http_request(method, path, headers, body)

    def as_asgi_app(self) -> Callable:
        """Expose ASGI 3.0 application for Modal web endpoint serving."""
        return self.gateway.as_asgi_app()

    def as_wsgi_app(self) -> Callable:
        """Expose WSGI application callable."""
        return self.gateway.as_wsgi_app()

    def simulate_function_call(
        self,
        job_meta: JobRequestMetadata,
        img_bytes: bytes,
        hint_bytes: bytes,
    ) -> str:
        """Simulate native Modal FunctionCall spawning and return native call handle."""
        native_id = f"fc-{uuid.uuid4().hex}"
        app_handle = f"handle-modal-{uuid.uuid4().hex}"
        record = self.handle_registry.register_job(
            job_meta=job_meta,
            native_handle=native_id,
            app_handle=app_handle,
            provider="modal",
        )
        self.gateway.execute_worker_job(record, img_bytes, hint_bytes)
        return record.native_handle

    @staticmethod
    def deploy_live() -> None:
        """Explicit refusal of unverified live cloud deployments."""
        raise NotImplementedError(
            "Live Modal deployment requires authorized platform credentials and explicit staging approval. "
            "Offline verification mode only."
        )


def create_modal_gateway(
    token_id: Optional[str] = None,
    token_secret: Optional[str] = None,
    limits: Optional[ServiceLimits] = None,
    auth_validator: Optional[Callable[[Dict[str, str]], bool]] = None,
) -> CloudGateway:
    """Factory helper creating a Modal-bound CloudGateway with required secrets validation."""
    if not (token_id and token_secret) and auth_validator is None:
        raise ValueError(
            "Production Modal gateway construction requires injected secrets (token_id, token_secret) or auth_validator"
        )
    adapter = ModalAppAdapter(
        token_id=token_id,
        token_secret=token_secret,
        limits=limits,
        auth_validator=auth_validator,
    )
    return adapter.gateway


def build_modal_app(
    app_name: str = "manga-cleaner-modal",
    volume_name: str = "manga-cleaner-weights",
    secret_name: str = "manga-cleaner-modal-secret",
    token_id: Optional[str] = None,
    token_secret: Optional[str] = None,
) -> Any:
    """Explicit factory builder for Modal production app definition.

    Fails closed before resource definitions to avoid advertising or deploying
    a non-inferencing stub worker. Live deployment requires authorized platform
    credentials and verified model pipeline.
    """
    if not _MODAL_SDK_AVAILABLE or modal is None:
        raise RuntimeError("Modal SDK is not installed; cannot build Modal production app")

    raise NotImplementedError(
        "Live Modal deployment requires authorized platform credentials and verified model pipeline. "
        "build_modal_app fails closed before resource definitions to prevent deploying an unverified stub."
    )
