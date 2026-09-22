"""Integration and parity tests for Modal and Beam adapters, cold/warm lifecycle, and fault injection."""

import json
import tempfile
import unittest
from pathlib import Path
from typing import Dict, Tuple
from unittest.mock import MagicMock, patch


from deploy.cloud.beam.app import BeamAppAdapter, build_beam_app, create_beam_gateway
from deploy.cloud.common.contract import (
    PROTOCOL_VERSION,
    CloudProvider,
    HealthResponse,
    JobAcceptedResponse,
    JobCancelResponse,
    JobExecutionStatus,
    JobRequestMetadata,
    JobStatusResponse,
    ModelInfoResponse,
    PreEnqueueRejectionResponse,
    ResultMetadata,
    WarmupResponse,
    compute_request_digest,
    provisional_fixture_limits,
    validate_cancel_response,
    validate_health_response,
    validate_job_accepted_response,
    validate_job_request_metadata,
    validate_model_info_response,
    validate_png_header,
    validate_response_binding,
    validate_result_bytes,
    validate_worker_result,
)
from deploy.cloud.common.api import CloudGateway
from deploy.cloud.common.flux import FluxWorker, WorkerLifecycleState, encode_png_rgb8
from deploy.cloud.common.handle_mapping import InMemoryHandleRegistry, JobRecord
from deploy.cloud.common.manifest import (
    MODEL_TEST_FLUX,
    RECIPE_TEST_SDNQ,
    REVISION_TEST_FLUX,
    ManifestFileRecord,
    ModelManifest,
    get_default_model_info,
    is_recipe_supported,
    verify_model_manifest,
)
from deploy.cloud.common.redact import REDACTED_PRESIGNED_URL, REDACTED_SECRET, redact_dict, redact_text
from deploy.cloud.modal.app import ModalAppAdapter, build_modal_app, create_modal_gateway
from deploy.cloud.tests.test_gateway import build_multipart_body

FIXTURES_DIR = Path(__file__).resolve().parent.parent / "fixtures"


class TestCloudAdapters(unittest.TestCase):
    def setUp(self):
        with open(FIXTURES_DIR / "tiny_image.png", "rb") as f:
            self.tiny_image_bytes = f.read()
        with open(FIXTURES_DIR / "tiny_hint.png", "rb") as f:
            self.tiny_hint_bytes = f.read()
        with open(FIXTURES_DIR / "job_request_metadata.json", "r", encoding="utf-8") as f:
            self.job_meta_dict = json.load(f)
            self.job_meta_json = json.dumps(self.job_meta_dict)
            self.job_meta = JobRequestMetadata.from_dict(self.job_meta_dict)

        self.limits = provisional_fixture_limits()

        # Modal Adapter instance with explicit test credentials
        self.modal_adapter = ModalAppAdapter(
            token_id="modal-key-abc",
            token_secret="modal-secret-xyz",
            limits=self.limits,
        )
        self.modal_auth = {"Modal-Key": "modal-key-abc", "Modal-Secret": "modal-secret-xyz"}

        # Beam Adapter instance with explicit test credentials
        self.beam_adapter = BeamAppAdapter(
            bearer_token="beam-token-123",
            limits=self.limits,
        )
        self.beam_auth = {"Authorization": "Bearer beam-token-123"}

    def test_modal_and_beam_fixture_parity(self):
        """Verify both Modal and Beam adapters produce exact matching fixture structures."""
        # 1. Health
        _, _, modal_health_raw = self.modal_adapter.handle_request("GET", "/health", self.modal_auth, b"")
        modal_health = HealthResponse.from_dict(json.loads(modal_health_raw.decode("utf-8")))
        self.assertEqual(modal_health.provider, "modal")
        self.assertEqual(modal_health.status, "ok")

        _, _, beam_health_raw = self.beam_adapter.handle_request("GET", "/health", self.beam_auth, b"")
        beam_health = HealthResponse.from_dict(json.loads(beam_health_raw.decode("utf-8")))
        self.assertEqual(beam_health.provider, "beam")
        self.assertEqual(beam_health.status, "ok")

        # 2. Model Info
        _, _, modal_info_raw = self.modal_adapter.handle_request("GET", "/model-info", self.modal_auth, b"")
        modal_info = ModelInfoResponse.from_dict(json.loads(modal_info_raw.decode("utf-8")))
        _, _, beam_info_raw = self.beam_adapter.handle_request("GET", "/model-info", self.beam_auth, b"")
        beam_info = ModelInfoResponse.from_dict(json.loads(beam_info_raw.decode("utf-8")))

        self.assertEqual(modal_info.model_id, beam_info.model_id)
        self.assertEqual(modal_info.model_revision, beam_info.model_revision)
        self.assertEqual(modal_info.recipe_id, beam_info.recipe_id)
        self.assertEqual(modal_info.preprocessing_version, beam_info.preprocessing_version)
        self.assertFalse(modal_info.native_mask_conditioning)
        self.assertFalse(beam_info.native_mask_conditioning)
        self.assertEqual(modal_info.limits.max_width, beam_info.limits.max_width)

    def test_cold_warm_worker_lifecycle(self):
        """Test worker cold start vs pre-warmed execution."""
        worker = FluxWorker(provider="modal", simulated_cold_start_delay=0.0)
        self.assertFalse(worker.is_warm)
        self.assertEqual(worker.state, WorkerLifecycleState.UNINITIALIZED)

        # 1. Explicit warmup probe transitions to WARMED
        warm_res = worker.warmup()
        self.assertTrue(worker.is_warm)
        self.assertEqual(worker.state, WorkerLifecycleState.WARMED)
        self.assertEqual(worker.warmup_count, 1)

        # 2. Subsequent inference executes on warm worker
        rec = JobRecord(
            app_handle="handle-test-warm",
            native_handle="native-test-warm",
            provider="modal",
            job_id="job-1",
            attempt_id="att-1",
            request_digest=self.job_meta.request_digest,
            recipe=self.job_meta.recipe,
            width=16,
            height=16,
            seed=42,
            steps=4,
            guidance_scaled=350,
        )
        res_png, res_digest, cost = worker.infer(rec, self.tiny_image_bytes, self.tiny_hint_bytes)
        validate_png_header(res_png, 16, 16, 2)
        self.assertGreater(len(res_png), 0)
        self.assertTrue(worker.is_warm)
        self.assertEqual(worker.inference_count, 1)

    def test_malformed_and_oversized_output_handling(self):
        """Test worker fault injection and robust gateway failure handling."""
        faulty_worker = FluxWorker(provider="modal")
        faulty_gateway = CloudGateway(
            provider=CloudProvider.MODAL,
            limits=self.limits,
            handle_registry=InMemoryHandleRegistry(),
            worker=faulty_worker,
            proxy_token_id="modal-key-abc",
            proxy_token_secret="modal-secret-xyz",
            auto_execute=True,
        )

        ct_header, multipart_body = build_multipart_body(
            self.job_meta_json, self.tiny_image_bytes, self.tiny_hint_bytes
        )
        req_headers = {"Modal-Key": "modal-key-abc", "Modal-Secret": "modal-secret-xyz", "Content-Type": ct_header}

        # 1. Inject OOM fault
        faulty_worker.simulated_fault = "oom"
        code, _, body = faulty_gateway.handle_http_request("POST", "/mc/v1/jobs", req_headers, multipart_body)
        self.assertEqual(code, 202)
        accepted = JobAcceptedResponse.from_dict(json.loads(body.decode("utf-8")))
        handle = accepted.handle

        code, _, body = faulty_gateway.handle_http_request("GET", f"/mc/v1/jobs/{handle}", req_headers, b"")
        self.assertEqual(code, 200)
        status_resp = JobStatusResponse.from_dict(json.loads(body.decode("utf-8")))
        self.assertEqual(status_resp.status, JobExecutionStatus.FAILED.value)
        self.assertIsNotNone(status_resp.error)
        self.assertEqual(status_resp.error.error_code, "execution_failed")
        self.assertIn("out of memory", status_resp.error.message.lower())

        code, _, _ = faulty_gateway.handle_http_request("GET", f"/mc/v1/jobs/{handle}/result", req_headers, b"")
        self.assertEqual(code, 409)

        # 2. Inject corrupted_output fault
        faulty_worker.simulated_fault = "corrupted_output"
        job_meta_2_dict = dict(self.job_meta_dict)
        job_meta_2_dict["job_id"] = "job-corrupt-001"
        job_meta_2_dict["request_digest"] = compute_request_digest(JobRequestMetadata.from_dict(job_meta_2_dict))
        ct_header2, body2 = build_multipart_body(
            json.dumps(job_meta_2_dict), self.tiny_image_bytes, self.tiny_hint_bytes
        )
        req_headers2 = {"Modal-Key": "modal-key-abc", "Modal-Secret": "modal-secret-xyz", "Content-Type": ct_header2}

        code, _, resp_body = faulty_gateway.handle_http_request("POST", "/mc/v1/jobs", req_headers2, body2)
        self.assertEqual(code, 202)
        accepted2 = JobAcceptedResponse.from_dict(json.loads(resp_body.decode("utf-8")))
        self.assertEqual(accepted2.status, JobExecutionStatus.PENDING.value)
        handle2 = accepted2.handle

        code, _, resp_body = faulty_gateway.handle_http_request("GET", f"/mc/v1/jobs/{handle2}", req_headers2, b"")
        self.assertEqual(code, 200)
        status_resp2 = JobStatusResponse.from_dict(json.loads(resp_body.decode("utf-8")))
        self.assertEqual(status_resp2.status, JobExecutionStatus.FAILED.value)
        self.assertIsNotNone(status_resp2.error)
        self.assertEqual(status_resp2.error.error_code, "execution_failed")
        self.assertIsNone(status_resp2.result_digest)
        self.assertIsNone(status_resp2.result_bytes)

        code, _, _ = faulty_gateway.handle_http_request("GET", f"/mc/v1/jobs/{handle2}/result", req_headers2, b"")
        self.assertEqual(code, 409)

        # 3. Inject wrong_dimensions fault
        faulty_worker.simulated_fault = "wrong_dimensions"
        job_meta_3_dict = dict(self.job_meta_dict)
        job_meta_3_dict["job_id"] = "job-dim-001"
        job_meta_3_dict["request_digest"] = compute_request_digest(JobRequestMetadata.from_dict(job_meta_3_dict))
        ct_header3, body3 = build_multipart_body(
            json.dumps(job_meta_3_dict), self.tiny_image_bytes, self.tiny_hint_bytes
        )
        req_headers3 = {"Modal-Key": "modal-key-abc", "Modal-Secret": "modal-secret-xyz", "Content-Type": ct_header3}

        code, _, resp_body = faulty_gateway.handle_http_request("POST", "/mc/v1/jobs", req_headers3, body3)
        self.assertEqual(code, 202)
        accepted3 = JobAcceptedResponse.from_dict(json.loads(resp_body.decode("utf-8")))
        self.assertEqual(accepted3.status, JobExecutionStatus.PENDING.value)
        handle3 = accepted3.handle

        code, _, resp_body = faulty_gateway.handle_http_request("GET", f"/mc/v1/jobs/{handle3}", req_headers3, b"")
        self.assertEqual(code, 200)
        status_resp3 = JobStatusResponse.from_dict(json.loads(resp_body.decode("utf-8")))
        self.assertEqual(status_resp3.status, JobExecutionStatus.FAILED.value)
        self.assertIsNotNone(status_resp3.error)
        self.assertEqual(status_resp3.error.error_code, "execution_failed")
        self.assertIn("dimensions", status_resp3.error.message.lower())
        self.assertIsNone(status_resp3.result_digest)
        self.assertIsNone(status_resp3.result_bytes)

        code, _, _ = faulty_gateway.handle_http_request("GET", f"/mc/v1/jobs/{handle3}/result", req_headers3, b"")
        self.assertEqual(code, 409)

    def test_corrupted_output_and_wrong_dimensions_modal_and_beam_simulation(self):
        """Verify corrupted output and wrong dimensions fail in Modal and Beam adapter simulation paths."""
        # 1. Modal simulate_function_call with corrupted_output
        modal_worker = FluxWorker(provider="modal")
        modal_adapter = ModalAppAdapter(
            token_id="modal-key-abc",
            token_secret="modal-secret-xyz",
            worker=modal_worker,
            limits=self.limits,
        )
        modal_worker.simulated_fault = "corrupted_output"
        job_meta_corrupt = JobRequestMetadata.from_dict({
            **self.job_meta_dict,
            "job_id": "job-modal-sim-corrupt",
            "request_digest": compute_request_digest(
                JobRequestMetadata.from_dict({**self.job_meta_dict, "job_id": "job-modal-sim-corrupt"})
            ),
        })
        native_id = modal_adapter.simulate_function_call(
            job_meta=job_meta_corrupt,
            img_bytes=self.tiny_image_bytes,
            hint_bytes=self.tiny_hint_bytes,
        )
        rec = modal_adapter.handle_registry.get_by_native_handle(native_id)
        self.assertIsNotNone(rec)
        self.assertEqual(rec.status, JobExecutionStatus.FAILED)
        self.assertIsNotNone(rec.error)
        self.assertEqual(rec.error.error_code, "execution_failed")
        self.assertIsNone(rec.result_data)
        self.assertIsNone(rec.result_digest)

        code, _, _ = modal_adapter.handle_request("GET", f"/mc/v1/jobs/{rec.app_handle}/result", self.modal_auth, b"")
        self.assertEqual(code, 409)

        # 2. Modal simulate_function_call with wrong_dimensions
        modal_worker.simulated_fault = "wrong_dimensions"
        job_meta_wrong_dim = JobRequestMetadata.from_dict({
            **self.job_meta_dict,
            "job_id": "job-modal-sim-dim",
            "request_digest": compute_request_digest(
                JobRequestMetadata.from_dict({**self.job_meta_dict, "job_id": "job-modal-sim-dim"})
            ),
        })
        native_id_dim = modal_adapter.simulate_function_call(
            job_meta=job_meta_wrong_dim,
            img_bytes=self.tiny_image_bytes,
            hint_bytes=self.tiny_hint_bytes,
        )
        rec_dim = modal_adapter.handle_registry.get_by_native_handle(native_id_dim)
        self.assertIsNotNone(rec_dim)
        self.assertEqual(rec_dim.status, JobExecutionStatus.FAILED)
        self.assertIsNotNone(rec_dim.error)
        self.assertEqual(rec_dim.error.error_code, "execution_failed")
        self.assertIn("dimensions", rec_dim.error.message.lower())
        self.assertIsNone(rec_dim.result_data)

        code, _, _ = modal_adapter.handle_request("GET", f"/mc/v1/jobs/{rec_dim.app_handle}/result", self.modal_auth, b"")
        self.assertEqual(code, 409)

        # 3. Beam simulate_task_enqueue with corrupted_output
        beam_worker = FluxWorker(provider="beam")
        beam_adapter = BeamAppAdapter(
            bearer_token="beam-token-123",
            worker=beam_worker,
            limits=self.limits,
        )
        beam_worker.simulated_fault = "corrupted_output"
        job_meta_beam_corrupt = JobRequestMetadata.from_dict({
            **self.job_meta_dict,
            "job_id": "job-beam-sim-corrupt",
            "request_digest": compute_request_digest(
                JobRequestMetadata.from_dict({**self.job_meta_dict, "job_id": "job-beam-sim-corrupt"})
            ),
        })
        beam_task_id = beam_adapter.simulate_task_enqueue(
            job_meta=job_meta_beam_corrupt,
            img_bytes=self.tiny_image_bytes,
            hint_bytes=self.tiny_hint_bytes,
        )
        beam_rec = beam_adapter.handle_registry.get_by_native_handle(beam_task_id)
        self.assertIsNotNone(beam_rec)
        self.assertEqual(beam_rec.status, JobExecutionStatus.FAILED)
        self.assertIsNotNone(beam_rec.error)
        self.assertEqual(beam_rec.error.error_code, "execution_failed")
        self.assertIsNone(beam_rec.result_data)

        code, _, _ = beam_adapter.handle_request("GET", f"/mc/v1/jobs/{beam_rec.app_handle}/result", self.beam_auth, b"")
        self.assertEqual(code, 409)

        # 4. Beam simulate_task_enqueue with wrong_dimensions
        beam_worker.simulated_fault = "wrong_dimensions"
        job_meta_beam_dim = JobRequestMetadata.from_dict({
            **self.job_meta_dict,
            "job_id": "job-beam-sim-dim",
            "request_digest": compute_request_digest(
                JobRequestMetadata.from_dict({**self.job_meta_dict, "job_id": "job-beam-sim-dim"})
            ),
        })
        beam_task_dim_id = beam_adapter.simulate_task_enqueue(
            job_meta=job_meta_beam_dim,
            img_bytes=self.tiny_image_bytes,
            hint_bytes=self.tiny_hint_bytes,
        )
        beam_dim_rec = beam_adapter.handle_registry.get_by_native_handle(beam_task_dim_id)
        self.assertIsNotNone(beam_dim_rec)
        self.assertEqual(beam_dim_rec.status, JobExecutionStatus.FAILED)
        self.assertIsNotNone(beam_dim_rec.error)
        self.assertEqual(beam_dim_rec.error.error_code, "execution_failed")
        self.assertIn("dimensions", beam_dim_rec.error.message.lower())
        self.assertIsNone(beam_dim_rec.result_data)

        code, _, _ = beam_adapter.handle_request("GET", f"/mc/v1/jobs/{beam_dim_rec.app_handle}/result", self.beam_auth, b"")
        self.assertEqual(code, 409)

    def test_valid_output_modal_and_beam_simulation(self):
        """Verify conforming worker output succeeds and is fully retrievable across Modal and Beam."""
        # 1. Modal valid simulation
        modal_worker = FluxWorker(provider="modal")
        modal_adapter = ModalAppAdapter(
            token_id="modal-key-abc",
            token_secret="modal-secret-xyz",
            worker=modal_worker,
            limits=self.limits,
        )
        job_meta_valid = JobRequestMetadata.from_dict({
            **self.job_meta_dict,
            "job_id": "job-modal-sim-valid",
            "request_digest": compute_request_digest(
                JobRequestMetadata.from_dict({**self.job_meta_dict, "job_id": "job-modal-sim-valid"})
            ),
        })
        native_id = modal_adapter.simulate_function_call(
            job_meta=job_meta_valid,
            img_bytes=self.tiny_image_bytes,
            hint_bytes=self.tiny_hint_bytes,
        )
        rec = modal_adapter.handle_registry.get_by_native_handle(native_id)
        self.assertIsNotNone(rec)
        self.assertEqual(rec.status, JobExecutionStatus.COMPLETED)
        self.assertIsNone(rec.error)
        self.assertIsNotNone(rec.result_data)
        self.assertIsNotNone(rec.result_digest)

        code, headers, body = modal_adapter.handle_request("GET", f"/mc/v1/jobs/{rec.app_handle}/result", self.modal_auth, b"")
        self.assertEqual(code, 200)
        self.assertEqual(headers["Content-Type"], "image/png")
        self.assertEqual(headers["X-MC-Result-Digest"], rec.result_digest)
        validate_png_header(body, job_meta_valid.width, job_meta_valid.height, 2)

        # 2. Beam valid simulation
        beam_worker = FluxWorker(provider="beam")
        beam_adapter = BeamAppAdapter(
            bearer_token="beam-token-123",
            worker=beam_worker,
            limits=self.limits,
        )
        job_meta_beam_valid = JobRequestMetadata.from_dict({
            **self.job_meta_dict,
            "job_id": "job-beam-sim-valid",
            "request_digest": compute_request_digest(
                JobRequestMetadata.from_dict({**self.job_meta_dict, "job_id": "job-beam-sim-valid"})
            ),
        })
        beam_task_id = beam_adapter.simulate_task_enqueue(
            job_meta=job_meta_beam_valid,
            img_bytes=self.tiny_image_bytes,
            hint_bytes=self.tiny_hint_bytes,
        )
        beam_rec = beam_adapter.handle_registry.get_by_native_handle(beam_task_id)
        self.assertIsNotNone(beam_rec)
        self.assertEqual(beam_rec.status, JobExecutionStatus.COMPLETED)
        self.assertIsNone(beam_rec.error)
        self.assertIsNotNone(beam_rec.result_data)

        code, headers, body = beam_adapter.handle_request("GET", f"/mc/v1/jobs/{beam_rec.app_handle}/result", self.beam_auth, b"")
        self.assertEqual(code, 200)
        self.assertEqual(headers["Content-Type"], "image/png")
        self.assertEqual(headers["X-MC-Result-Digest"], beam_rec.result_digest)
        validate_png_header(body, job_meta_beam_valid.width, job_meta_beam_valid.height, 2)


    def test_result_retry_by_same_handle(self):
        """Test downloading result multiple times returns identical bytes without duplicate inference."""
        ct_header, multipart_body = build_multipart_body(
            self.job_meta_json, self.tiny_image_bytes, self.tiny_hint_bytes
        )
        req_headers = dict(self.modal_auth)
        req_headers["Content-Type"] = ct_header

        # Submit job
        _, _, body = self.modal_adapter.handle_request("POST", "/mc/v1/jobs", req_headers, multipart_body)
        accepted = JobAcceptedResponse.from_dict(json.loads(body.decode("utf-8")))
        handle = accepted.handle

        # Initial inference count
        initial_inf_count = self.modal_adapter.worker.inference_count

        # First result download
        code1, headers1, body1 = self.modal_adapter.handle_request(
            "GET", f"/mc/v1/jobs/{handle}/result", self.modal_auth, b""
        )
        self.assertEqual(code1, 200)

        # Second result download
        code2, headers2, body2 = self.modal_adapter.handle_request(
            "GET", f"/mc/v1/jobs/{handle}/result", self.modal_auth, b""
        )
        self.assertEqual(code2, 200)

        # Third result download
        code3, headers3, body3 = self.modal_adapter.handle_request(
            "GET", f"/mc/v1/jobs/{handle}/result", self.modal_auth, b""
        )
        self.assertEqual(code3, 200)

        # Byte content and digest headers must be identical
        self.assertEqual(body1, body2)
        self.assertEqual(body2, body3)
        self.assertEqual(headers1["X-MC-Result-Digest"], headers2["X-MC-Result-Digest"])

        # No new inference was run on result retries
        self.assertEqual(self.modal_adapter.worker.inference_count, initial_inf_count)

    def test_ambiguous_submit_and_no_blind_retry(self):
        """Verify ambiguous submission handling and existing handle reuse on repeat submit."""
        ct_header, multipart_body = build_multipart_body(
            self.job_meta_json, self.tiny_image_bytes, self.tiny_hint_bytes
        )
        req_headers = dict(self.modal_auth)
        req_headers["Content-Type"] = ct_header

        # First submission
        _, _, body1 = self.modal_adapter.handle_request("POST", "/mc/v1/jobs", req_headers, multipart_body)
        accepted1 = JobAcceptedResponse.from_dict(json.loads(body1.decode("utf-8")))

        # Repeated submission with the same job_id and attempt_id
        _, _, body2 = self.modal_adapter.handle_request("POST", "/mc/v1/jobs", req_headers, multipart_body)
        accepted2 = JobAcceptedResponse.from_dict(json.loads(body2.decode("utf-8")))

        # Must return the SAME application handle rather than creating a second job
        self.assertEqual(accepted1.handle, accepted2.handle)
        self.assertEqual(accepted1.job_id, accepted2.job_id)
        self.assertEqual(accepted1.attempt_id, accepted2.attempt_id)
        self.assertEqual(accepted1.status, JobExecutionStatus.PENDING.value)
        self.assertEqual(accepted2.status, JobExecutionStatus.PENDING.value)

        # Inference count must remain exactly 1 without re-executing or billing
        self.assertEqual(self.modal_adapter.worker.inference_count, 1)

    def test_duplicate_submission_does_not_reexecute_or_bill(self):
        """Verify duplicate (job_id, attempt_id) submissions return existing handle and inference_count stays 1."""
        worker = FluxWorker(provider="beam", limits=self.limits)
        adapter = BeamAppAdapter(bearer_token="beam-token-123", worker=worker, limits=self.limits)
        auth = {"Authorization": "Bearer beam-token-123"}

        ct_header, multipart_body = build_multipart_body(
            self.job_meta_json, self.tiny_image_bytes, self.tiny_hint_bytes
        )
        headers = dict(auth)
        headers["Content-Type"] = ct_header

        self.assertEqual(worker.inference_count, 0)

        # 1. Initial submission
        code1, _, body1 = adapter.handle_request("POST", "/mc/v1/jobs", headers, multipart_body)
        self.assertEqual(code1, 202)
        accepted1 = JobAcceptedResponse.from_dict(json.loads(body1.decode("utf-8")))
        self.assertEqual(accepted1.status, "pending")
        self.assertEqual(worker.inference_count, 1)

        # 2. Duplicate submission with exact same job_id & attempt_id
        code2, _, body2 = adapter.handle_request("POST", "/mc/v1/jobs", headers, multipart_body)
        self.assertEqual(code2, 202)
        accepted2 = JobAcceptedResponse.from_dict(json.loads(body2.decode("utf-8")))
        self.assertEqual(accepted2.handle, accepted1.handle)
        self.assertEqual(accepted2.status, "pending")
        # Critical invariant: inference_count MUST stay 1
        self.assertEqual(worker.inference_count, 1)

    def test_execute_worker_job_honors_cancel_requested_and_terminal_states(self):
        """execute_worker_job must honor cancel_requested and terminal states and never run or overwrite them."""
        from deploy.cloud.common.api import execute_worker_job

        registry = InMemoryHandleRegistry()
        worker = FluxWorker(provider="modal", limits=self.limits)

        # 1. Job with cancel_requested = True before execution
        record1 = registry.register_job(
            job_meta=self.job_meta,
            native_handle="native-cancel-test-1",
            app_handle="handle-cancel-1",
            provider="modal",
        )
        registry.request_cancel("handle-cancel-1")
        self.assertTrue(record1.cancel_requested)

        ran1 = execute_worker_job(
            handle_registry=registry,
            worker=worker,
            record=record1,
            img_bytes=self.tiny_image_bytes,
            hint_bytes=self.tiny_hint_bytes,
            limits=self.limits,
            auto_execute=True,
        )
        self.assertFalse(ran1)
        self.assertEqual(worker.inference_count, 0)
        self.assertEqual(record1.status, JobExecutionStatus.PENDING)

        # 2. Job already completed
        record2 = JobRecord(
            app_handle="handle-term-comp",
            native_handle="native-term-comp",
            provider="modal",
            job_id="job-comp-1",
            attempt_id="att-comp-1",
            request_digest=self.job_meta.request_digest,
            recipe=self.job_meta.recipe,
            width=16,
            height=16,
            seed=42,
            steps=4,
            guidance_scaled=350,
            status=JobExecutionStatus.COMPLETED,
        )
        ran2 = execute_worker_job(
            handle_registry=registry,
            worker=worker,
            record=record2,
            img_bytes=self.tiny_image_bytes,
            hint_bytes=self.tiny_hint_bytes,
            limits=self.limits,
            auto_execute=True,
        )
        self.assertFalse(ran2)
        self.assertEqual(worker.inference_count, 0)
        self.assertEqual(record2.status, JobExecutionStatus.COMPLETED)

        # 3. Job already failed
        record3 = JobRecord(
            app_handle="handle-term-fail",
            native_handle="native-term-fail",
            provider="modal",
            job_id="job-fail-1",
            attempt_id="att-fail-1",
            request_digest=self.job_meta.request_digest,
            recipe=self.job_meta.recipe,
            width=16,
            height=16,
            seed=42,
            steps=4,
            guidance_scaled=350,
            status=JobExecutionStatus.FAILED,
        )
        ran3 = execute_worker_job(
            handle_registry=registry,
            worker=worker,
            record=record3,
            img_bytes=self.tiny_image_bytes,
            hint_bytes=self.tiny_hint_bytes,
            limits=self.limits,
            auto_execute=True,
        )
        self.assertFalse(ran3)
        self.assertEqual(worker.inference_count, 0)
        self.assertEqual(record3.status, JobExecutionStatus.FAILED)

    def test_presigned_url_redaction_beam_and_gcs_x_goog(self):
        """Verify Beam, S3, and GCS X-Goog signed URLs are completely redacted in text and dictionaries."""
        # 1. GCS X-Goog signed URL
        gcs_signed_url = (
            "https://storage.googleapis.com/manga-bucket/output/clean.png?"
            "X-Goog-Algorithm=GOOG4-RSA-SHA256&"
            "X-Goog-Credential=service-account%40proj.iam.gserviceaccount.com&"
            "X-Goog-Date=20260921T120000Z&"
            "X-Goog-Expires=3600&"
            "X-Goog-SignedHeaders=host&"
            "X-Goog-Signature=abcdef1234567890secretgoogsignature"
        )
        redacted_gcs = redact_text(f"Fetched artifact from {gcs_signed_url}")
        self.assertNotIn("secretgoogsignature", redacted_gcs)
        self.assertNotIn("service-account", redacted_gcs)
        self.assertIn(REDACTED_PRESIGNED_URL, redacted_gcs)

        # 2. GCS GoogleAccessId style signed URL
        gcs_legacy_url = "https://storage.googleapis.com/bucket/out.png?GoogleAccessId=acc@proj.iam.gserviceaccount.com&Signature=legacysecret123"
        redacted_legacy = redact_text(f"URL: {gcs_legacy_url}")
        self.assertNotIn("legacysecret123", redacted_legacy)
        self.assertIn(REDACTED_PRESIGNED_URL, redacted_legacy)

        # 3. S3 / Beam presigned URL
        presigned_link = "https://storage.beam.cloud/v2/outputs/crop123.png?X-Amz-Signature=secret999&X-Amz-Credential=AKIAEXAMPLE"
        redacted_s3 = redact_text(f"Task completed. Download at {presigned_link}")
        self.assertNotIn("secret999", redacted_s3)
        self.assertIn(REDACTED_PRESIGNED_URL, redacted_s3)

        # 4. Dictionary redaction
        d = {
            "gcs_signed_url": gcs_signed_url,
            "nested": {"url_text": f"GCS link: {gcs_signed_url}"},
        }
        redacted_d = redact_dict(d)
        self.assertEqual(redacted_d["gcs_signed_url"], REDACTED_SECRET)
        self.assertNotIn("secretgoogsignature", str(redacted_d))

    def test_presigned_url_redaction_beam(self):
        """Verify Beam presigned storage URLs are redacted from logs and never returned in API responses."""
        presigned_link = "https://storage.beam.cloud/v2/outputs/crop123.png?X-Amz-Signature=secret999&X-Amz-Credential=AKIAEXAMPLE"

        # Native task enqueue simulation with internal storage URL
        native_task_id = self.beam_adapter.simulate_task_enqueue(
            job_meta=self.job_meta,
            img_bytes=self.tiny_image_bytes,
            hint_bytes=self.tiny_hint_bytes,
            presigned_storage_url=presigned_link,
        )

        record = self.beam_adapter.handle_registry.get_by_native_handle(native_task_id)
        self.assertIsNotNone(record)
        handle = record.app_handle

        # 1. API status polling response does NOT leak presigned URL
        _, _, status_raw = self.beam_adapter.handle_request("GET", f"/mc/v1/jobs/{handle}", self.beam_auth, b"")
        status_text = status_raw.decode("utf-8")
        self.assertNotIn("secret999", status_text)
        self.assertNotIn("storage.beam.cloud", status_text)

        # 2. Serialized snapshot dict does NOT contain the URL
        rec_dict = record.to_dict()
        self.assertNotIn("internal_storage_ref", rec_dict)

        # 3. Text redaction utility replaces presigned URL
        sanitized_log = redact_text(f"Task completed. Download at {presigned_link}")
        self.assertNotIn("secret999", sanitized_log)
        self.assertIn(REDACTED_PRESIGNED_URL, sanitized_log)

    def test_lazy_sdk_imports_and_unsupported_live_actions(self):
        """Verify SDK imports are lazy/optional and live actions raise explicit refusal."""
        from deploy.cloud.modal.app import _MODAL_SDK_AVAILABLE, ModalAppAdapter
        from deploy.cloud.beam.app import _BEAM_SDK_AVAILABLE, BeamAppAdapter

        # Calling live deploy without platform authorization raises NotImplementedError
        with self.assertRaises(NotImplementedError):
            ModalAppAdapter.deploy_live()

        with self.assertRaises(NotImplementedError):
            BeamAppAdapter.deploy_live()

    def test_build_modal_app_fails_closed_even_with_mocked_sdk(self):
        """Verify build_modal_app fails closed before resource definitions even if Modal SDK is mocked available."""
        mock_modal = MagicMock()
        with patch("deploy.cloud.modal.app._MODAL_SDK_AVAILABLE", True), patch(
            "deploy.cloud.modal.app.modal", mock_modal
        ):
            with self.assertRaises(NotImplementedError):
                build_modal_app()

            # Verify fail-closed behavior before any resource definitions
            mock_modal.App.assert_not_called()
            mock_modal.Volume.from_name.assert_not_called()
            mock_modal.Image.debian_slim.assert_not_called()

        # When Modal SDK is not available, building app raises RuntimeError
        with patch("deploy.cloud.modal.app._MODAL_SDK_AVAILABLE", False), patch(
            "deploy.cloud.modal.app.modal", None
        ):
            with self.assertRaises(RuntimeError):
                build_modal_app()

    def test_build_beam_app_fails_closed_even_with_mocked_sdk(self):
        """Verify build_beam_app fails closed before resource definitions even if Beam SDK is mocked available."""
        mock_beam = MagicMock()
        with patch("deploy.cloud.beam.app._BEAM_SDK_AVAILABLE", True), patch(
            "deploy.cloud.beam.app.beam", mock_beam
        ):
            with self.assertRaises(NotImplementedError):
                build_beam_app()

            # Verify fail-closed behavior before any resource constructor calls
            mock_beam.Volume.assert_not_called()
            mock_beam.App.assert_not_called()
            mock_beam.task_queue.assert_not_called()
            mock_beam.endpoint.assert_not_called()

        # When Beam SDK is not available, building app raises RuntimeError
        with patch("deploy.cloud.beam.app._BEAM_SDK_AVAILABLE", False), patch(
            "deploy.cloud.beam.app.beam", None
        ):
            with self.assertRaises(RuntimeError):
                build_beam_app()

    def test_factory_requires_secrets_and_adapter_no_default_credentials(self):
        """Verify production gateway creation requires secrets and adapters have no default hardcoded secrets."""
        # Unconfigured Modal adapter rejects unauthenticated requests
        unconfigured_modal = ModalAppAdapter()
        code, _, _ = unconfigured_modal.handle_request("GET", "/health", {}, b"")
        self.assertEqual(code, 401)

        # Unconfigured Beam adapter rejects unauthenticated requests
        unconfigured_beam = BeamAppAdapter()
        code, _, _ = unconfigured_beam.handle_request("GET", "/health", {}, b"")
        self.assertEqual(code, 401)

        # Gateway factories require secrets or auth_validator
        with self.assertRaises(ValueError):
            create_modal_gateway()

        with self.assertRaises(ValueError):
            create_beam_gateway()

        # Gateway factories succeed when secrets are provided
        gw_modal = create_modal_gateway(token_id="k", token_secret="s")
        self.assertIsInstance(gw_modal, CloudGateway)

        gw_beam = create_beam_gateway(bearer_token="b")
        self.assertIsInstance(gw_beam, CloudGateway)

    def test_in_memory_handle_mapping_records(self):
        """Verify handle registry mapping, query methods, and state transitions."""
        registry = InMemoryHandleRegistry()
        record = registry.register_job(
            job_meta=self.job_meta,
            native_handle="native-fc-12345",
            app_handle="handle-test-modal-999",
            provider="modal",
        )
        self.assertEqual(record.app_handle, "handle-test-modal-999")
        self.assertEqual(record.status, JobExecutionStatus.PENDING)

        # Lookup by app handle
        by_app = registry.get_by_app_handle("handle-test-modal-999")
        self.assertEqual(by_app, record)

        # Lookup by native handle
        by_native = registry.get_by_native_handle("native-fc-12345")
        self.assertEqual(by_native, record)

        # Lookup by job & attempt ID
        by_id = registry.get_by_job_attempt(self.job_meta.job_id, self.job_meta.attempt_id)
        self.assertEqual(by_id, record)

        # Update status to RUNNING
        registry.update_status("handle-test-modal-999", JobExecutionStatus.RUNNING)
        self.assertEqual(record.status, JobExecutionStatus.RUNNING)
        self.assertIsNotNone(record.started_at)

        # Complete with result
        registry.store_result(
            app_handle="handle-test-modal-999",
            result_data=self.tiny_image_bytes,
            result_digest=self.job_meta.image_sha256,
            reported_cost_usd=0.0012,
        )
        self.assertEqual(record.status, JobExecutionStatus.COMPLETED)
        self.assertEqual(record.result_bytes, len(self.tiny_image_bytes))

    def test_manifest_verification(self):
        """Verify model manifest file checks, hash verification, and missing file detection."""
        with tempfile.TemporaryDirectory() as tmp_dir:
            tmp_path = Path(tmp_dir)

            # Create dummy model file
            model_file = tmp_path / "model.safetensors"
            model_content = b"DUMMY_MODEL_WEIGHT_BYTES_1234567890"
            model_file.write_bytes(model_content)

            import hashlib
            model_sha = hashlib.sha256(model_content).hexdigest()

            manifest = ModelManifest(
                model_id="test-model",
                model_revision=REVISION_TEST_FLUX,
                recipe_id=RECIPE_TEST_SDNQ,
                preprocessing_version="1.0.0",
                files=[
                    ManifestFileRecord(
                        relative_path="model.safetensors",
                        sha256=model_sha,
                        size_bytes=len(model_content),
                    ),
                    ManifestFileRecord(
                        relative_path="config.json",
                        sha256="0000000000000000000000000000000000000000000000000000000000000000",
                        size_bytes=100,
                    ),
                ],
                total_bytes=len(model_content) + 100,
                native_mask_conditioning=False,
            )

            # 1. Verification with missing config.json -> invalid
            report = verify_model_manifest(tmp_path, manifest)
            self.assertFalse(report["valid"])
            self.assertIn("config.json", report["missing_files"])
            self.assertIn("model.safetensors", report["verified_files"])

            # 2. Add config.json with matching hash -> valid
            config_file = tmp_path / "config.json"
            config_content = b"X" * 100
            config_sha = hashlib.sha256(config_content).hexdigest()
            config_file.write_bytes(config_content)

            complete_manifest = ModelManifest(
                model_id="test-model",
                model_revision=REVISION_TEST_FLUX,
                recipe_id=RECIPE_TEST_SDNQ,
                preprocessing_version="1.0.0",
                files=[
                    ManifestFileRecord("model.safetensors", model_sha, len(model_content)),
                    ManifestFileRecord("config.json", config_sha, len(config_content)),
                ],
                total_bytes=len(model_content) + len(config_content),
                native_mask_conditioning=False,
            )
            report2 = verify_model_manifest(tmp_path, complete_manifest)
            self.assertTrue(report2["valid"])
            self.assertEqual(len(report2["missing_files"]), 0)
            self.assertEqual(len(report2["corrupted_files"]), 0)

    def test_modal_simulate_function_call_duplicate_returns_same_native_handle(self):
        """Modal simulate_function_call for existing (job_id, attempt_id) returns existing native handle."""
        modal_adapter = ModalAppAdapter(
            token_id="modal-key-abc",
            token_secret="modal-secret-xyz",
            limits=self.limits,
        )
        native_id1 = modal_adapter.simulate_function_call(
            job_meta=self.job_meta,
            img_bytes=self.tiny_image_bytes,
            hint_bytes=self.tiny_hint_bytes,
        )
        native_id2 = modal_adapter.simulate_function_call(
            job_meta=self.job_meta,
            img_bytes=self.tiny_image_bytes,
            hint_bytes=self.tiny_hint_bytes,
        )
        self.assertEqual(native_id1, native_id2)
        rec = modal_adapter.handle_registry.get_by_native_handle(native_id1)
        self.assertIsNotNone(rec)
        self.assertEqual(rec.native_handle, native_id1)

    def test_beam_simulate_task_enqueue_duplicate_returns_same_native_handle(self):
        """Beam simulate_task_enqueue for existing (job_id, attempt_id) returns existing native handle."""
        beam_adapter = BeamAppAdapter(
            bearer_token="beam-token-123",
            limits=self.limits,
        )
        task_id1 = beam_adapter.simulate_task_enqueue(
            job_meta=self.job_meta,
            img_bytes=self.tiny_image_bytes,
            hint_bytes=self.tiny_hint_bytes,
        )
        task_id2 = beam_adapter.simulate_task_enqueue(
            job_meta=self.job_meta,
            img_bytes=self.tiny_image_bytes,
            hint_bytes=self.tiny_hint_bytes,
        )
        self.assertEqual(task_id1, task_id2)
        rec = beam_adapter.handle_registry.get_by_native_handle(task_id1)
        self.assertIsNotNone(rec)
        self.assertEqual(rec.native_handle, task_id1)


if __name__ == "__main__":
    unittest.main()
