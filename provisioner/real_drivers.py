"""Real provider driver adapters for Modal and Beam cloud provisioning.

Implements lazy real-driver boundaries:
- Fails closed when real SDKs are absent or unverified in the local runtime.
- Truthfully checks platform compatibility (macOS/Linux vs Windows WSL gating).
- Enforces Python version compatibility (Modal requires Python >= 3.10).
- Never mutates global CLI configuration files (~/.modal.toml, ~/.beam/config.ini).
- Never creates resources without an approved plan.
- Refuses unverified live operations with actionable remedy guidance.
"""

from datetime import datetime, timezone
import importlib.util
import os
from pathlib import Path
import sys
from typing import Any, Dict, List, Optional, Set

from provisioner.driver_base import (
    AccountInspectionResult,
    BaseProviderDriver,
    CleanupPlan,
    CompatibilityValidationResult,
    DeploymentPlan,
    compute_canonical_hash,
)
from provisioner.protocol import (
    ERR_ACTIONABLE_PERMISSION,
    ERR_EXECUTION_FAILED,
    ERR_PLATFORM_GATED,
    ERR_PROVIDER_UNAVAILABLE,
    ERR_SECURITY_VIOLATION,
    ERR_UNAPPROVED_PLAN,
    ERR_VALIDATION,
    ProtocolError,
    validate_https_url,
    validate_identifier,
)


def _utc_now() -> str:
    return datetime.now(timezone.utc).isoformat()


class RealModalDriver(BaseProviderDriver):
    """Lazy real SDK adapter driver for Modal provisioning."""

    def __init__(
        self,
        allow_real_sdk: bool = False,
        platform_override: Optional[str] = None,
        python_version_override: Optional[tuple] = None,
    ) -> None:
        super().__init__(provider_id="modal", allow_real_sdk=allow_real_sdk)
        self.platform_override = platform_override
        self.python_version_override = python_version_override

    def _check_runtime_and_sdk(self) -> None:
        """Enforce Python runtime compatibility and verify SDK presence."""
        py_ver = self.python_version_override or sys.version_info[:2]
        if py_ver < (3, 10):
            current_ver_str = f"{py_ver[0]}.{py_ver[1]}"
            raise ProtocolError(
                ERR_PLATFORM_GATED,
                f"Modal SDK candidate requires Python >= 3.10; current runtime is Python {current_ver_str} (macOS system Python 3.9.6 is incompatible)",
                actionable_guidance="Run helper using Python >= 3.10 or a bundled runtime environment.",
                remedy_steps=[
                    "Install Python >= 3.10 (compatible range: <3.15,>=3.10)",
                    "Execute helper within a Python 3.10+ virtualenv or bundled runtime",
                ],
            )

        if importlib.util.find_spec("modal") is None:
            raise ProtocolError(
                ERR_PROVIDER_UNAVAILABLE,
                "Modal SDK ('modal==1.5.5') is not installed in the current Python environment.",
                actionable_guidance="Install the pinned Modal SDK package or run within the packaged runtime bundle.",
                remedy_steps=[
                    "Install pinned wheel: pip install 'modal==1.5.5'",
                    "Ensure Python >= 3.10 is active",
                ],
            )

    def inspect_account(
        self,
        credentials: Dict[str, Any],
        options: Optional[Dict[str, Any]] = None,
    ) -> AccountInspectionResult:
        """Inspect Modal account using explicit credentials with zero global config mutation."""
        token_id = credentials.get("token_id") or credentials.get("modal_token_id")
        token_secret = credentials.get("token_secret") or credentials.get("modal_token_secret")

        if not token_id or not token_secret:
            raise ProtocolError(
                ERR_VALIDATION,
                "Modal explicit credentials require both 'token_id' (ak-...) and 'token_secret' (as-...)",
                actionable_guidance="Provide valid Modal API token ID and secret via secure in-memory parameters.",
                remedy_steps=[
                    "Generate API token in Modal Dashboard (Settings > API Tokens)",
                    "Supply 'token_id' and 'token_secret' in helper request params",
                ],
            )

        self._check_runtime_and_sdk()

        if not self.allow_real_sdk:
            raise ProtocolError(
                ERR_PLATFORM_GATED,
                "Live Modal account inspection is unverified in this environment.",
                actionable_guidance="Live account execution is a P1 prerequisite. Ensure live test account access is configured.",
                remedy_steps=[
                    "Verify Modal account API token permissions on https://modal.com/settings",
                    "For offline tests, explicitly inject FakeModalDriver",
                ],
            )

        # Real SDK scope probing is not yet implemented.
        # Fail closed: never hardcode granted permissions or claim deploy/token rights.
        raise ProtocolError(
            ERR_PLATFORM_GATED,
            "Modal live token permission scopes are unverified: automated scope probes for deploy/token/volume rights are not yet implemented. Live mutation rights cannot be claimed without explicit verification.",
            actionable_guidance="Live account execution is platform-gated until live scope probes are implemented. For offline testing and development, explicitly inject FakeModalDriver.",
            remedy_steps=[
                "Verify Modal account API token permissions on https://modal.com/settings",
                "Ensure token has required roles (workspace:view, apps:deploy, volumes:create, proxy_tokens:create)",
                "For test environments, inject FakeModalDriver explicitly",
            ],
        )

    def create_deployment_plan(
        self,
        inspection: AccountInspectionResult,
        installation_id: str,
        options: Optional[Dict[str, Any]] = None,
    ) -> DeploymentPlan:
        """Create deterministic deployment plan matching staging specification."""
        validate_identifier("installation_id", installation_id)
        if not inspection.eligible:
            raise ProtocolError(
                ERR_ACTIONABLE_PERMISSION,
                f"Cannot create deployment plan: account is missing required permissions: {inspection.permissions_missing}",
                actionable_guidance=inspection.actionable_remedy,
                remedy_steps=[
                    "Open Modal dashboard at https://modal.com/settings",
                    "Navigate to Workspace > API Tokens",
                    "Assign required roles to API token",
                ],
            )

        app_name = f"mc-app-{installation_id}"
        vol_name = f"mc-modal-vol-{installation_id}"
        token_name = f"tok-proxy-{installation_id}"

        resources_to_create = [
            {
                "name": vol_name,
                "type": "volume",
                "purpose": "Model weight cache and recipe storage",
                "tags": {"installation_id": installation_id, "managed_by": "manga-cleaner"},
            },
            {
                "name": app_name,
                "type": "app",
                "purpose": "Inference gateway and worker services",
                "tags": {"installation_id": installation_id, "managed_by": "manga-cleaner"},
            },
            {
                "name": token_name,
                "type": "proxy_token",
                "purpose": "Scoped runtime authentication for /mc/v1 endpoints",
                "tags": {"installation_id": installation_id, "managed_by": "manga-cleaner"},
            },
        ]

        allocation = {
            "cpu": 0.125,
            "memory_mb": 128,
            "gpu": None,
            "model_weights_required": False,
        }

        cleanup_summary = [
            f"Revoke and delete scoped proxy token '{token_name}'",
            f"Stop persistent app '{app_name}'",
            f"Delete persistent volume '{vol_name}' (requires explicit confirmation)",
        ]

        now = _utc_now()
        plan_dict = {
            "provider": "modal",
            "installation_id": installation_id,
            "app_name": app_name,
            "target_environment": "main",
            "resource_allocation": allocation,
            "resources_to_create": resources_to_create,
            "required_permissions": inspection.permissions_granted,
            "estimated_monthly_cost": "Zero cost when idle (scale-to-zero)",
            "cleanup_plan_summary": cleanup_summary,
        }
        plan_hash = compute_canonical_hash(plan_dict)

        return DeploymentPlan(
            plan_id=f"plan-modal-{installation_id}",
            installation_id=installation_id,
            provider="modal",
            app_name=app_name,
            target_environment="main",
            resource_allocation=allocation,
            resources_to_create=resources_to_create,
            required_permissions=inspection.permissions_granted,
            estimated_monthly_cost="Zero cost when idle (scale-to-zero)",
            cleanup_plan_summary=cleanup_summary,
            plan_hash=plan_hash,
            created_at_utc=now,
        )

    def seed_volume(
        self,
        installation_id: str,
        credentials: Dict[str, Any],
    ) -> Dict[str, Any]:
        """Refuse unverified live volume creation."""
        validate_identifier("installation_id", installation_id)
        self._check_runtime_and_sdk()
        raise ProtocolError(
            ERR_PLATFORM_GATED,
            "Live Modal volume creation is unverified on this platform/account. Live mutation refused.",
            actionable_guidance="Live account execution is a P1 prerequisite and requires verified live test account access.",
            remedy_steps=[
                "Complete live account verification prerequisites as documented in docs/cloud-feasibility.md",
                "In test environments, inject FakeModalDriver explicitly",
            ],
        )

    def deploy_service(
        self,
        installation_id: str,
        app_name: str,
        credentials: Dict[str, Any],
    ) -> Dict[str, Any]:
        """Refuse unverified live service deployment."""
        validate_identifier("installation_id", installation_id)
        validate_identifier("app_name", app_name)
        self._check_runtime_and_sdk()
        raise ProtocolError(
            ERR_PLATFORM_GATED,
            "Live Modal app deployment is unverified on this platform/account. Live mutation refused.",
            actionable_guidance="Live account execution is a P1 prerequisite and requires verified live test account access.",
            remedy_steps=[
                "Complete live account verification prerequisites as documented in docs/cloud-feasibility.md",
                "In test environments, inject FakeModalDriver explicitly",
            ],
        )

    def discover_endpoint(
        self,
        installation_id: str,
        app_name: str,
        credentials: Dict[str, Any],
    ) -> str:
        """Refuse unverified live endpoint discovery."""
        validate_identifier("installation_id", installation_id)
        validate_identifier("app_name", app_name)
        self._check_runtime_and_sdk()
        raise ProtocolError(
            ERR_PLATFORM_GATED,
            "Live Modal endpoint discovery is unverified on this platform/account.",
            actionable_guidance="Complete live account verification prerequisites.",
        )

    def create_runtime_credential(
        self,
        installation_id: str,
        credentials: Dict[str, Any],
    ) -> Dict[str, Any]:
        """Refuse unverified live proxy token creation."""
        validate_identifier("installation_id", installation_id)
        self._check_runtime_and_sdk()
        raise ProtocolError(
            ERR_PLATFORM_GATED,
            "Live Modal proxy token creation is unverified on this platform/account. Live mutation refused.",
            actionable_guidance="Complete live account verification prerequisites.",
        )

    def validate_compatibility(
        self,
        endpoint_url: str,
        runtime_credential: Dict[str, Any],
    ) -> CompatibilityValidationResult:
        """Probe GET /health on HTTPS endpoint without GPU inference."""
        validate_https_url("endpoint_url", endpoint_url)
        raise ProtocolError(
            ERR_PLATFORM_GATED,
            f"Compatibility probe against live endpoint '{endpoint_url}' unverified in offline helper boundary.",
            actionable_guidance="Endpoint validation requires live network connectivity to verified deployment.",
        )

    def create_cleanup_plan(
        self,
        installation_id: str,
        known_resources: List[Dict[str, Any]],
    ) -> CleanupPlan:
        """Generate ownership-verified cleanup plan for Modal resources."""
        validate_identifier("installation_id", installation_id)
        to_delete = []
        ignored = []

        for r in known_resources:
            tags = r.get("ownership_tags", {})
            if tags.get("managed_by") == "manga-cleaner" and tags.get("installation_id") == installation_id:
                to_delete.append(r)
            else:
                ignored.append(r)

        plan_dict = {
            "provider": "modal",
            "installation_id": installation_id,
            "resources_to_delete": to_delete,
            "foreign_resources_ignored": ignored,
            "persistent_storage_requires_explicit_confirmation": True,
        }
        plan_hash = compute_canonical_hash(plan_dict)

        return CleanupPlan(
            plan_id=f"clean-modal-{installation_id}",
            installation_id=installation_id,
            provider="modal",
            resources_to_delete=to_delete,
            foreign_resources_ignored=ignored,
            persistent_storage_requires_explicit_confirmation=True,
            plan_hash=plan_hash,
            created_at_utc=_utc_now(),
        )

    def execute_cleanup(
        self,
        cleanup_plan: CleanupPlan,
        credentials: Dict[str, Any],
    ) -> Dict[str, Any]:
        """Refuse unverified live resource teardown."""
        self._check_runtime_and_sdk()
        raise ProtocolError(
            ERR_PLATFORM_GATED,
            "Live Modal resource teardown is unverified on this platform/account. Live mutation refused.",
            actionable_guidance="Complete live account verification prerequisites.",
        )


class RealBeamDriver(BaseProviderDriver):
    """Lazy real SDK adapter driver for Beam / Beta9 provisioning."""

    def __init__(
        self,
        allow_real_sdk: bool = False,
        platform_override: Optional[str] = None,
    ) -> None:
        super().__init__(provider_id="beam", allow_real_sdk=allow_real_sdk)
        self.platform_override = platform_override

    def _check_platform_and_sdk(self) -> None:
        """Check platform support (Windows WSL gating) and SDK presence."""
        plat = self.platform_override or sys.platform
        if plat.startswith("win"):
            raise ProtocolError(
                ERR_PLATFORM_GATED,
                "Beam is officially unsupported on native Windows (documented only through WSL Ubuntu 22.04). Native Windows packaging is unverified.",
                actionable_guidance="Use WSL (Windows Subsystem for Linux) with Ubuntu 22.04, or use the Modal provider on Windows.",
                remedy_steps=[
                    "Install WSL2 and Ubuntu 22.04 on Windows",
                    "Run manga-cleaner provisioner inside WSL Ubuntu environment",
                    "Alternatively, select Modal cloud provider",
                ],
            )

        has_beam = importlib.util.find_spec("beam") is not None
        has_beta9 = importlib.util.find_spec("beta9") is not None
        if not has_beam and not has_beta9:
            raise ProtocolError(
                ERR_PROVIDER_UNAVAILABLE,
                "Beam SDK ('beam-client==0.2.211' / 'beta9==0.1.268') is not installed in the current Python environment.",
                actionable_guidance="Install pinned beam-client and beta9 packages or run within the packaged runtime bundle.",
                remedy_steps=[
                    "Install pinned wheel: pip install 'beam-client==0.2.211'",
                    "Install pinned wheel: pip install 'beta9==0.1.268'",
                ],
            )

    def inspect_account(
        self,
        credentials: Dict[str, Any],
        options: Optional[Dict[str, Any]] = None,
    ) -> AccountInspectionResult:
        """Inspect Beam account with isolated config semantics (never touch ~/.beam/config.ini)."""
        token = credentials.get("token") or credentials.get("beam_token")
        if not token:
            raise ProtocolError(
                ERR_VALIDATION,
                "Beam explicit credentials require 'token' or 'beam_token' parameter (b9_...)",
                actionable_guidance="Provide valid Beam API token via secure in-memory parameters.",
                remedy_steps=[
                    "Generate API token in Beam Dashboard (Settings > API Keys)",
                    "Supply 'token' or 'beam_token' in helper request params",
                ],
            )

        self._check_platform_and_sdk()

        if not self.allow_real_sdk:
            raise ProtocolError(
                ERR_PLATFORM_GATED,
                "Live Beam account inspection is unverified in this environment.",
                actionable_guidance="Live account execution is a P1 prerequisite. Ensure live test account access is configured.",
                remedy_steps=[
                    "Verify Beam API token permissions on https://beam.cloud",
                    "For offline tests, explicitly inject FakeBeamDriver",
                ],
            )

        # Real SDK scope probing is not yet implemented.
        # Fail closed: never hardcode granted permissions or claim deploy/token rights.
        raise ProtocolError(
            ERR_PLATFORM_GATED,
            "Beam live token permission scopes are unverified: automated scope probes for deployment/storage/token rights are not yet implemented. Live mutation rights cannot be claimed without explicit verification.",
            actionable_guidance="Live account execution is platform-gated until live scope probes are implemented. For offline testing and development, explicitly inject FakeBeamDriver.",
            remedy_steps=[
                "Verify Beam API token permissions on https://beam.cloud",
                "Ensure token has required roles (workspace:read, deployment:create, storage:create, token:create)",
                "For test environments, inject FakeBeamDriver explicitly",
            ],
        )

    def create_deployment_plan(
        self,
        inspection: AccountInspectionResult,
        installation_id: str,
        options: Optional[Dict[str, Any]] = None,
    ) -> DeploymentPlan:
        """Create deterministic deployment plan matching staging specification."""
        validate_identifier("installation_id", installation_id)
        if not inspection.eligible:
            raise ProtocolError(
                ERR_ACTIONABLE_PERMISSION,
                f"Cannot create deployment plan: account is missing required permissions: {inspection.permissions_missing}",
                actionable_guidance=inspection.actionable_remedy,
                remedy_steps=[
                    "Open Beam dashboard at https://beam.cloud",
                    "Navigate to Settings > API Keys",
                    "Ensure token has deployment and storage management permissions",
                ],
            )

        service_name = f"mc-beam-svc-{installation_id}"
        vol_name = f"mc-beam-vol-{installation_id}"
        token_name = f"b9-tok-{installation_id}"

        resources_to_create = [
            {
                "name": vol_name,
                "type": "volume",
                "purpose": "Model weight cache and recipe storage",
                "tags": {"installation_id": installation_id, "managed_by": "manga-cleaner"},
            },
            {
                "name": service_name,
                "type": "service",
                "purpose": "CPU control service and task router",
                "tags": {"installation_id": installation_id, "managed_by": "manga-cleaner"},
            },
            {
                "name": token_name,
                "type": "restricted_token",
                "purpose": "Scoped runtime token for /mc/v1 endpoints",
                "tags": {"installation_id": installation_id, "managed_by": "manga-cleaner"},
            },
        ]

        allocation = {
            "cpu": 0.5,
            "memory_mb": 512,
            "gpu": None,
            "model_weights_required": False,
        }

        cleanup_summary = [
            f"Revoke and delete restricted token '{token_name}'",
            f"Delete deployed service '{service_name}'",
            f"Delete persistent volume '{vol_name}' (requires explicit confirmation)",
        ]

        now = _utc_now()
        plan_dict = {
            "provider": "beam",
            "installation_id": installation_id,
            "app_name": service_name,
            "target_environment": "default",
            "resource_allocation": allocation,
            "resources_to_create": resources_to_create,
            "required_permissions": inspection.permissions_granted,
            "estimated_monthly_cost": "Zero cost when idle (scale-to-zero)",
            "cleanup_plan_summary": cleanup_summary,
        }
        plan_hash = compute_canonical_hash(plan_dict)

        return DeploymentPlan(
            plan_id=f"plan-beam-{installation_id}",
            installation_id=installation_id,
            provider="beam",
            app_name=service_name,
            target_environment="default",
            resource_allocation=allocation,
            resources_to_create=resources_to_create,
            required_permissions=inspection.permissions_granted,
            estimated_monthly_cost="Zero cost when idle (scale-to-zero)",
            cleanup_plan_summary=cleanup_summary,
            plan_hash=plan_hash,
            created_at_utc=now,
        )

    def seed_volume(
        self,
        installation_id: str,
        credentials: Dict[str, Any],
    ) -> Dict[str, Any]:
        """Refuse unverified live volume creation."""
        validate_identifier("installation_id", installation_id)
        self._check_platform_and_sdk()
        raise ProtocolError(
            ERR_PLATFORM_GATED,
            "Live Beam volume creation is unverified on this platform/account. Live mutation refused.",
            actionable_guidance="Live account execution is a P1 prerequisite and requires verified live test account access.",
            remedy_steps=[
                "Complete live account verification prerequisites as documented in docs/cloud-feasibility.md",
                "In test environments, inject FakeBeamDriver explicitly",
            ],
        )

    def deploy_service(
        self,
        installation_id: str,
        app_name: str,
        credentials: Dict[str, Any],
    ) -> Dict[str, Any]:
        """Refuse unverified live service deployment."""
        validate_identifier("installation_id", installation_id)
        validate_identifier("app_name", app_name)
        self._check_platform_and_sdk()
        raise ProtocolError(
            ERR_PLATFORM_GATED,
            "Live Beam service deployment is unverified on this platform/account. Live mutation refused.",
            actionable_guidance="Live account execution is a P1 prerequisite and requires verified live test account access.",
            remedy_steps=[
                "Complete live account verification prerequisites as documented in docs/cloud-feasibility.md",
                "In test environments, inject FakeBeamDriver explicitly",
            ],
        )

    def discover_endpoint(
        self,
        installation_id: str,
        app_name: str,
        credentials: Dict[str, Any],
    ) -> str:
        """Refuse unverified live endpoint discovery."""
        validate_identifier("installation_id", installation_id)
        validate_identifier("app_name", app_name)
        self._check_platform_and_sdk()
        raise ProtocolError(
            ERR_PLATFORM_GATED,
            "Live Beam endpoint discovery is unverified on this platform/account.",
            actionable_guidance="Complete live account verification prerequisites.",
        )

    def create_runtime_credential(
        self,
        installation_id: str,
        credentials: Dict[str, Any],
    ) -> Dict[str, Any]:
        """Refuse unverified live token creation."""
        validate_identifier("installation_id", installation_id)
        self._check_platform_and_sdk()
        raise ProtocolError(
            ERR_PLATFORM_GATED,
            "Live Beam restricted token creation is unverified on this platform/account. Live mutation refused.",
            actionable_guidance="Complete live account verification prerequisites.",
        )

    def validate_compatibility(
        self,
        endpoint_url: str,
        runtime_credential: Dict[str, Any],
    ) -> CompatibilityValidationResult:
        """Probe GET /health on HTTPS endpoint without GPU inference."""
        validate_https_url("endpoint_url", endpoint_url)
        raise ProtocolError(
            ERR_PLATFORM_GATED,
            f"Compatibility probe against live endpoint '{endpoint_url}' unverified in offline helper boundary.",
            actionable_guidance="Endpoint validation requires live network connectivity to verified deployment.",
        )

    def create_cleanup_plan(
        self,
        installation_id: str,
        known_resources: List[Dict[str, Any]],
    ) -> CleanupPlan:
        """Generate ownership-verified cleanup plan for Beam resources."""
        validate_identifier("installation_id", installation_id)
        to_delete = []
        ignored = []

        for r in known_resources:
            tags = r.get("ownership_tags", {})
            if tags.get("managed_by") == "manga-cleaner" and tags.get("installation_id") == installation_id:
                to_delete.append(r)
            else:
                ignored.append(r)

        plan_dict = {
            "provider": "beam",
            "installation_id": installation_id,
            "resources_to_delete": to_delete,
            "foreign_resources_ignored": ignored,
            "persistent_storage_requires_explicit_confirmation": True,
        }
        plan_hash = compute_canonical_hash(plan_dict)

        return CleanupPlan(
            plan_id=f"clean-beam-{installation_id}",
            installation_id=installation_id,
            provider="beam",
            resources_to_delete=to_delete,
            foreign_resources_ignored=ignored,
            persistent_storage_requires_explicit_confirmation=True,
            plan_hash=plan_hash,
            created_at_utc=_utc_now(),
        )

    def execute_cleanup(
        self,
        cleanup_plan: CleanupPlan,
        credentials: Dict[str, Any],
    ) -> Dict[str, Any]:
        """Refuse unverified live resource teardown."""
        self._check_platform_and_sdk()
        raise ProtocolError(
            ERR_PLATFORM_GATED,
            "Live Beam resource teardown is unverified on this platform/account. Live mutation refused.",
            actionable_guidance="Complete live account verification prerequisites.",
        )
