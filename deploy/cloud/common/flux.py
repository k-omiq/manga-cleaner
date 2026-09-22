"""Cloud FLUX Worker and Residency Abstraction.

Implements the cold/warm worker lifecycle, CUDA memory/residency simulation,
pure-Python synthetic PNG diffusion generation, and fault-injection controls
for offline verification.
"""

from __future__ import annotations

import hashlib
import struct
import time
import zlib
from enum import Enum
from typing import Optional, Tuple

from deploy.cloud.common.contract import (
    ContractValidationError,
    LATENT_STRIDE,
    ServiceLimits,
    provisional_fixture_limits,
)
from deploy.cloud.common.handle_mapping import JobRecord
from deploy.cloud.common.manifest import (
    MODEL_TEST_FLUX,
    RECIPE_TEST_SDNQ,
    REVISION_TEST_FLUX,
    PINNED_RECIPES,
)


def encode_png_rgb8(
    width: int,
    height: int,
    fill_rgb: Tuple[int, int, int] = (255, 255, 255),
) -> bytes:
    """Generate a single-frame RGB8 PNG with valid IHDR CRC-32 and zlib compression."""
    raw_rows = bytearray()
    row_bytes = bytes([fill_rgb[0] & 0xFF, fill_rgb[1] & 0xFF, fill_rgb[2] & 0xFF]) * width
    for _ in range(height):
        raw_rows.append(0)  # Filter byte: None (0)
        raw_rows.extend(row_bytes)

    compressed = zlib.compress(bytes(raw_rows), level=6)

    # IHDR chunk
    ihdr_data = struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0)
    ihdr_crc = zlib.crc32(b"IHDR" + ihdr_data) & 0xFFFFFFFF
    ihdr_chunk = struct.pack(">I", 13) + b"IHDR" + ihdr_data + struct.pack(">I", ihdr_crc)

    # IDAT chunk
    idat_len = len(compressed)
    idat_crc = zlib.crc32(b"IDAT" + compressed) & 0xFFFFFFFF
    idat_chunk = struct.pack(">I", idat_len) + b"IDAT" + compressed + struct.pack(">I", idat_crc)

    # IEND chunk
    iend_crc = zlib.crc32(b"IEND") & 0xFFFFFFFF
    iend_chunk = struct.pack(">I", 0) + b"IEND" + struct.pack(">I", iend_crc)

    return b"\x89PNG\r\n\x1a\n" + ihdr_chunk + idat_chunk + iend_chunk


class WorkerLifecycleState(str, Enum):
    UNINITIALIZED = "uninitialized"
    COLD = "cold"
    WARMING = "warming"
    WARMED = "warmed"
    BUSY = "busy"
    FAILED = "failed"


class FluxWorker:
    """Offline FLUX SDNQ Diffusion Worker with cold/warm lifecycle and fault injection."""

    def __init__(
        self,
        provider: str = "modal",
        limits: Optional[ServiceLimits] = None,
        simulated_cold_start_delay: float = 0.0,
        reported_cost_per_job: float = 0.0012,
    ):
        self.provider = provider
        self.limits = limits or provisional_fixture_limits()
        self.simulated_cold_start_delay = simulated_cold_start_delay
        self.reported_cost_per_job = reported_cost_per_job
        self.state = WorkerLifecycleState.UNINITIALIZED
        self.warmup_count = 0
        self.inference_count = 0
        self.last_inferred_at: Optional[float] = None
        self.simulated_fault: Optional[str] = None  # "oom", "timeout", "corrupted_output", "wrong_dimensions"

    @property
    def is_warm(self) -> bool:
        return self.state == WorkerLifecycleState.WARMED

    def warmup(self) -> dict:
        """Explicit potentially billable worker readiness probe."""
        self.state = WorkerLifecycleState.WARMING
        if self.simulated_cold_start_delay > 0:
            time.sleep(self.simulated_cold_start_delay)
        self.state = WorkerLifecycleState.WARMED
        self.warmup_count += 1
        return {
            "status": "warmed",
            "provider": self.provider,
            "worker_state": "warm",
            "warmup_count": self.warmup_count,
        }

    def infer(
        self,
        job_record: JobRecord,
        image_bytes: bytes,
        hint_bytes: bytes,
    ) -> Tuple[bytes, str, Optional[float]]:
        """Execute diffusion crop inpainting and return (result_bytes, result_digest, cost)."""
        # Simulate cold start transition if worker wasn't warmed
        was_cold = (self.state != WorkerLifecycleState.WARMED)
        if was_cold:
            self.state = WorkerLifecycleState.WARMING
            if self.simulated_cold_start_delay > 0:
                time.sleep(self.simulated_cold_start_delay)
            self.state = WorkerLifecycleState.WARMED

        # Fault injection hooks
        if self.simulated_fault == "oom":
            raise RuntimeError("CUDA out of memory during inference")
        elif self.simulated_fault == "timeout":
            raise TimeoutError("Worker execution timed out")
        elif self.simulated_fault == "corrupted_output":
            corrupted = b"\x89PNG\r\n\x1a\nCORRUPTED_IDAT_BYTES_TRUNCATED"
            digest = hashlib.sha256(corrupted).hexdigest()
            return corrupted, digest, self.reported_cost_per_job
        elif self.simulated_fault == "wrong_dimensions":
            # Return width + 16
            wrong_png = encode_png_rgb8(job_record.width + 16, job_record.height)
            digest = hashlib.sha256(wrong_png).hexdigest()
            return wrong_png, digest, self.reported_cost_per_job

        # Generate clean single-frame RGB8 PNG with exact dimensions snapped to latent stride
        result_png = encode_png_rgb8(job_record.width, job_record.height, fill_rgb=(240, 240, 240))
        result_digest = hashlib.sha256(result_png).hexdigest()

        self.inference_count += 1
        self.last_inferred_at = time.time()

        return result_png, result_digest, self.reported_cost_per_job
