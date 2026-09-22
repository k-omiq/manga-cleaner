"""Base provider driver interface and data transfer objects for cloud provisioning.

Defines the contract for Modal and Beam drivers:
- Account inspection with actionable missing-privilege analysis.
- Deterministic, content-hashed deployment and cleanup plans.
- Ownership-verified resource lifecycle operations.
- Explicit-client / isolated configuration semantics.
- Non-billable compatibility probes.
"""

from abc import ABC, abstractmethod
from dataclasses import asdict, dataclass, field
from datetime import datetime, timezone
import hashlib
import json
from typing import Any, Dict, List, Optional, Set

from provisioner.protocol import (
    ERR_ACTIONABLE_PERMISSION,
    ERR_SECURITY_VIOLATION,
    ERR_VALIDATION,
    ProtocolError,
    validate_hash,
    validate_identifier,
)
from provisioner.redaction import redact_data


def compute_canonical_hash(data: Dict[str, Any], fields_to_exclude: Optional[Set[str]] = None) -> str:
    """Compute deterministic SHA-256 hash over canonical JSON representation."""
    exclude = fields_to_exclude or {"plan_hash", "created_at_utc"}
    filtered = {k: v for k, v in data.items() if k not in exclude}
    canonical_json = json.dumps(filtered, sort_keys=True, separators=(",", ":"))
    return hashlib.sha256(canonical_json.encode("utf-8")).hexdigest()


@dataclass
class AccountInspectionResult:
    provider: str
    account_id: str
    workspace_name: str
    authenticated: bool
    permissions_granted: List[str]
    permissions_missing: List[str]
    eligible: bool
    actionable_remedy: Optional[str] = None
    platform_supported: bool = True
    platform_notes: str = ""

    def to_dict(self) -> Dict[str, Any]:
        return asdict(self)


@dataclass
class DeploymentPlan:
    plan_id: str
    installation_id: str
    provider: str
    app_name: str
    target_environment: str
    resource_allocation: Dict[str, Any]
    resources_to_create: List[Dict[str, Any]]
    required_permissions: List[str]
    estimated_monthly_cost: Optional[str]
    cleanup_plan_summary: List[str]
    plan_hash: str
    created_at_utc: str

    def to_dict(self) -> Dict[str, Any]:
        return asdict(self)


@dataclass
class CleanupPlan:
    plan_id: str
    installation_id: str
    provider: str
    resources_to_delete: List[Dict[str, Any]]
    foreign_resources_ignored: List[Dict[str, Any]]
    persistent_storage_requires_explicit_confirmation: bool
    plan_hash: str
    created_at_utc: str

    def to_dict(self) -> Dict[str, Any]:
        return asdict(self)


@dataclass
class CompatibilityValidationResult:
    endpoint_url: str
    status: str
    api_version: str
    model_recipe: str
    healthy: bool
    gpu_incurred: bool  # Invariant: must be False on non-inference compatibility probe
    latency_ms: float
    details: Dict[str, Any] = field(default_factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        return asdict(self)


class BaseProviderDriver(ABC):
    """Abstract driver interface for cloud providers."""

    def __init__(self, provider_id: str, allow_real_sdk: bool = False) -> None:
        self.provider_id = provider_id
        self.allow_real_sdk = allow_real_sdk

    @abstractmethod
    def inspect_account(
        self,
        credentials: Dict[str, Any],
        options: Optional[Dict[str, Any]] = None,
    ) -> AccountInspectionResult:
        """Inspect caller account, permissions, and platform suitability."""
        pass

    @abstractmethod
    def create_deployment_plan(
        self,
        inspection: AccountInspectionResult,
        installation_id: str,
        options: Optional[Dict[str, Any]] = None,
    ) -> DeploymentPlan:
        """Generate a deterministic, hashed deployment plan."""
        pass

    @abstractmethod
    def seed_volume(
        self,
        installation_id: str,
        credentials: Dict[str, Any],
    ) -> Dict[str, Any]:
        """Create or locate model storage volume."""
        pass

    @abstractmethod
    def deploy_service(
        self,
        installation_id: str,
        app_name: str,
        credentials: Dict[str, Any],
    ) -> Dict[str, Any]:
        """Deploy CPU control service / worker."""
        pass

    @abstractmethod
    def discover_endpoint(
        self,
        installation_id: str,
        app_name: str,
        credentials: Dict[str, Any],
    ) -> str:
        """Discover public HTTP endpoint URL."""
        pass

    @abstractmethod
    def create_runtime_credential(
        self,
        installation_id: str,
        credentials: Dict[str, Any],
    ) -> Dict[str, Any]:
        """Generate or scope runtime proxy/restricted token."""
        pass

    @abstractmethod
    def validate_compatibility(
        self,
        endpoint_url: str,
        runtime_credential: Dict[str, Any],
    ) -> CompatibilityValidationResult:
        """Probe GET /health and /model-info without GPU inference."""
        pass

    @abstractmethod
    def create_cleanup_plan(
        self,
        installation_id: str,
        known_resources: List[Dict[str, Any]],
    ) -> CleanupPlan:
        """Construct ownership-verified cleanup plan."""
        pass

    @abstractmethod
    def execute_cleanup(
        self,
        cleanup_plan: CleanupPlan,
        credentials: Dict[str, Any],
    ) -> Dict[str, Any]:
        """Execute teardown on verified owned resources."""
        pass
