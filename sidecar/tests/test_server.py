"""Unit tests for the sidecar HTTP server and request handler."""

from __future__ import annotations

import io
import json
import os
import sys
import threading
import time
import unittest
import urllib.error
import urllib.request
from typing import Any, Dict, Optional, Tuple

sys.path.insert(0, os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))

from manga_cleaner_sidecar.backend.base import BackendBase
from manga_cleaner_sidecar.protocol import (
    Basis,
    ErrorKind,
    ImageBuffer,
    ModelMetadata,
    SidecarError,
    State,
)
from manga_cleaner_sidecar.server import (
    SidecarRequestHandler,
    SidecarServerContext,
    create_server,
)


class DummyBackend(BackendBase):
    def __init__(self, backend_id: str = "mflux", weights_dir: Optional[str] = None) -> None:
        self._backend_id = backend_id
        self.weights_dir = weights_dir
        self.opened = False

    @property
    def backend_id(self) -> str:
        return self._backend_id

    def open(
        self,
        model_id: str,
        limits: Dict[str, Any],
        require: list[str],
    ) -> Dict[str, Any]:
        self.opened = True
        return {
            "backend": self.backend_id,
            "model": model_id,
            "applied": [],
            "basis": Basis.DECLARED.value,
        }

    def render(
        self,
        image: ImageBuffer,
        hint: Optional[ImageBuffer],
        prompt: str,
        steps: int,
        seed: int,
        guidance: float,
        deadline_ms: int,
    ) -> Tuple[ImageBuffer, int]:
        return image, 10

    def release(self) -> None:
        self.opened = False

    def get_model_metadata(self) -> ModelMetadata:
        return ModelMetadata(
            model_id="test-model",
            family="test",
            parameter_count="4B",
            quantization="4bit",
            context_window=512,
            supports_cpu_offload=False,
            supports_memory_fraction=False,
            supports_vae_tiling=False,
            supports_te_eviction=False,
        )

    def get_weights_bytes(self) -> Optional[int]:
        return 1000

    def get_working_set_bytes(self) -> Optional[int]:
        return 2000

    def get_basis(self) -> Basis:
        return Basis.DECLARED

    def get_applied_guards(self) -> list[str]:
        return []


class MockHandler:
    """Mock handler exposing SidecarRequestHandler helper methods for unit testing."""

    def __init__(
        self,
        headers: Dict[str, str],
        body_bytes: bytes = b"",
        token: str = "secret-token",
    ) -> None:
        self.headers = headers
        self.rfile = io.BytesIO(body_bytes)
        self.ctx = SidecarServerContext(token=token, backend_name="mflux")
        self.ctx.backend = DummyBackend("mflux")

    def _authenticate(self) -> None:
        SidecarRequestHandler._authenticate(self)  # type: ignore[arg-type]

    def _read_json_body(self) -> Dict[str, Any]:
        return SidecarRequestHandler._read_json_body(self)  # type: ignore[arg-type]


class ServerAuthenticationTests(unittest.TestCase):
    def test_valid_token_authenticates_successfully(self) -> None:
        handler = MockHandler(
            headers={"x-mc-token": "secret-token-12345"},
            token="secret-token-12345",
        )
        # Should not raise
        handler._authenticate()

    def test_invalid_token_raises_401_unauthorized(self) -> None:
        handler = MockHandler(
            headers={"x-mc-token": "wrong-token"},
            token="secret-token-12345",
        )
        with self.assertRaises(SidecarError) as ctx:
            handler._authenticate()
        self.assertEqual(ctx.exception.status_code, 401)
        self.assertEqual(ctx.exception.kind, ErrorKind.UNAUTHORIZED.value)

    def test_missing_token_header_raises_401_unauthorized(self) -> None:
        handler = MockHandler(
            headers={},
            token="secret-token-12345",
        )
        with self.assertRaises(SidecarError) as ctx:
            handler._authenticate()
        self.assertEqual(ctx.exception.status_code, 401)
        self.assertEqual(ctx.exception.kind, ErrorKind.UNAUTHORIZED.value)

    def test_empty_server_token_allows_unauthenticated_access(self) -> None:
        handler = MockHandler(
            headers={},
            token="",
        )
        # Should not raise when no server token is configured
        handler._authenticate()


class ServerContentLengthTests(unittest.TestCase):
    def test_missing_content_length_returns_empty_dict(self) -> None:
        handler = MockHandler(headers={})
        self.assertEqual(handler._read_json_body(), {})

    def test_negative_content_length_returns_empty_dict_without_hanging(self) -> None:
        handler = MockHandler(
            headers={"Content-Length": "-1"},
            body_bytes=b"some trailing payload",
        )
        self.assertEqual(handler._read_json_body(), {})

    def test_zero_content_length_returns_empty_dict(self) -> None:
        handler = MockHandler(
            headers={"Content-Length": "0"},
            body_bytes=b"",
        )
        self.assertEqual(handler._read_json_body(), {})

    def test_valid_json_body_parsed_correctly(self) -> None:
        payload = b'{"key": "value", "count": 42}'
        handler = MockHandler(
            headers={"Content-Length": str(len(payload))},
            body_bytes=payload,
        )
        result = handler._read_json_body()
        self.assertEqual(result, {"key": "value", "count": 42})

    def test_malformed_json_body_raises_422_bad_request(self) -> None:
        payload = b"{not-valid-json}"
        handler = MockHandler(
            headers={"Content-Length": str(len(payload))},
            body_bytes=payload,
        )
        with self.assertRaises(SidecarError) as ctx:
            handler._read_json_body()
        self.assertEqual(ctx.exception.status_code, 422)
        self.assertEqual(ctx.exception.kind, ErrorKind.BAD_REQUEST.value)

    def test_excessive_content_length_raises_422_bad_request(self) -> None:
        handler = MockHandler(
            headers={"Content-Length": str(200 * 1024 * 1024)},
            body_bytes=b"",
        )
        with self.assertRaises(SidecarError) as ctx:
            handler._read_json_body()
        self.assertEqual(ctx.exception.status_code, 422)
        self.assertEqual(ctx.exception.kind, ErrorKind.BAD_REQUEST.value)


class ServerStateLockingTests(unittest.TestCase):
    def test_backend_switch_under_state_lock(self) -> None:
        ctx = SidecarServerContext(token="token", backend_name="mflux")
        ctx.backend = DummyBackend("mflux")

        # Simulate _handle_open lock behavior
        backend_name = "sdnq"
        with ctx.state_lock:
            if backend_name != ctx.backend.backend_id:
                ctx.backend = DummyBackend(backend_name)
            ctx.state = State.LOADING

        self.assertEqual(ctx.backend.backend_id, "sdnq")
        self.assertEqual(ctx.state, State.LOADING)


class ServerIntegrationHttpTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.token = "test-secret-token-xyz"
        cls.server, cls.port = create_server(host="127.0.0.1", port=0, token=cls.token)
        cls.server_thread = threading.Thread(target=cls.server.serve_forever, daemon=True)
        cls.server_thread.start()
        cls.base_url = f"http://127.0.0.1:{cls.port}"

    @classmethod
    def tearDownClass(cls) -> None:
        cls.server.shutdown()
        cls.server.server_close()
        cls.server_thread.join(timeout=2.0)

    def _request(
        self,
        path: str,
        method: str = "GET",
        headers: Optional[Dict[str, str]] = None,
        data: Optional[bytes] = None,
    ) -> Tuple[int, Dict[str, Any]]:
        req = urllib.request.Request(url=f"{self.base_url}{path}", method=method, data=data)
        if headers:
            for k, v in headers.items():
                req.add_header(k, v)
        try:
            with urllib.request.urlopen(req, timeout=5.0) as resp:
                raw = resp.read()
                return resp.status, json.loads(raw.decode("utf-8")) if raw else {}
        except urllib.error.HTTPError as exc:
            raw = exc.read()
            return exc.code, json.loads(raw.decode("utf-8")) if raw else {}

    def test_health_check_succeeds_without_auth(self) -> None:
        status, body = self._request("/v1/health")
        self.assertEqual(status, 200)
        self.assertIn("sidecar_version", body)
        self.assertIn("state", body)

    def test_post_open_unauthorized_without_token(self) -> None:
        status, body = self._request("/v1/open", method="POST")
        self.assertEqual(status, 401)
        self.assertEqual(body.get("error", {}).get("kind"), ErrorKind.UNAUTHORIZED.value)

    def test_post_open_unauthorized_with_wrong_token(self) -> None:
        status, body = self._request(
            "/v1/open",
            method="POST",
            headers={"x-mc-token": "wrong-token"},
        )
        self.assertEqual(status, 401)
        self.assertEqual(body.get("error", {}).get("kind"), ErrorKind.UNAUTHORIZED.value)

    def test_negative_content_length_does_not_hang(self) -> None:
        # Send raw HTTP request with Content-Length: -1
        import socket

        s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        s.settimeout(3.0)
        s.connect(("127.0.0.1", self.port))
        req = (
            f"POST /v1/release HTTP/1.1\r\n"
            f"Host: 127.0.0.1:{self.port}\r\n"
            f"x-mc-token: {self.token}\r\n"
            f"Content-Length: -1\r\n"
            f"Connection: close\r\n\r\n"
        ).encode("utf-8")
        s.sendall(req)
        response = b""
        while True:
            try:
                chunk = s.recv(4096)
                if not chunk:
                    break
                response += chunk
            except socket.timeout:
                break
        s.close()
        # Should return 200 OK without timing out
        self.assertIn(b"200 OK", response)


if __name__ == "__main__":
    unittest.main()
