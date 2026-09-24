"""Provider-Neutral CPU Control Gateway and API Implementation.

Implements the application-owned `/mc/v1` wire contract routes, bounded multipart
payload parsing, authenticated control plane dispatch, and secure result streaming.
"""

from __future__ import annotations

import asyncio
import email
import email.policy
import hmac
import json
import logging
from typing import Any, Callable, Dict, List, Optional, Set, Tuple, Union

from deploy.cloud.common.contract import (
    PROTOCOL_VERSION,
    CloudProvider,
    ContractValidationError,
    HealthResponse,
    JobAcceptedResponse,
    JobCancelResponse,
    JobExecutionStatus,
    JobRequestMetadata,
    JobStatusResponse,
    ModelInfoResponse,
    PreEnqueueRejectionResponse,
    RenderRecipe,
    ResultMetadata,
    ServiceLimits,
    TypedError,
    WarmupResponse,
    provisional_fixture_limits,
    validate_crop_payload,
    validate_recipe_compatibility,
    validate_worker_result,
)
from deploy.cloud.common.flux import FluxWorker, sampling_mismatch
from deploy.cloud.common.handle_mapping import InMemoryHandleRegistry, JobRecord
from deploy.cloud.common.jobs import (
    LocalJobBackend,
    PreEnqueueError,
    ResultUnavailableError,
    execute_worker_job,
)
from deploy.cloud.common.manifest import get_default_model_info
from deploy.cloud.common.redact import redact_text

logger = logging.getLogger("deploy.cloud.gateway")


def parse_multipart_payload(
    content_type_header: str,
    body_bytes: bytes,
    max_multipart_bytes: int,
) -> Tuple[Optional[str], Optional[bytes], Optional[bytes], Optional[str]]:
    """Parse multipart/form-data payload into (metadata_json_str, image_bytes, hint_bytes, error_msg)."""
    if len(body_bytes) > max_multipart_bytes:
        return None, None, None, f"Payload size {len(body_bytes)} exceeds maximum limit {max_multipart_bytes}"

    # Extract boundary
    ct_lower = content_type_header.lower()
    if not ct_lower.startswith("multipart/form-data"):
        return None, None, None, f"Expected Content-Type multipart/form-data, got '{content_type_header}'"

    try:
        header_line = f"Content-Type: {content_type_header}\r\n\r\n".encode("utf-8")
        msg = email.message_from_bytes(header_line + body_bytes, policy=email.policy.default)

        metadata_str: Optional[str] = None
        image_bytes: Optional[bytes] = None
        hint_bytes: Optional[bytes] = None
        seen_fields: Set[str] = set()

        for part in msg.iter_parts():
            disp = part.get("content-disposition", "")
            if not disp or "form-data" not in disp.lower():
                return None, None, None, "Malformed multipart part: missing or invalid Content-Disposition"

            name = part.get_param("name", header="content-disposition")
            if not name:
                return None, None, None, "Multipart part missing 'name' in Content-Disposition"
            name = str(name).strip().strip('"').strip("'")

            if name not in {"metadata", "image", "hint"}:
                return None, None, None, f"Unknown multipart field '{name}' (only metadata, image, hint allowed)"

            if name in seen_fields:
                return None, None, None, f"Duplicate multipart field '{name}' rejected"
            seen_fields.add(name)

            part_ct = part.get_content_type()
            payload = part.get_payload(decode=True)
            if payload is None:
                return None, None, None, f"Empty or undecodable payload for field '{name}'"

            if name == "metadata":
                if part_ct and part_ct not in ("application/json", "text/plain", "application/octet-stream"):
                    return None, None, None, f"Invalid Content-Type '{part_ct}' for metadata, expected application/json"
                try:
                    metadata_str = payload.decode("utf-8")
                except UnicodeDecodeError:
                    return None, None, None, "Metadata payload is not valid UTF-8"
            elif name == "image":
                if part_ct and part_ct not in ("image/png", "application/octet-stream"):
                    return None, None, None, f"Invalid Content-Type '{part_ct}' for image, expected image/png"
                image_bytes = payload
            elif name == "hint":
                if part_ct and part_ct not in ("image/png", "application/octet-stream"):
                    return None, None, None, f"Invalid Content-Type '{part_ct}' for hint, expected image/png"
                hint_bytes = payload

        missing = {"metadata", "image", "hint"} - seen_fields
        if missing:
            return None, None, None, f"Missing required multipart field(s): {sorted(missing)}"

        return metadata_str, image_bytes, hint_bytes, None
    except Exception as exc:
        return None, None, None, f"Malformed multipart body: {exc}"


class CloudGateway:
    """Provider-agnostic CPU Control Gateway implementing `/mc/v1`."""

    def __init__(
        self,
        provider: Union[CloudProvider, str],
        model_info: Optional[ModelInfoResponse] = None,
        limits: Optional[ServiceLimits] = None,
        handle_registry: Optional[InMemoryHandleRegistry] = None,
        worker: Optional[FluxWorker] = None,
        auth_validator: Optional[Callable[[Dict[str, str]], bool]] = None,
        proxy_token_id: Optional[str] = None,
        proxy_token_secret: Optional[str] = None,
        bearer_token: Optional[str] = None,
        auto_execute: bool = True,
        backend: Optional[Any] = None,
        trust_edge_auth: bool = False,
    ):
        if isinstance(provider, str):
            provider = CloudProvider(provider.lower())
        self.provider = provider
        self.limits = limits or provisional_fixture_limits()
        self.model_info = model_info or get_default_model_info(self.provider.value, limits=self.limits)
        self.handle_registry = handle_registry or InMemoryHandleRegistry()
        # The in-process fake worker only exists for the local backend; a deployed
        # gateway dispatches to the provider and must never render a stand-in result.
        if worker is None and backend is None:
            worker = FluxWorker(provider=self.provider.value, limits=self.limits)
        self.worker = worker
        self.auth_validator = auth_validator
        self.proxy_token_id = proxy_token_id
        self.proxy_token_secret = proxy_token_secret
        self.bearer_token = bearer_token
        self.auto_execute = auto_execute
        # Deployed gateways sit behind provider edge auth (Modal proxy auth, Beam
        # authorized endpoints): requests without valid credentials never reach this
        # code, and the gateway holds no copy of those credentials to compare against.
        self.trust_edge_auth = trust_edge_auth
        self.backend = backend or LocalJobBackend(
            provider=self.provider.value,
            registry=self.handle_registry,
            worker=self.worker,
            limits=self.limits,
            auto_execute=auto_execute,
        )

    def check_auth(self, headers: Dict[str, str]) -> bool:
        """Authenticate incoming request according to provider configuration.

        Fails closed when credentials / auth validator are absent.
        """
        if self.trust_edge_auth:
            return True

        # Custom validator takes precedence if provided
        if self.auth_validator is not None:
            return self.auth_validator(headers)

        # Normalize header keys to lowercase
        norm_headers = {k.lower(): v for k, v in headers.items()}

        if self.provider == CloudProvider.MODAL:
            # Must fail closed if no proxy credentials configured
            if not self.proxy_token_id or not self.proxy_token_secret:
                return False
            req_key = norm_headers.get("modal-key", "")
            req_secret = norm_headers.get("modal-secret", "")
            if not req_key or not req_secret:
                return False
            return hmac.compare_digest(req_key, self.proxy_token_id) and hmac.compare_digest(req_secret, self.proxy_token_secret)

        elif self.provider == CloudProvider.BEAM:
            # Must fail closed if no bearer token configured
            if not self.bearer_token:
                return False
            auth_val = norm_headers.get("authorization", "")
            if not auth_val:
                return False
            expected = f"Bearer {self.bearer_token}"
            return hmac.compare_digest(auth_val, expected)

        return False

    def handle_http_request(
        self,
        method: str,
        raw_path: str,
        headers: Dict[str, str],
        body: bytes,
    ) -> Tuple[int, Dict[str, str], bytes]:
        """Dispatch HTTP request and return (status_code, response_headers, body_bytes)."""
        method = method.upper()
        # Strip query string and normalize path
        path = raw_path.split("?", 1)[0].rstrip("/")
        if not path:
            path = "/"

        # Normalize path prefixes: strip leading /mc/v1 if present
        norm_path = path
        if norm_path.startswith("/mc/v1"):
            norm_path = norm_path[6:]
            if not norm_path:
                norm_path = "/"

        # 1. Enforce Authentication on all /mc/v1 routes
        if not self.check_auth(headers):
            err_body = {
                "error_code": "unauthorized",
                "message": "Authentication failed: missing or invalid credentials",
            }
            return (
                401,
                {"Content-Type": "application/json", "X-Content-Type-Options": "nosniff"},
                json.dumps(err_body).encode("utf-8"),
            )

        # 2. Route Dispatch
        try:
            if method == "GET" and norm_path == "/health":
                return self._route_health()

            if method == "GET" and norm_path == "/model-info":
                return self._route_model_info()

            if method == "POST" and norm_path == "/jobs":
                return self._route_submit_job(headers, body)

            if method == "GET" and norm_path.startswith("/jobs/"):
                segments = [s for s in norm_path[6:].split("/") if s]
                if len(segments) == 1:
                    handle = segments[0]
                    return self._route_job_status(handle)
                elif len(segments) == 2 and segments[1] == "result":
                    handle = segments[0]
                    return self._route_job_result(handle)

            if method == "POST" and norm_path.startswith("/jobs/") and norm_path.endswith("/cancel"):
                segments = [s for s in norm_path[6:].split("/") if s]
                if len(segments) == 2 and segments[1] == "cancel":
                    handle = segments[0]
                    return self._route_job_cancel(handle)

            if method == "POST" and norm_path == "/warmup":
                return self._route_warmup()

            # 404 Route Not Found
            not_found = {
                "error_code": "not_found",
                "message": f"Route '{path}' with method '{method}' not found",
            }
            return (
                404,
                {"Content-Type": "application/json"},
                json.dumps(not_found).encode("utf-8"),
            )

        except Exception as exc:
            err_msg = redact_text(str(exc))
            logger.error("Internal gateway error: %s", err_msg)
            err_payload = {
                "error_code": "internal_error",
                "message": f"Internal gateway error: {err_msg}",
            }
            return (
                500,
                {"Content-Type": "application/json"},
                json.dumps(err_payload).encode("utf-8"),
            )

    def _route_health(self) -> Tuple[int, Dict[str, str], bytes]:
        """GET /health - Authenticated control reachability."""
        resp = HealthResponse(
            status="ok",
            provider=self.provider.value,
            protocol_version=PROTOCOL_VERSION,
        )
        return (
            200,
            {"Content-Type": "application/json"},
            json.dumps(resp.to_dict()).encode("utf-8"),
        )

    def _route_model_info(self) -> Tuple[int, Dict[str, str], bytes]:
        """GET /model-info - Pinned recipe and limits."""
        return (
            200,
            {"Content-Type": "application/json"},
            json.dumps(self.model_info.to_dict()).encode("utf-8"),
        )

    def _route_submit_job(
        self,
        headers: Dict[str, str],
        body: bytes,
    ) -> Tuple[int, Dict[str, str], bytes]:
        """POST /jobs - Multipart crop submission with structured validation."""
        norm_headers = {k.lower(): v for k, v in headers.items()}
        ct_header = norm_headers.get("content-type", "")

        # 1. Parse multipart form
        meta_str, img_bytes, hint_bytes, parse_err = parse_multipart_payload(
            ct_header, body, self.limits.max_multipart_bytes
        )

        if parse_err is not None or meta_str is None or img_bytes is None or hint_bytes is None:
            err_msg = parse_err or "Malformed multipart payload"
            if parse_err and "exceeds" in parse_err.lower():
                rejection = PreEnqueueRejectionResponse(
                    enqueued=False,
                    error_code="payload_too_large",
                    message=redact_text(err_msg),
                    job_id="unknown",
                    attempt_id="unknown",
                    request_digest="0000000000000000000000000000000000000000000000000000000000000000",
                    retryable=False,
                )
                return 413, {"Content-Type": "application/json"}, json.dumps(rejection.to_dict()).encode("utf-8")

            err_code = "forbidden_fields" if (parse_err and ("unknown" in parse_err.lower() or "duplicate" in parse_err.lower())) else "invalid_image"
            rejection = PreEnqueueRejectionResponse(
                enqueued=False,
                error_code=err_code,
                message=redact_text(err_msg),
                job_id="unknown",
                attempt_id="unknown",
                request_digest="0000000000000000000000000000000000000000000000000000000000000000",
                retryable=False,
            )
            return 400, {"Content-Type": "application/json"}, json.dumps(rejection.to_dict()).encode("utf-8")

        # 2. Parse request metadata
        try:
            meta_dict = json.loads(meta_str)
            job_meta = JobRequestMetadata.from_dict(meta_dict)
        except Exception as exc:
            rejection = PreEnqueueRejectionResponse(
                enqueued=False,
                error_code="forbidden_fields" if "forbidden" in str(exc).lower() else "unknown_protocol_version",
                message=redact_text(str(exc)),
                job_id="unknown",
                attempt_id="unknown",
                request_digest="0000000000000000000000000000000000000000000000000000000000000000",
                retryable=False,
            )
            return 400, {"Content-Type": "application/json"}, json.dumps(rejection.to_dict()).encode("utf-8")

        # 3. Validate crop payload and geometry bounds
        try:
            validate_crop_payload(job_meta, img_bytes, hint_bytes, self.limits)
        except ContractValidationError as exc:
            err_msg = str(exc)
            err_msg_lower = err_msg.lower()
            if "sha-256" in err_msg_lower or "sha256" in err_msg_lower or "digest" in err_msg_lower or "hash" in err_msg_lower:
                err_code = "invalid_digest"
            elif "stride" in err_msg_lower or "dimension" in err_msg_lower or "width" in err_msg_lower or "height" in err_msg_lower:
                err_code = "invalid_dimensions"
            elif "pixel" in err_msg_lower or "limit" in err_msg_lower or "exceeds" in err_msg_lower or "too large" in err_msg_lower:
                err_code = "payload_too_large"
            elif "png" in err_msg_lower or "checksum" in err_msg_lower or "ihdr" in err_msg_lower or "idat" in err_msg_lower or "iend" in err_msg_lower or "filter" in err_msg_lower:
                err_code = "invalid_image"
            else:
                err_code = "invalid_image"

            rejection = PreEnqueueRejectionResponse(
                enqueued=False,
                error_code=err_code,
                message=redact_text(err_msg),
                job_id=job_meta.job_id,
                attempt_id=job_meta.attempt_id,
                request_digest=job_meta.request_digest,
                retryable=False,
            )
            return 400, {"Content-Type": "application/json"}, json.dumps(rejection.to_dict()).encode("utf-8")

        # 4. Validate recipe compatibility against advertised model info
        try:
            validate_recipe_compatibility(job_meta, self.model_info)
        except ContractValidationError as exc:
            rejection = PreEnqueueRejectionResponse(
                enqueued=False,
                error_code="unsupported_recipe",
                message=redact_text(str(exc)),
                job_id=job_meta.job_id,
                attempt_id=job_meta.attempt_id,
                request_digest=job_meta.request_digest,
                retryable=False,
            )
            return 400, {"Content-Type": "application/json"}, json.dumps(rejection.to_dict()).encode("utf-8")

        # 4b. A recipe that pins its sampling (the production recipe) only renders with
        # exactly those values; anything else would be a different image under the same id.
        mismatch = sampling_mismatch(job_meta)
        if mismatch:
            rejection = PreEnqueueRejectionResponse(
                enqueued=False,
                error_code="unsupported_recipe",
                message=redact_text(mismatch),
                job_id=job_meta.job_id,
                attempt_id=job_meta.attempt_id,
                request_digest=job_meta.request_digest,
                retryable=False,
            )
            return 400, {"Content-Type": "application/json"}, json.dumps(rejection.to_dict()).encode("utf-8")

        # 5. Register (deduplicated on job_id + attempt_id) and hand to the worker
        try:
            record = self.backend.submit(job_meta, img_bytes, hint_bytes)
        except ContractValidationError as exc:
            rejection = PreEnqueueRejectionResponse(
                enqueued=False,
                error_code="invalid_digest",
                message=redact_text(str(exc)),
                job_id=job_meta.job_id,
                attempt_id=job_meta.attempt_id,
                request_digest=job_meta.request_digest,
                retryable=False,
            )
            return 409, {"Content-Type": "application/json"}, json.dumps(rejection.to_dict()).encode("utf-8")
        except PreEnqueueError as exc:
            rejection = PreEnqueueRejectionResponse(
                enqueued=False,
                error_code=exc.error_code,
                message=redact_text(str(exc)) or exc.error_code,
                job_id=job_meta.job_id,
                attempt_id=job_meta.attempt_id,
                request_digest=job_meta.request_digest,
                retryable=exc.retryable,
            )
            return exc.http_status, {"Content-Type": "application/json"}, json.dumps(rejection.to_dict()).encode("utf-8")

        # 6. Return 202 Accepted reporting pending per contract (status endpoint holds actual state)
        accepted = JobAcceptedResponse(
            handle=record.app_handle,
            status=JobExecutionStatus.PENDING.value,
            job_id=job_meta.job_id,
            attempt_id=job_meta.attempt_id,
            request_digest=job_meta.request_digest,
            recipe_id=job_meta.recipe.recipe_id,
            preprocessing_version=job_meta.recipe.preprocessing_version,
            model_id=job_meta.recipe.model_id,
            model_revision=job_meta.recipe.model_revision,
            native_mask_conditioning=False,
        )
        return 202, {"Content-Type": "application/json"}, json.dumps(accepted.to_dict()).encode("utf-8")

    def _route_job_status(self, handle: str) -> Tuple[int, Dict[str, str], bytes]:
        """GET /jobs/{handle} - Status polling accepting only opaque app handles."""
        record = self.backend.get(handle)
        if not record:
            err = TypedError(error_code="not_found", message=f"Job handle '{handle}' not found")
            return 404, {"Content-Type": "application/json"}, json.dumps(err.to_dict()).encode("utf-8")

        status_resp = JobStatusResponse(
            handle=record.app_handle,
            job_id=record.job_id,
            attempt_id=record.attempt_id,
            request_digest=record.request_digest,
            recipe_id=record.recipe.recipe_id,
            preprocessing_version=record.recipe.preprocessing_version,
            model_id=record.recipe.model_id,
            model_revision=record.recipe.model_revision,
            native_mask_conditioning=False,
            status=record.status.value,
            reported_cost_usd=record.reported_cost_usd,
            error=record.error,
            result_digest=record.result_digest,
            result_bytes=record.result_bytes,
        )
        return 200, {"Content-Type": "application/json"}, json.dumps(status_resp.to_dict()).encode("utf-8")

    def _route_job_result(self, handle: str) -> Tuple[int, Dict[str, str], bytes]:
        """GET /jobs/{handle}/result - Validated result download accepting only opaque app handles."""
        try:
            record, result_png = self.backend.result_png(handle)
        except ResultUnavailableError as exc:
            err = TypedError(
                error_code="result_unavailable",
                message=redact_text(f"Result for '{handle}' is not readable right now: {exc}")[:1024],
            )
            return 503, {"Content-Type": "application/json"}, json.dumps(err.to_dict()).encode("utf-8")
        if not record:
            err = TypedError(error_code="not_found", message=f"Job handle '{handle}' not found")
            return 404, {"Content-Type": "application/json"}, json.dumps(err.to_dict()).encode("utf-8")

        if record.status != JobExecutionStatus.COMPLETED or result_png is None:
            err = TypedError(
                error_code="job_not_ready",
                message=f"Job '{handle}' is not completed (status: {record.status.value})",
            )
            return 409, {"Content-Type": "application/json"}, json.dumps(err.to_dict()).encode("utf-8")

        resp_headers: Dict[str, str] = {
            "Content-Type": "image/png",
            "X-MC-Result-Digest": record.result_digest or "",
        }
        if record.reported_cost_usd is not None:
            resp_headers["X-MC-Reported-Cost-Usd"] = str(record.reported_cost_usd)
        return 200, resp_headers, result_png

    def _route_job_cancel(self, handle: str) -> Tuple[int, Dict[str, str], bytes]:
        """POST /jobs/{handle}/cancel - Request cancellation accepting only opaque app handles."""
        # The backend flags cancel atomically and refuses terminal jobs without mutating them.
        updated_record, accepted = self.backend.cancel(handle)
        if updated_record is None:
            err = TypedError(error_code="not_found", message=f"Job handle '{handle}' not found")
            return 404, {"Content-Type": "application/json"}, json.dumps(err.to_dict()).encode("utf-8")

        if not accepted:
            err = TypedError(
                error_code="job_terminal",
                message=f"Cannot cancel job '{handle}' in terminal state '{updated_record.status.value}'",
            )
            return 409, {"Content-Type": "application/json"}, json.dumps(err.to_dict()).encode("utf-8")

        cancel_resp = JobCancelResponse(
            handle=updated_record.app_handle,
            job_id=updated_record.job_id,
            attempt_id=updated_record.attempt_id,
            request_digest=updated_record.request_digest,
            status="cancel_requested",
            acknowledged=True,
        )
        return 200, {"Content-Type": "application/json"}, json.dumps(cancel_resp.to_dict()).encode("utf-8")

    def _route_warmup(self) -> Tuple[int, Dict[str, str], bytes]:
        """POST /warmup - Explicit worker readiness probe."""
        resp = WarmupResponse(
            status="ok",
            provider=self.provider.value,
            protocol_version=PROTOCOL_VERSION,
            worker_state=self.backend.warmup(),
        )
        return 200, {"Content-Type": "application/json"}, json.dumps(resp.to_dict()).encode("utf-8")

    def execute_worker_job(
        self,
        record: JobRecord,
        img_bytes: bytes,
        hint_bytes: bytes,
        internal_storage_ref: Optional[str] = None,
    ) -> bool:
        """Execute worker inference, validate result buffer against contract, and persist result or typed failure."""
        return execute_worker_job(
            handle_registry=self.handle_registry,
            worker=self.worker,
            record=record,
            img_bytes=img_bytes,
            hint_bytes=hint_bytes,
            limits=self.limits,
            internal_storage_ref=internal_storage_ref,
            auto_execute=self.auto_execute,
        )

    def as_wsgi_app(self) -> Callable:
        """Return a WSGI application callable for serving with standard WSGI servers."""
        def wsgi_app(environ: Dict[str, Any], start_response: Callable) -> List[bytes]:
            method = environ.get("REQUEST_METHOD", "GET")
            path = environ.get("PATH_INFO", "/")
            query = environ.get("QUERY_STRING", "")
            if query:
                path = f"{path}?{query}"

            headers: Dict[str, str] = {}
            for k, v in environ.items():
                if k.startswith("HTTP_"):
                    header_name = k[5:].replace("_", "-").title()
                    headers[header_name] = str(v)
                elif k in ("CONTENT_TYPE", "CONTENT_LENGTH"):
                    header_name = k.replace("_", "-").title()
                    headers[header_name] = str(v)

            content_length_str = environ.get("CONTENT_LENGTH", "")
            content_length = int(content_length_str) if content_length_str.isdigit() else 0
            if content_length > self.limits.max_multipart_bytes:
                err_payload = json.dumps({
                    "error_code": "payload_too_large",
                    "message": f"Payload size {content_length} exceeds maximum limit {self.limits.max_multipart_bytes}",
                }).encode("utf-8")
                start_response("413 Payload Too Large", [("Content-Type", "application/json"), ("X-Content-Type-Options", "nosniff")])
                return [err_payload]

            body = bytearray()
            input_stream = environ.get("wsgi.input")
            if input_stream:
                if content_length > 0:
                    remaining = content_length
                    chunk_size = 65536
                    while remaining > 0:
                        read_len = min(remaining, chunk_size)
                        chunk = input_stream.read(read_len)
                        if not chunk:
                            break
                        body.extend(chunk)
                        remaining -= len(chunk)
                        if len(body) > self.limits.max_multipart_bytes:
                            err_payload = json.dumps({
                                "error_code": "payload_too_large",
                                "message": f"Payload size exceeds maximum limit {self.limits.max_multipart_bytes}",
                            }).encode("utf-8")
                            start_response("413 Payload Too Large", [("Content-Type", "application/json"), ("X-Content-Type-Options", "nosniff")])
                            return [err_payload]
                elif method in ("POST", "PUT", "PATCH") and not content_length_str:
                    chunk_size = 65536
                    while True:
                        chunk = input_stream.read(chunk_size)
                        if not chunk:
                            break
                        body.extend(chunk)
                        if len(body) > self.limits.max_multipart_bytes:
                            err_payload = json.dumps({
                                "error_code": "payload_too_large",
                                "message": f"Payload size exceeds maximum limit {self.limits.max_multipart_bytes}",
                            }).encode("utf-8")
                            start_response("413 Payload Too Large", [("Content-Type", "application/json"), ("X-Content-Type-Options", "nosniff")])
                            return [err_payload]

            status_code, resp_headers, resp_body = self.handle_http_request(
                method=method,
                raw_path=path,
                headers=headers,
                body=bytes(body),
            )

            status_line = f"{status_code} OK"
            if status_code == 202:
                status_line = "202 Accepted"
            elif status_code == 400:
                status_line = "400 Bad Request"
            elif status_code == 401:
                status_line = "401 Unauthorized"
            elif status_code == 404:
                status_line = "404 Not Found"
            elif status_code == 409:
                status_line = "409 Conflict"
            elif status_code == 413:
                status_line = "413 Payload Too Large"
            elif status_code == 500:
                status_line = "500 Internal Server Error"
            elif status_code == 503:
                status_line = "503 Service Unavailable"

            start_response(status_line, list(resp_headers.items()))
            return [resp_body]

        return wsgi_app

    def as_asgi_app(self) -> Callable:
        """Return an ASGI 3.0 application callable for serving with ASGI servers."""
        async def asgi_app(scope: Dict[str, Any], receive: Callable, send: Callable) -> None:
            if scope["type"] == "http":
                method = scope["method"]
                path = scope["path"]
                query_string = scope.get("query_string", b"").decode("utf-8")
                if query_string:
                    path = f"{path}?{query_string}"

                headers = {}
                content_length = None
                for raw_k, raw_v in scope.get("headers", []):
                    k_str = raw_k.decode("latin1").title()
                    v_str = raw_v.decode("latin1")
                    headers[k_str] = v_str
                    if k_str.lower() == "content-length" and v_str.isdigit():
                        content_length = int(v_str)

                if content_length is not None and content_length > self.limits.max_multipart_bytes:
                    err_payload = json.dumps({
                        "error_code": "payload_too_large",
                        "message": f"Payload size {content_length} exceeds maximum limit {self.limits.max_multipart_bytes}",
                    }).encode("utf-8")
                    await send({
                        "type": "http.response.start",
                        "status": 413,
                        "headers": [(b"content-type", b"application/json"), (b"x-content-type-options", b"nosniff")],
                    })
                    await send({
                        "type": "http.response.body",
                        "body": err_payload,
                    })
                    return

                body = bytearray()
                more_body = True
                while more_body:
                    message = await receive()
                    chunk = message.get("body", b"")
                    body.extend(chunk)
                    if len(body) > self.limits.max_multipart_bytes:
                        err_payload = json.dumps({
                            "error_code": "payload_too_large",
                            "message": f"Payload size exceeds maximum limit of {self.limits.max_multipart_bytes} bytes",
                        }).encode("utf-8")
                        await send({
                            "type": "http.response.start",
                            "status": 413,
                            "headers": [(b"content-type", b"application/json"), (b"x-content-type-options", b"nosniff")],
                        })
                        await send({
                            "type": "http.response.body",
                            "body": err_payload,
                        })
                        return
                    more_body = message.get("more_body", False)

                # Backends make blocking provider calls; a worker thread keeps the event
                # loop free for concurrent status polls.
                status_code, resp_headers, resp_body = await asyncio.to_thread(
                    self.handle_http_request,
                    method,
                    path,
                    headers,
                    bytes(body),
                )

                asgi_headers = [
                    (k.encode("latin1"), v.encode("latin1")) for k, v in resp_headers.items()
                ]

                await send({
                    "type": "http.response.start",
                    "status": status_code,
                    "headers": asgi_headers,
                })
                await send({
                    "type": "http.response.body",
                    "body": resp_body,
                })
            elif scope["type"] == "lifespan":
                while True:
                    message = await receive()
                    if message["type"] == "lifespan.startup":
                        await send({"type": "lifespan.startup.complete"})
                    elif message["type"] == "lifespan.shutdown":
                        await send({"type": "lifespan.shutdown.complete"})
                        return
        return asgi_app
