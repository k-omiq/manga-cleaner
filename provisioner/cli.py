"""CLI entrypoint and stdin/stdout protocol runner for cloud provisioner helper.

Protocol guarantees:
- Accepts exactly one bounded versioned JSON request via stdin (bounded to 64 KiB).
- Emits exactly one redacted JSON response on stdout.
- Uses zero secret-bearing argv or environment logging.
- Fails closed with actionable errors when real SDKs are absent or unverified.
"""

from pathlib import Path
import sys
from typing import Optional

from provisioner.controller import ProvisioningController
from provisioner.protocol import (
    ERR_EXECUTION_FAILED,
    ERR_INVALID_PAYLOAD,
    ERR_PAYLOAD_TOO_LARGE,
    MAX_REQUEST_BYTES,
    make_error_response,
)


def _resolve_journal_root() -> Path:
    """Parse journal root from argv or env without secret logging."""
    argv = sys.argv[1:]
    for idx, arg in enumerate(argv):
        if arg in ("--journal-root", "-j") and idx + 1 < len(argv):
            return Path(argv[idx + 1]).resolve()
        if arg.startswith("--journal-root="):
            return Path(arg.split("=", 1)[1]).resolve()

    import os
    env_val = os.environ.get("MANGA_CLEANER_JOURNAL_ROOT")
    if env_val:
        return Path(env_val).resolve()

    # Default isolated path in user home directory
    return (Path.home() / ".manga-cleaner" / "cloud" / "journal").resolve()


def run_helper(stdin_stream=None, stdout_stream=None) -> int:
    """Execute one bounded JSON helper request and emit one redacted response."""
    stdin = stdin_stream if stdin_stream is not None else sys.stdin.buffer
    stdout = stdout_stream if stdout_stream is not None else sys.stdout

    # Read bounded input (up to MAX_REQUEST_BYTES + 1)
    try:
        raw_bytes = stdin.read(MAX_REQUEST_BYTES + 1)
    except Exception as e:
        err = make_error_response(
            request_id="req-unknown",
            code=ERR_INVALID_PAYLOAD,
            message=f"Failed to read request from stdin: {e}",
        )
        stdout.write(err.serialize() + "\n")
        stdout.flush()
        return 0

    if len(raw_bytes) > MAX_REQUEST_BYTES:
        err = make_error_response(
            request_id="req-unknown",
            code=ERR_PAYLOAD_TOO_LARGE,
            message=f"Request payload size ({len(raw_bytes)} bytes) exceeds maximum limit of {MAX_REQUEST_BYTES} bytes",
            actionable_guidance=f"Ensure JSON request payload is smaller than {MAX_REQUEST_BYTES} bytes (64 KiB).",
        )
        stdout.write(err.serialize() + "\n")
        stdout.flush()
        return 0

    if not raw_bytes or not raw_bytes.strip():
        err = make_error_response(
            request_id="req-unknown",
            code=ERR_INVALID_PAYLOAD,
            message="Empty request payload received on stdin",
            actionable_guidance="Provide a valid versioned JSON request envelope via stdin.",
        )
        stdout.write(err.serialize() + "\n")
        stdout.flush()
        return 0

    try:
        journal_root = _resolve_journal_root()
        controller = ProvisioningController(journal_root=journal_root)
        response_json = controller.handle_request(raw_bytes)
        stdout.write(response_json + "\n")
        stdout.flush()
        return 0
    except Exception as e:
        err = make_error_response(
            request_id="req-unknown",
            code=ERR_EXECUTION_FAILED,
            message=f"Fatal provisioner error: {e}",
        )
        stdout.write(err.serialize() + "\n")
        stdout.flush()
        return 0


def main() -> int:
    """Main CLI entrypoint."""
    return run_helper()


if __name__ == "__main__":
    sys.exit(main())
