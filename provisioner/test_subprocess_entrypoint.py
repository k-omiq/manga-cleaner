"""Subprocess execution tests for `python -m provisioner` module entrypoint.

Proves:
1. No fake success: CLI entrypoint never defaults to fake drivers; fails closed when real SDKs absent/unverified.
2. Bounded input: Rejection of oversized payloads (> 64 KiB) before memory bloat.
3. One-response protocol: Exactly ONE redacted JSON response envelope emitted on stdout.
4. Stderr/log redaction: No sensitive credentials leaked to stderr, argv, or response logs.
5. Provider/platform gating: Truthful failure on unverified platforms or unsupported providers.
"""

import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

from provisioner.protocol import (
    ERR_INVALID_PAYLOAD,
    ERR_PAYLOAD_TOO_LARGE,
    ERR_PLATFORM_GATED,
    ERR_PROVIDER_UNAVAILABLE,
    ERR_UNSUPPORTED_OP,
    ERR_UNSUPPORTED_PROVIDER,
    ERR_VALIDATION,
    HELPER_PROTOCOL_VERSION,
    MAX_REQUEST_BYTES,
    OP_APPLY,
    OP_INSPECT,
    OP_PLAN,
    PROVIDER_BEAM,
    PROVIDER_MODAL,
)

REPO_ROOT = Path(__file__).resolve().parent.parent


class TestSubprocessEntrypoint(unittest.TestCase):
    """Subprocess integration tests for python -m provisioner entrypoint."""

    def setUp(self):
        self.temp_dir = tempfile.TemporaryDirectory()
        self.journal_root = Path(self.temp_dir.name)

    def tearDown(self):
        self.temp_dir.cleanup()

    def _run_provisioner(
        self,
        input_data: str,
        extra_args: list = None,
    ) -> subprocess.CompletedProcess:
        """Run python3 -m provisioner in a subprocess with piped stdin/stdout/stderr."""
        cmd = [sys.executable, "-m", "provisioner"]
        if extra_args:
            cmd.extend(extra_args)
        else:
            cmd.extend(["--journal-root", str(self.journal_root)])

        return subprocess.run(
            cmd,
            input=input_data.encode("utf-8"),
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            cwd=str(REPO_ROOT),
        )

    # --- 1. No Fake Success In Production ---

    def test_subprocess_no_fake_success_modal_inspect(self):
        """Proves python -m provisioner does not return simulated fake success for Modal."""
        req = {
            "protocol_version": HELPER_PROTOCOL_VERSION,
            "request_id": "req-sub-modal-1",
            "op": OP_INSPECT,
            "provider": PROVIDER_MODAL,
            "params": {
                "credentials": {
                    "token_id": "ak-test-token-id",
                    "token_secret": "as-test-token-secret",
                }
            },
        }
        res = self._run_provisioner(json.dumps(req))
        self.assertEqual(res.returncode, 0)

        stdout_text = res.stdout.decode("utf-8").strip()
        data = json.loads(stdout_text)

        # Invariant: Must fail closed with real driver error, NOT return fake driver account
        self.assertFalse(data["success"])
        self.assertNotIn("acc_modal_simulated_user", stdout_text)
        self.assertIn(
            data["error"]["code"],
            (ERR_PROVIDER_UNAVAILABLE, ERR_PLATFORM_GATED),
        )

    def test_subprocess_no_fake_success_beam_inspect(self):
        """Proves python -m provisioner does not return simulated fake success for Beam."""
        req = {
            "protocol_version": HELPER_PROTOCOL_VERSION,
            "request_id": "req-sub-beam-1",
            "op": OP_INSPECT,
            "provider": PROVIDER_BEAM,
            "params": {
                "credentials": {
                    "token": "b9_secret_token_12345678",
                }
            },
        }
        res = self._run_provisioner(json.dumps(req))
        self.assertEqual(res.returncode, 0)

        stdout_text = res.stdout.decode("utf-8").strip()
        data = json.loads(stdout_text)

        # Invariant: Must fail closed with real driver error, NOT return fake driver account
        self.assertFalse(data["success"])
        self.assertNotIn("acc_beam_simulated_user", stdout_text)
        self.assertIn(
            data["error"]["code"],
            (ERR_PROVIDER_UNAVAILABLE, ERR_PLATFORM_GATED),
        )

    def test_subprocess_no_fake_success_modal_apply(self):
        """Proves apply operation fails closed without fake resource creation."""
        req = {
            "protocol_version": HELPER_PROTOCOL_VERSION,
            "request_id": "req-sub-modal-apply",
            "op": OP_APPLY,
            "provider": PROVIDER_MODAL,
            "params": {
                "installation_id": "inst-real-sub-1",
                "approved_plan_hash": "a" * 64,
                "credentials": {
                    "token_id": "ak-test-token",
                    "token_secret": "as-test-secret",
                },
            },
        }
        res = self._run_provisioner(json.dumps(req))
        self.assertEqual(res.returncode, 0)

        data = json.loads(res.stdout.decode("utf-8").strip())
        self.assertFalse(data["success"])
        self.assertIn(
            data["error"]["code"],
            (ERR_PROVIDER_UNAVAILABLE, ERR_PLATFORM_GATED),
        )

    # --- 2. Bounded Input Protocol ---

    def test_subprocess_bounded_input_payload_too_large(self):
        """Proves payloads exceeding 64 KiB are rejected with ERR_PAYLOAD_TOO_LARGE."""
        oversized_str = "x" * (MAX_REQUEST_BYTES + 4096)
        res = self._run_provisioner(oversized_str)
        self.assertEqual(res.returncode, 0)

        data = json.loads(res.stdout.decode("utf-8").strip())
        self.assertFalse(data["success"])
        self.assertEqual(data["error"]["code"], ERR_PAYLOAD_TOO_LARGE)

    def test_subprocess_empty_stdin_rejected(self):
        """Proves empty stdin is rejected with ERR_INVALID_PAYLOAD."""
        res = self._run_provisioner("")
        self.assertEqual(res.returncode, 0)

        data = json.loads(res.stdout.decode("utf-8").strip())
        self.assertFalse(data["success"])
        self.assertEqual(data["error"]["code"], ERR_INVALID_PAYLOAD)

    def test_subprocess_malformed_json_rejected(self):
        """Proves malformed JSON is rejected with ERR_INVALID_PAYLOAD."""
        res = self._run_provisioner("{not valid json: 123")
        self.assertEqual(res.returncode, 0)

        data = json.loads(res.stdout.decode("utf-8").strip())
        self.assertFalse(data["success"])
        self.assertEqual(data["error"]["code"], ERR_INVALID_PAYLOAD)

    # --- 3. One-Response Protocol ---

    def test_subprocess_emits_exactly_one_valid_json_response(self):
        """Proves stdout contains exactly one parseable JSON response and nothing else."""
        req = {
            "protocol_version": HELPER_PROTOCOL_VERSION,
            "request_id": "req-one-resp-test",
            "op": OP_INSPECT,
            "provider": PROVIDER_MODAL,
            "params": {
                "credentials": {
                    "token_id": "ak-test-id",
                    "token_secret": "as-test-secret",
                }
            },
        }
        res = self._run_provisioner(json.dumps(req))
        self.assertEqual(res.returncode, 0)

        stdout_text = res.stdout.decode("utf-8")
        # Ensure it parses as a single JSON object without trailing extraneous output
        parsed = json.loads(stdout_text.strip())
        self.assertEqual(parsed["protocol_version"], HELPER_PROTOCOL_VERSION)
        self.assertEqual(parsed["request_id"], "req-one-resp-test")
        self.assertIn("success", parsed)

    # --- 4. Stderr and Log Credential Redaction ---

    def test_subprocess_stderr_and_logs_never_leak_secrets(self):
        """Proves credentials supplied via stdin do not appear in stderr or stdout error responses."""
        secret_modal = "as-verysecretmodaltoken123456"
        secret_beam = "b9_verysecretbeamtoken789012"
        req = {
            "protocol_version": HELPER_PROTOCOL_VERSION,
            "request_id": "req-redact-test",
            "op": OP_INSPECT,
            "provider": PROVIDER_MODAL,
            "params": {
                "credentials": {
                    "token_id": "ak-test-modal",
                    "token_secret": secret_modal,
                    "beam_token": secret_beam,
                }
            },
        }
        res = self._run_provisioner(json.dumps(req))
        self.assertEqual(res.returncode, 0)

        stdout_text = res.stdout.decode("utf-8")
        stderr_text = res.stderr.decode("utf-8")

        # Invariant: raw secrets must NEVER appear in stdout or stderr
        self.assertNotIn(secret_modal, stdout_text)
        self.assertNotIn(secret_beam, stdout_text)
        self.assertNotIn(secret_modal, stderr_text)
        self.assertNotIn(secret_beam, stderr_text)

    # --- 5. Provider and Platform Gating ---

    def test_subprocess_unsupported_provider_rejected(self):
        """Proves unknown/unsupported provider fails closed with ERR_UNSUPPORTED_PROVIDER."""
        req = {
            "protocol_version": HELPER_PROTOCOL_VERSION,
            "request_id": "req-bad-prov",
            "op": OP_INSPECT,
            "provider": "unsupported_cloud",
            "params": {},
        }
        res = self._run_provisioner(json.dumps(req))
        self.assertEqual(res.returncode, 0)

        data = json.loads(res.stdout.decode("utf-8").strip())
        self.assertFalse(data["success"])
        self.assertEqual(data["error"]["code"], ERR_UNSUPPORTED_PROVIDER)

    def test_subprocess_unsupported_operation_rejected(self):
        """Proves unknown operation fails closed with ERR_UNSUPPORTED_OP."""
        req = {
            "protocol_version": HELPER_PROTOCOL_VERSION,
            "request_id": "req-bad-op",
            "op": "arbitrary_bad_op",
            "provider": PROVIDER_MODAL,
            "params": {},
        }
        res = self._run_provisioner(json.dumps(req))
        self.assertEqual(res.returncode, 0)

        data = json.loads(res.stdout.decode("utf-8").strip())
        self.assertFalse(data["success"])
        self.assertEqual(data["error"]["code"], ERR_UNSUPPORTED_OP)

    def test_subprocess_custom_journal_root_flag(self):
        """Proves --journal-root CLI argument is accepted without error."""
        custom_root = self.journal_root / "custom_sub"
        custom_root.mkdir(parents=True, exist_ok=True)
        req = {
            "protocol_version": HELPER_PROTOCOL_VERSION,
            "request_id": "req-custom-root",
            "op": OP_INSPECT,
            "provider": PROVIDER_MODAL,
            "params": {
                "credentials": {
                    "token_id": "ak-test",
                    "token_secret": "as-test",
                }
            },
        }
        res = self._run_provisioner(
            json.dumps(req),
            extra_args=["--journal-root", str(custom_root)],
        )
        self.assertEqual(res.returncode, 0)
        data = json.loads(res.stdout.decode("utf-8").strip())
        self.assertFalse(data["success"])
