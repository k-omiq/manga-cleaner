"""Beam Cloud Deployment Template and Adapter.

Provides the production-shaped Beam app template with CPU control gateway
and task queue worker, plus an offline test adapter that exercises the
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

logger = logging.getLogger("deploy.cloud.beam")

# Lazy, optional import of the official Beam SDK
try:
    import beam  # type: ignore
    _BEAM_SDK_AVAILABLE: bool = True
except ImportError:
    beam = None
    _BEAM_SDK_AVAILABLE: bool = False


class BeamAppAdapter:
    """Offline-testable Beam provider adapter implementing `/mc/v1`."""

    def __init__(
        self,
        bearer_token: Optional[str] = None,
        model_info: Optional[ModelInfoResponse] = None,
        limits: Optional[ServiceLimits] = None,
        handle_registry: Optional[InMemoryHandleRegistry] = None,
        worker: Optional[FluxWorker] = None,
        auth_validator: Optional[Callable[[Dict[str, str]], bool]] = None,
        auto_execute: bool = True,
    ):
        self.provider = CloudProvider.BEAM
        self.bearer_token = bearer_token
        self.auth_validator = auth_validator
        self.limits = limits or provisional_fixture_limits()
        self.model_info = model_info or get_default_model_info("beam", limits=self.limits)
        self.handle_registry = handle_registry or InMemoryHandleRegistry()
        self.worker = worker or FluxWorker(provider="beam", limits=self.limits)
        self.auto_execute = auto_execute

        # Instantiate shared CPU Gateway bound to Beam bearer auth
        self.gateway = CloudGateway(
            provider=self.provider,
            model_info=self.model_info,
            limits=self.limits,
            handle_registry=self.handle_registry,
            worker=self.worker,
            auth_validator=self.auth_validator,
            bearer_token=self.bearer_token,
            auto_execute=self.auto_execute,
        )

    def handle_request(
        self,
        method: str,
        path: str,
        headers: Dict[str, str],
        body: bytes,
    ) -> Tuple[int, Dict[str, str], bytes]:
        """Dispatch HTTP request through the Beam-configured CPU gateway."""
        return self.gateway.handle_http_request(method, path, headers, body)

    def as_asgi_app(self) -> Callable:
        """Expose ASGI 3.0 application for Beam web endpoint serving."""
        return self.gateway.as_asgi_app()

    def as_wsgi_app(self) -> Callable:
        """Expose WSGI application callable."""
        return self.gateway.as_wsgi_app()

    def simulate_task_enqueue(
        self,
        job_meta: JobRequestMetadata,
        img_bytes: bytes,
        hint_bytes: bytes,
        presigned_storage_url: Optional[str] = None,
    ) -> str:
        """Simulate native Beam task queue submission and return native task handle."""
        native_task_id = f"task-{uuid.uuid4().hex}"
        app_handle = f"handle-beam-{uuid.uuid4().hex}"
        record = self.handle_registry.register_job(
            job_meta=job_meta,
            native_handle=native_task_id,
            app_handle=app_handle,
            provider="beam",
        )
        if presigned_storage_url:
            record.internal_storage_ref = presigned_storage_url

        self.gateway.execute_worker_job(
            record,
            img_bytes,
            hint_bytes,
            internal_storage_ref=presigned_storage_url,
        )
        return record.native_handle

    @staticmethod
    def deploy_live() -> None:
        """Explicit refusal of unverified live cloud deployments."""
        raise NotImplementedError(
            "Live Beam deployment requires authorized platform credentials and explicit staging approval. "
            "Offline verification mode only."
        )


def create_beam_gateway(
    bearer_token: Optional[str] = None,
    limits: Optional[ServiceLimits] = None,
    auth_validator: Optional[Callable[[Dict[str, str]], bool]] = None,
) -> CloudGateway:
    """Factory helper creating a Beam-bound CloudGateway with required secrets validation."""
    if not bearer_token and auth_validator is None:
        raise ValueError(
            "Production Beam gateway construction requires injected secrets (bearer_token) or auth_validator"
        )
    adapter = BeamAppAdapter(
        bearer_token=bearer_token,
        limits=limits,
        auth_validator=auth_validator,
    )
    return adapter.gateway


def build_beam_app(
    volume_name: str = "manga-cleaner-models",
    mount_path: str = "/models",
    bearer_token: Optional[str] = None,
) -> Any:
    """Explicit factory builder for Beam production app definition.

    Fails closed before resource definitions to avoid advertising or deploying
    a non-inferencing stub worker. Live deployment requires authorized platform
    credentials and verified model pipeline.
    """
    if not _BEAM_SDK_AVAILABLE or beam is None:
        raise RuntimeError("Beam SDK is not installed; cannot build Beam production app")

    raise NotImplementedError(
        "Live Beam deployment requires authorized platform credentials and verified model pipeline. "
        "build_beam_app fails closed before resource definitions to prevent deploying an unverified stub."
    )
