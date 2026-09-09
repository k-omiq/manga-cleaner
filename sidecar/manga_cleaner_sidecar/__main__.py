"""Entry point for running Manga Cleaner Sidecar via python -m manga_cleaner_sidecar."""

from __future__ import annotations

import argparse
import os
import sys

from .self_test import run_self_test
from .server import create_server
from .watchdog import start_parent_watchdog


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Manga Cleaner Sidecar (FLUX.2 Klein 4B Inpainting Engine)",
    )
    parser.add_argument(
        "--self-test",
        action="store_true",
        help="Run the end-to-end self-test suite and exit.",
    )
    parser.add_argument(
        "--host",
        type=str,
        default=os.environ.get("MC_SIDECAR_HOST", "127.0.0.1"),
        help="Host address to bind (default: 127.0.0.1).",
    )
    parser.add_argument(
        "--port",
        type=int,
        default=int(os.environ.get("MC_SIDECAR_PORT", "8080")),
        help="TCP port to bind (default: 8080).",
    )
    parser.add_argument(
        "--token",
        type=str,
        default=os.environ.get("MC_SIDECAR_TOKEN"),
        help="Shared authentication secret (x-mc-token header).",
    )
    parser.add_argument(
        "--parent-pid",
        type=int,
        default=int(os.environ.get("MC_SIDECAR_PARENT_PID", "0")),
        help="PID of parent process for lifecycle watchdog.",
    )
    parser.add_argument(
        "--backend",
        type=str,
        default=os.environ.get("MC_SIDECAR_BACKEND", "mflux"),
        help="Inference backend engine (mflux, sdnq or sdcpp).",
    )
    parser.add_argument(
        "--weights-dir",
        type=str,
        default=os.environ.get("MC_SIDECAR_WEIGHTS_DIR"),
        help="Path to model weights directory on disk.",
    )

    args = parser.parse_args()

    if args.self_test:
        exit_code = run_self_test()
        sys.exit(exit_code)

    # Initialize parent process watchdog
    if args.parent_pid > 1:
        start_parent_watchdog(args.parent_pid)

    # Create and start loopback HTTP server
    server, bound_port = create_server(
        host=args.host,
        port=args.port,
        token=args.token,
        backend_name=args.backend,
        weights_dir=args.weights_dir,
    )

    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        server.server_close()


if __name__ == "__main__":
    main()
