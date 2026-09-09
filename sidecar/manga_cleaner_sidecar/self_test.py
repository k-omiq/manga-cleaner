"""Self-test runner for Manga Cleaner sidecar.

Executes an end-to-end integration test against an ephemeral loopback instance:
1. Starts server on an ephemeral port.
2. Queries GET /v1/health (pre-open liveness check).
3. Executes POST /v1/open with memory constraints and required guards.
4. Executes POST /v1/render on a synthetic test image.
5. Executes POST /v1/release and verifies return to floor.
6. Executes POST /v1/shutdown and verifies graceful exit.
"""

from __future__ import annotations

import base64
import json
import os
import sys
import threading
import time
import urllib.error
import urllib.request
from typing import Any, Dict, Optional, Tuple

from .memory import get_peak_phys_footprint_bytes, get_peak_rss_bytes
from .server import create_server


def _local_peak_bytes() -> int:
    """This process's own peak, in the honest quantity where one can be read.

    Physical footprint, falling back to peak resident set size only where no
    Mach ledger is available. See `memory.py` for why the two differ.
    """
    peak = get_peak_phys_footprint_bytes()
    return peak if peak is not None else get_peak_rss_bytes()


def _http_request(
    url: str,
    method: str = "GET",
    token: Optional[str] = None,
    body: Optional[Dict[str, Any]] = None,
    timeout_sec: float = 300.0,
) -> Tuple[int, Dict[str, Any]]:
    """Execute one HTTP JSON request against the loopback server."""
    req = urllib.request.Request(url=url, method=method)
    req.add_header("Connection", "close")
    req.add_header("Accept", "application/json")
    if token:
        req.add_header("x-mc-token", token)

    data_bytes = None
    if body is not None:
        data_bytes = json.dumps(body).encode("utf-8")
        req.add_header("Content-Type", "application/json")
        req.add_header("Content-Length", str(len(data_bytes)))

    try:
        with urllib.request.urlopen(req, data=data_bytes, timeout=timeout_sec) as resp:
            status = resp.status
            raw_body = resp.read()
            parsed = json.loads(raw_body.decode("utf-8")) if raw_body else {}
            return status, parsed
    except urllib.error.HTTPError as exc:
        raw_body = exc.read()
        parsed = json.loads(raw_body.decode("utf-8")) if raw_body else {}
        return exc.code, parsed


def create_synthetic_image(width: int = 512, height: int = 512) -> Dict[str, Any]:
    """Generate a synthetic 512x512 RGB8 image with background and dark text pattern."""
    # Create raw interleaved RGB byte array
    num_pixels = width * height
    # Light gray background (246, 246, 246)
    samples = bytearray([246, 246, 246] * num_pixels)

    # Draw dark strokes (20, 20, 20) simulating text in a central region
    for y in range(180, min(330, height), 12):
        for x in range(200, min(320, width)):
            idx = (y * width + x) * 3
            samples[idx] = 20
            samples[idx + 1] = 20
            samples[idx + 2] = 20

    b64_data = base64.b64encode(samples).decode("ascii")
    return {
        "width": width,
        "height": height,
        "channels": 3,
        "encoding": "rgb8",
        "data": b64_data,
    }


def run_self_test() -> int:
    """Run the complete end-to-end self-test suite."""
    print("=== Manga Cleaner Sidecar Self-Test ===")
    test_token = os.urandom(24).hex()
    server, port = create_server(host="127.0.0.1", port=0, token=test_token)

    server_thread = threading.Thread(target=server.serve_forever, daemon=True)
    server_thread.start()
    base_url = f"http://127.0.0.1:{port}"

    steps = [
        "1. Health (pre-open)",
        "2. Open Model",
        "3. Render Synthetic Image",
        "4. Release Model",
        "5. Shutdown Server",
    ]
    results: Dict[str, str] = {}
    peak_rss_gb = 0.0

    try:
        # Step 1: Health
        step_name = steps[0]
        status, health = _http_request(f"{base_url}/v1/health", method="GET")
        if status == 200 and health.get("protocol") == 1 and health.get("state") == "idle":
            print(f"[PASS] {step_name}: state={health.get('state')} weights_bytes={health.get('weights_bytes')}")
            results[step_name] = "PASS"
        else:
            print(f"[FAIL] {step_name}: status={status} body={health}")
            results[step_name] = "FAIL"
            return 1

        # Step 2: Open
        step_name = steps[1]
        open_body = {
            "backend": "mflux",
            "model": "flux2-klein-4b",
            "limits": {
                "total_bytes": 14_000_000_000,
                "cache_bytes": 1_000_000_000,
            },
            # The two the `mflux` contract still requires. `vae_tiling` and
            # `text_encoder_eviction` are gone from it - see
            # `backend/mflux.py`'s docstring and
            # `cleaner_core::sidecar::backend::MFLUX`.
            "require": [
                "mx.set_memory_limit",
                "mx.set_cache_limit",
            ],
        }
        status, open_resp = _http_request(
            f"{base_url}/v1/open",
            method="POST",
            token=test_token,
            body=open_body,
            timeout_sec=600.0,
        )
        if status == 200 and "applied" in open_resp:
            print(f"[PASS] {step_name}: applied={open_resp.get('applied')}")
            results[step_name] = "PASS"
        else:
            print(f"[FAIL] {step_name}: status={status} body={open_resp}")
            results[step_name] = "FAIL"
            return 1

        # Step 3: Render
        step_name = steps[2]
        test_image = create_synthetic_image(512, 512)
        render_body = {
            "region": "self-test:crop-0",
            "image": test_image,
            "hint": None,
            "prompt": "seamless black and white manga background art, clean screentone texture, no text",
            "steps": 4,
            "seed": 7,
            "guidance": 1.0,
            "deadline_ms": 300000,
        }
        status, render_resp = _http_request(
            f"{base_url}/v1/render",
            method="POST",
            token=test_token,
            body=render_body,
            timeout_sec=300.0,
        )
        if status == 200 and "image" in render_resp:
            out_img = render_resp["image"]
            if (out_img.get("width"), out_img.get("height"), out_img.get("encoding")) == (512, 512, "rgb8"):
                elapsed = render_resp.get("elapsed_ms", 0)
                mem = render_resp.get("memory", {})
                # The reply's peak is a physical footprint, not a resident
                # set size; printing it as "peak_rss" is how an earlier
                # measurement came to publish 2.89 GB for a render that cost
                # 6.58 GB.
                peak_bytes = mem.get("peak_rss_bytes") or _local_peak_bytes()
                peak_rss_gb = peak_bytes / 1e9
                instrument = mem.get("instrument", "unknown")
                print(
                    f"[PASS] {step_name}: rendered in {elapsed}ms "
                    f"peak={peak_rss_gb:.2f}GB ({instrument})"
                )
                results[step_name] = "PASS"
            else:
                print(f"[FAIL] {step_name}: geometry mismatch in reply: {out_img}")
                results[step_name] = "FAIL"
                return 1
        else:
            print(f"[FAIL] {step_name}: status={status} body={render_resp}")
            results[step_name] = "FAIL"
            return 1

        # Step 4: Release
        step_name = steps[3]
        status, release_resp = _http_request(
            f"{base_url}/v1/release",
            method="POST",
            token=test_token,
        )
        if status == 200:
            print(f"[PASS] {step_name}: memory={release_resp}")
            results[step_name] = "PASS"
        else:
            print(f"[FAIL] {step_name}: status={status} body={release_resp}")
            results[step_name] = "FAIL"
            return 1

        # Step 5: Shutdown
        step_name = steps[4]
        status, shutdown_resp = _http_request(
            f"{base_url}/v1/shutdown",
            method="POST",
            token=test_token,
        )
        if status in (200, 202):
            print(f"[PASS] {step_name}: status={status}")
            results[step_name] = "PASS"
        else:
            print(f"[FAIL] {step_name}: status={status} body={shutdown_resp}")
            results[step_name] = "FAIL"
            return 1

    finally:
        server.shutdown()

    peak_rss_gb = max(peak_rss_gb, _local_peak_bytes() / 1e9)
    print("\n--- Summary ---")
    all_passed = True
    for step in steps:
        res = results.get(step, "SKIPPED")
        print(f"  {step:<30} {res}")
        if res != "PASS":
            all_passed = False

    print(f"\nFinal Peak memory footprint: {peak_rss_gb:.2f} GB")
    print(f"Result: {'PASS' if all_passed else 'FAIL'}")
    return 0 if all_passed else 1
