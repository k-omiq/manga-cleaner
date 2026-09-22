"""Beam Feasibility Source Sample (CPU-Only Packaging Fixture).

NONDEPLOYABLE SOURCE SAMPLE:
- This file is an offline packaging and AST syntax validation fixture only.
- It is NOT deployable as-is: it lacks real Beam decorator wiring (@beam.endpoint)
  and real authentication middleware.
- Contains NO production model IDs, NO recipe revisions, and NO GPU worker stubs.
"""

from typing import Any, Dict
import time

FEASIBILITY_FIXTURE_VERSION = "0.1.0-offline"


def handle_health() -> Dict[str, Any]:
    """Minimal unauthenticated health check handler for source-packaging verification.

    NOTE: This is an offline feasibility stub. It does NOT provide or prove
    cloud authentication, GPU allocation, or container residency.
    """
    return {
        "status": "offline_fixture",
        "provider": "beam",
        "fixture_version": FEASIBILITY_FIXTURE_VERSION,
        "timestamp": time.time(),
    }
