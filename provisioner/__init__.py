"""Manga Cleaner Cloud Provisioner Package (P1, P7, P8).

Provides:
- Pinned candidate matrix and AST packaging probes (P1).
- Versioned allowlisted helper protocol (P7/P8).
- Resumable installation/resource journal with atomic durability (P7/P8).
- Deterministic fake provider drivers for Modal and Beam.
- Isolated configuration semantics (zero mutation of global CLI configs).
- Credential redaction and non-billable compatibility checks.
"""

from provisioner.cli import main, run_helper
from provisioner.controller import ProvisioningController
from provisioner.driver_base import (
    AccountInspectionResult,
    BaseProviderDriver,
    CleanupPlan,
    CompatibilityValidationResult,
    DeploymentPlan,
)
from provisioner.fake_drivers import FakeBeamDriver, FakeModalDriver
from provisioner.journal import InstallationJournal, InstallationRecord, ResourceRecord
from provisioner.protocol import (
    ERR_PROVIDER_UNAVAILABLE,
    HELPER_PROTOCOL_VERSION,
    HelperRequest,
    HelperResponse,
    ProtocolError,
    make_error_response,
    make_success_response,
    parse_request,
)
from provisioner.real_drivers import RealBeamDriver, RealModalDriver
from provisioner.redaction import redact_data, redact_string

__version__ = "0.2.0"
__all__ = [
    "HELPER_PROTOCOL_VERSION",
    "ERR_PROVIDER_UNAVAILABLE",
    "ProvisioningController",
    "InstallationJournal",
    "InstallationRecord",
    "ResourceRecord",
    "BaseProviderDriver",
    "FakeModalDriver",
    "FakeBeamDriver",
    "RealModalDriver",
    "RealBeamDriver",
    "AccountInspectionResult",
    "DeploymentPlan",
    "CleanupPlan",
    "CompatibilityValidationResult",
    "HelperRequest",
    "HelperResponse",
    "ProtocolError",
    "parse_request",
    "make_success_response",
    "make_error_response",
    "redact_data",
    "redact_string",
    "main",
    "run_helper",
]
