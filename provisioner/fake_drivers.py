"""Deterministic fake provider drivers for Modal and Beam automatic provisioning.

Implements P7 and P8 offline driver requirements:
- Inspect account with actionable missing-privilege reporting.
- Deterministic deployment plans with SHA-256 integrity hashes.
- Seed, deploy, endpoint discovery, and runtime credential creation.
- Compatibility validation (GET /health and /model-info) with zero GPU charge.
- Interruption and resume without duplicate resources.
- Ownership-verified cleanup plans that ignore foreign resources.
- Modal driver: Explicit-client semantics (no ~/.modal.toml mutation).
- Beam driver: Isolated configuration semantics (never touches ~/.beam/config.ini).
- Truthful Windows platform gating for Beam with actionable WSL / manual fallback.
- Real SDK imports and actions are optional and disabled by default.
"""

from datetime import datetime, timezone
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
    ERR_VALIDATION,
    ProtocolError,
    validate_https_url,
)
from provisioner.redaction import redact_data, redact_string


def _utc_now() -> str:
    return datetime.now(timezone.utc).isoformat()


class FakeModalDriver(BaseProviderDriver):
    """Deterministic offline driver for Modal provisioning."""

    def __init__(
        self,
        allow_real_sdk: bool = False,
        simulated_missing_permissions: Optional[Set[str]] = None,
        simulate_failure_at_step: Optional[str] = None,
        platform_override: Optional[str] = None,
    ) -> None:
        super().__init__(provider_id="modal", allow_real_sdk=allow_real_sdk)
        self.simulated_missing_permissions: Set[str] = simulated_missing_permissions or set()
        self.simulate_failure_at_step: Optional[str] = simulate_failure_at_step
        self.platform_override: Optional[str] = platform_override
        # In-memory mock cloud resource store: resource_id -> dict
        self.cloud_store: Dict[str, Dict[str, Any]] = {}
        # Invariant checks: track whether global config file was ever touched
        self.global_config_touched: bool = False

    def inspect_account(
        self,
        credentials: Dict[str, Any],
        options: Optional[Dict[str, Any]] = None,
    ) -> AccountInspectionResult:
        """Inspect Modal account using explicit-client semantics (no ~/.modal.toml)."""
        # Validate explicit credentials
        token_id = credentials.get("token_id") or credentials.get("modal_token_id")
        token_secret = credentials.get("token_secret") or credentials.get("modal_token_secret")

        if not token_id or not token_secret:
            raise ProtocolError(
                ERR_VALIDATION,
                "Modal explicit credentials require both 'token_id' (ak-...) and 'token_secret' (as-...)",
                actionable_guidance="Provide valid Modal API token ID and secret via secure in-memory parameters.",
            )

        all_permissions = {
            "workspace:view",
            "apps:deploy",
            "volumes:create",
            "proxy_tokens:create",
        }
        missing = sorted(list(self.simulated_missing_permissions.intersection(all_permissions)))
        granted = sorted(list(all_permissions - self.simulated_missing_permissions))

        eligible = len(missing) == 0
        remedy: Optional[str] = None

        if not eligible:
            remedy_parts = []
            for perm in missing:
                if perm == "proxy_tokens:create":
                    remedy_parts.append(
                        "Missing permission 'proxy_tokens:create'. In Modal Settings > Workspace > Roles, "
                        "ensure the API token is granted 'Workspace Developer' or proxy token creation rights."
                    )
                elif perm == "apps:deploy":
                    remedy_parts.append(
                        "Missing permission 'apps:deploy'. Ensure the API token has permission to deploy applications."
                    )
                elif perm == "volumes:create":
                    remedy_parts.append(
                        "Missing permission 'volumes:create'. Ensure the API token has volume management rights."
                    )
                else:
                    remedy_parts.append(f"Missing permission '{perm}'. Grant this permission in the Modal workspace settings.")
            remedy = " ".join(remedy_parts)

        # Platform check
        plat = self.platform_override or sys.platform
        platform_notes = f"Pure Python wheel supported on {plat} (requires Python >= 3.10 runtime bundle)"

        return AccountInspectionResult(
            provider="modal",
            account_id="acc_modal_simulated_user",
            workspace_name="manga-cleaner-ws",
            authenticated=True,
            permissions_granted=granted,
            permissions_missing=missing,
            eligible=eligible,
            actionable_remedy=remedy,
            platform_supported=True,
            platform_notes=platform_notes,
        )

    def create_deployment_plan(
        self,
        inspection: AccountInspectionResult,
        installation_id: str,
        options: Optional[Dict[str, Any]] = None,
    ) -> DeploymentPlan:
        """Create deterministic deployment plan with explicit client isolation."""
        if not inspection.eligible:
            raise ProtocolError(
                ERR_ACTIONABLE_PERMISSION,
                f"Cannot create deployment plan: account is missing required permissions: {inspection.permissions_missing}",
                actionable_guidance=inspection.actionable_remedy,
                remedy_steps=[
                    "Open Modal dashboard at https://modal.com/settings",
                    "Navigate to Workspace > API Tokens",
                    "Assign the required roles to your token or generate an Admin token",
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
            f"Delete persistent volume '{vol_name}' (requires explicit approval)",
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
        """Create model volume in cloud store; reuse if already present (no duplicate)."""
        if self.simulate_failure_at_step == "seed":
            raise ProtocolError(
                ERR_EXECUTION_FAILED,
                "Simulated failure during volume seeding",
                actionable_guidance="Verify volume creation quota in Modal dashboard.",
            )

        vol_name = f"mc-modal-vol-{installation_id}"
        # Check existing
        for res_id, res in self.cloud_store.items():
            if res.get("name") == vol_name and res.get("type") == "volume":
                return res

        vol_id = f"vol_modal_{installation_id}"
        res = {
            "resource_id": vol_id,
            "resource_type": "volume",
            "name": vol_name,
            "provider": "modal",
            "status": "ready",
            "ownership_tags": {
                "installation_id": installation_id,
                "managed_by": "manga-cleaner",
            },
        }
        self.cloud_store[vol_id] = res
        return res

    def deploy_service(
        self,
        installation_id: str,
        app_name: str,
        credentials: Dict[str, Any],
    ) -> Dict[str, Any]:
        """Deploy application in cloud store; reuse if already deployed."""
        if self.simulate_failure_at_step == "deploy":
            raise ProtocolError(
                ERR_EXECUTION_FAILED,
                "Simulated failure during application deployment",
                actionable_guidance="Check Modal build logs for container preparation errors.",
            )

        # Check existing
        for res_id, res in self.cloud_store.items():
            if res.get("name") == app_name and res.get("type") == "app":
                return res

        app_id = f"app_modal_{installation_id}"
        res = {
            "resource_id": app_id,
            "resource_type": "app",
            "name": app_name,
            "provider": "modal",
            "status": "deployed",
            "ownership_tags": {
                "installation_id": installation_id,
                "managed_by": "manga-cleaner",
            },
        }
        self.cloud_store[app_id] = res
        return res

    def discover_endpoint(
        self,
        installation_id: str,
        app_name: str,
        credentials: Dict[str, Any],
    ) -> str:
        """Discover live endpoint URL for deployed app."""
        if self.simulate_failure_at_step == "endpoint":
            raise ProtocolError(
                ERR_EXECUTION_FAILED,
                "Simulated failure during endpoint URL discovery",
            )
        return f"https://manga-cleaner-ws--{app_name}.modal.run"

    def create_runtime_credential(
        self,
        installation_id: str,
        credentials: Dict[str, Any],
    ) -> Dict[str, Any]:
        """Create scoped proxy token."""
        if self.simulate_failure_at_step == "runtime_credential":
            raise ProtocolError(
                ERR_EXECUTION_FAILED,
                "Simulated failure during runtime proxy token creation",
            )

        tok_id = f"tok_modal_{installation_id}"
        token_name = f"tok-proxy-{installation_id}"
        # Return opaque reference and token value
        res = {
            "resource_id": tok_id,
            "resource_type": "proxy_token",
            "name": token_name,
            "provider": "modal",
            "token_id": f"proxy-id-{installation_id}",
            "token_secret": f"as-proxy-secret-{installation_id}",
            "scoped_environment": "main",
            "ownership_tags": {
                "installation_id": installation_id,
                "managed_by": "manga-cleaner",
            },
        }
        self.cloud_store[tok_id] = res
        return res

    def validate_compatibility(
        self,
        endpoint_url: str,
        runtime_credential: Dict[str, Any],
    ) -> CompatibilityValidationResult:
        """Simulate probing GET /health and GET /model-info."""
        validate_https_url("endpoint_url", endpoint_url)
        if self.simulate_failure_at_step == "validate":
            return CompatibilityValidationResult(
                endpoint_url=endpoint_url,
                status="unhealthy",
                api_version="1.0.0",
                model_recipe="unknown",
                healthy=False,
                gpu_incurred=False,
                latency_ms=120.0,
                details={"error": "Gateway returned HTTP 503 Service Unavailable"},
            )

        return CompatibilityValidationResult(
            endpoint_url=endpoint_url,
            status="healthy",
            api_version="1.0.0",
            model_recipe="flux-crop-sdnq-v1",
            healthy=True,
            gpu_incurred=False,  # Proves zero GPU charge
            latency_ms=45.2,
            details={"provider": "modal", "verified_offline": True},
        )

    def create_cleanup_plan(
        self,
        installation_id: str,
        known_resources: List[Dict[str, Any]],
    ) -> CleanupPlan:
        """Create cleanup plan that strictly verifies resource ownership before deletion."""
        to_delete = []
        foreign_ignored = []

        # Check all resources in known list and cloud store
        all_res = list(known_resources)
        for s_id, s_res in self.cloud_store.items():
            if not any(r.get("resource_id") == s_id for r in all_res):
                all_res.append(s_res)

        for r in all_res:
            tags = r.get("ownership_tags", {})
            r_inst = tags.get("installation_id")
            managed_by = tags.get("managed_by")

            if r_inst == installation_id and managed_by == "manga-cleaner":
                to_delete.append({
                    "resource_id": r.get("resource_id"),
                    "resource_type": r.get("resource_type"),
                    "name": r.get("name"),
                    "verified_ownership": True,
                })
            else:
                foreign_ignored.append({
                    "resource_id": r.get("resource_id"),
                    "resource_type": r.get("resource_type"),
                    "name": r.get("name"),
                    "reason": "Not owned by this installation or missing manga-cleaner tag",
                })

        now = _utc_now()
        plan_dict = {
            "provider": "modal",
            "installation_id": installation_id,
            "resources_to_delete": to_delete,
            "foreign_resources_ignored": foreign_ignored,
            "persistent_storage_requires_explicit_confirmation": True,
        }
        plan_hash = compute_canonical_hash(plan_dict)

        return CleanupPlan(
            plan_id=f"cleanup-modal-{installation_id}",
            installation_id=installation_id,
            provider="modal",
            resources_to_delete=to_delete,
            foreign_resources_ignored=foreign_ignored,
            persistent_storage_requires_explicit_confirmation=True,
            plan_hash=plan_hash,
            created_at_utc=now,
        )

    def execute_cleanup(
        self,
        cleanup_plan: CleanupPlan,
        credentials: Dict[str, Any],
    ) -> Dict[str, Any]:
        """Delete only verified owned resources in the cleanup plan."""
        deleted = []
        for item in cleanup_plan.resources_to_delete:
            r_id = item.get("resource_id")
            if r_id in self.cloud_store:
                del self.cloud_store[r_id]
                deleted.append(r_id)
        return {
            "deleted_resources": deleted,
            "total_deleted": len(deleted),
            "status": "cleaned_up",
        }


class FakeBeamDriver(BaseProviderDriver):
    """Deterministic offline driver for Beam provisioning.

    Enforces:
    - Isolated config semantics (never accesses or writes ~/.beam/config.ini).
    - Truthful Windows platform gating (blocks unverified native setup, details WSL/manual fallback).
    - Actionable missing privileges.
    - Ownership-verified cleanup.
    """

    def __init__(
        self,
        allow_real_sdk: bool = False,
        simulated_missing_permissions: Optional[Set[str]] = None,
        simulate_failure_at_step: Optional[str] = None,
        platform_override: Optional[str] = None,
    ) -> None:
        super().__init__(provider_id="beam", allow_real_sdk=allow_real_sdk)
        self.simulated_missing_permissions: Set[str] = simulated_missing_permissions or set()
        self.simulate_failure_at_step: Optional[str] = simulate_failure_at_step
        self.platform_override: Optional[str] = platform_override
        self.cloud_store: Dict[str, Dict[str, Any]] = {}
        # Invariant check: track whether ~/.beam/config.ini was ever accessed
        self.beam_config_ini_accessed: bool = False

    def inspect_account(
        self,
        credentials: Dict[str, Any],
        options: Optional[Dict[str, Any]] = None,
    ) -> AccountInspectionResult:
        """Inspect Beam account using isolated configuration (BEAM_TOKEN, zero config.ini)."""
        token = credentials.get("beam_token") or credentials.get("token")
        if not token or not isinstance(token, str):
            raise ProtocolError(
                ERR_VALIDATION,
                "Beam isolated authentication requires a valid 'beam_token' parameter (e.g. b9_...)",
                actionable_guidance="Provide a valid Beam API token via secure in-memory credentials.",
            )

        # Truthful Windows platform gate
        current_plat = (self.platform_override or sys.platform).lower()
        is_windows = current_plat.startswith("win")

        if is_windows:
            wsl_remedy = (
                "Native Windows setup for beam-client is unverified and unsupported out of the box. "
                "Official Beam tooling documents Windows deployment ONLY through WSL (Ubuntu 22.04 LTS). "
                "Recommended fallbacks:\n"
                "1. WSL Setup: Install WSL 2 ('wsl --install'), launch Ubuntu 22.04, install 'beam-client', "
                "and deploy from WSL.\n"
                "2. Manual Endpoint: Deploy your container independently and use 'Connect an existing deployment' "
                "in Manga Cleaner Settings with your HTTPS endpoint URL and runtime token."
            )
            return AccountInspectionResult(
                provider="beam",
                account_id="acc_beam_simulated_user",
                workspace_name="manga-cleaner-beam-ws",
                authenticated=True,
                permissions_granted=[],
                permissions_missing=["platform:native_windows_supported"],
                eligible=False,
                actionable_remedy=wsl_remedy,
                platform_supported=False,
                platform_notes="Native Windows execution is unverified; official documentation specifies WSL Ubuntu 22.04.",
            )

        all_permissions = {
            "workspace:view",
            "deployments:create",
            "tokens:create",
            "volumes:create",
        }
        missing = sorted(list(self.simulated_missing_permissions.intersection(all_permissions)))
        granted = sorted(list(all_permissions - self.simulated_missing_permissions))
        eligible = len(missing) == 0

        remedy: Optional[str] = None
        if not eligible:
            parts = []
            for perm in missing:
                if perm == "deployments:create":
                    parts.append(
                        "Missing required Beam permission 'deployments:create'. In Beam Dashboard > Settings > API Keys, "
                        "ensure the API key has 'Deployment Creator' or 'Admin' privileges."
                    )
                elif perm == "tokens:create":
                    parts.append(
                        "Missing required Beam permission 'tokens:create'. In Beam Dashboard > Settings > API Keys, "
                        "enable token creation permissions."
                    )
                elif perm == "volumes:create":
                    parts.append(
                        "Missing required Beam permission 'volumes:create'. Ensure your account has storage volume rights."
                    )
                else:
                    parts.append(f"Missing required permission '{perm}'.")
            remedy = " ".join(parts)

        return AccountInspectionResult(
            provider="beam",
            account_id="acc_beam_simulated_user",
            workspace_name="manga-cleaner-beam-ws",
            authenticated=True,
            permissions_granted=granted,
            permissions_missing=missing,
            eligible=eligible,
            actionable_remedy=remedy,
            platform_supported=True,
            platform_notes=f"Supported on {current_plat} via isolated in-memory configuration.",
        )

    def create_deployment_plan(
        self,
        inspection: AccountInspectionResult,
        installation_id: str,
        options: Optional[Dict[str, Any]] = None,
    ) -> DeploymentPlan:
        """Create deterministic deployment plan for Beam."""
        if not inspection.platform_supported:
            raise ProtocolError(
                ERR_PLATFORM_GATED,
                f"Cannot create deployment plan: {inspection.platform_notes}",
                actionable_guidance=inspection.actionable_remedy,
                remedy_steps=[
                    "Run deployment inside a WSL 2 (Ubuntu 22.04) environment",
                    "Or use 'Connect an existing deployment' with manual endpoint URL",
                ],
            )

        if not inspection.eligible:
            raise ProtocolError(
                ERR_ACTIONABLE_PERMISSION,
                f"Cannot create deployment plan: account is missing required permissions: {inspection.permissions_missing}",
                actionable_guidance=inspection.actionable_remedy,
                remedy_steps=[
                    "Open Beam Dashboard at https://beam.cloud/dashboard",
                    "Navigate to Settings > API Keys",
                    "Update token permissions to include deployment and token creation",
                ],
            )

        service_name = f"mc-beam-svc-{installation_id}"
        vol_name = f"mc-beam-vol-{installation_id}"
        token_name = f"b9-tok-{installation_id}"

        resources_to_create = [
            {
                "name": vol_name,
                "type": "volume",
                "purpose": "Model weights and cache storage",
                "tags": {"installation_id": installation_id, "managed_by": "manga-cleaner"},
            },
            {
                "name": service_name,
                "type": "service",
                "purpose": "CPU control service and task queue gateway",
                "tags": {"installation_id": installation_id, "managed_by": "manga-cleaner"},
            },
            {
                "name": token_name,
                "type": "restricted_token",
                "purpose": "Workspace-restricted runtime credential for inference calls",
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
            f"Delete restricted runtime token '{token_name}'",
            f"Delete deployed service '{service_name}'",
            f"Delete storage volume '{vol_name}' (requires explicit approval)",
            "Verify zero mutations to ~/.beam/config.ini",
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
        """Seed Beam volume; avoid duplicates if already created."""
        if self.simulate_failure_at_step == "seed":
            raise ProtocolError(
                ERR_EXECUTION_FAILED,
                "Simulated failure during Beam volume creation",
            )

        vol_name = f"mc-beam-vol-{installation_id}"
        for res_id, res in self.cloud_store.items():
            if res.get("name") == vol_name and res.get("type") == "volume":
                return res

        vol_id = f"vol_beam_{installation_id}"
        res = {
            "resource_id": vol_id,
            "resource_type": "volume",
            "name": vol_name,
            "provider": "beam",
            "status": "ready",
            "ownership_tags": {
                "installation_id": installation_id,
                "managed_by": "manga-cleaner",
            },
        }
        self.cloud_store[vol_id] = res
        return res

    def deploy_service(
        self,
        installation_id: str,
        app_name: str,
        credentials: Dict[str, Any],
    ) -> Dict[str, Any]:
        """Deploy Beam service; reuse existing if already deployed."""
        if self.simulate_failure_at_step == "deploy":
            raise ProtocolError(
                ERR_EXECUTION_FAILED,
                "Simulated failure during Beam service deployment",
            )

        for res_id, res in self.cloud_store.items():
            if res.get("name") == app_name and res.get("type") == "service":
                return res

        svc_id = f"svc_beam_{installation_id}"
        res = {
            "resource_id": svc_id,
            "resource_type": "service",
            "name": app_name,
            "provider": "beam",
            "status": "deployed",
            "ownership_tags": {
                "installation_id": installation_id,
                "managed_by": "manga-cleaner",
            },
        }
        self.cloud_store[svc_id] = res
        return res

    def discover_endpoint(
        self,
        installation_id: str,
        app_name: str,
        credentials: Dict[str, Any],
    ) -> str:
        """Discover Beam gateway endpoint."""
        if self.simulate_failure_at_step == "endpoint":
            raise ProtocolError(
                ERR_EXECUTION_FAILED,
                "Simulated failure during Beam endpoint discovery",
            )
        return f"https://gateway.beam.cloud/v1/apps/{installation_id}"

    def create_runtime_credential(
        self,
        installation_id: str,
        credentials: Dict[str, Any],
    ) -> Dict[str, Any]:
        """Create restricted runtime token."""
        if self.simulate_failure_at_step == "runtime_credential":
            raise ProtocolError(
                ERR_EXECUTION_FAILED,
                "Simulated failure during restricted token creation",
            )

        tok_id = f"tok_beam_{installation_id}"
        token_name = f"b9-tok-{installation_id}"
        res = {
            "resource_id": tok_id,
            "resource_type": "restricted_token",
            "name": token_name,
            "provider": "beam",
            "token": f"b9_restricted_token_{installation_id}",
            "token_type": "workspace_restricted",
            "ownership_tags": {
                "installation_id": installation_id,
                "managed_by": "manga-cleaner",
            },
        }
        self.cloud_store[tok_id] = res
        return res

    def validate_compatibility(
        self,
        endpoint_url: str,
        runtime_credential: Dict[str, Any],
    ) -> CompatibilityValidationResult:
        """Probe GET /health and GET /model-info."""
        validate_https_url("endpoint_url", endpoint_url)
        if self.simulate_failure_at_step == "validate":
            return CompatibilityValidationResult(
                endpoint_url=endpoint_url,
                status="unhealthy",
                api_version="1.0.0",
                model_recipe="unknown",
                healthy=False,
                gpu_incurred=False,
                latency_ms=150.0,
                details={"error": "Beam gateway returned HTTP 502 Bad Gateway"},
            )

        return CompatibilityValidationResult(
            endpoint_url=endpoint_url,
            status="healthy",
            api_version="1.0.0",
            model_recipe="flux-crop-sdnq-v1",
            healthy=True,
            gpu_incurred=False,
            latency_ms=52.0,
            details={"provider": "beam", "verified_offline": True},
        )

    def create_cleanup_plan(
        self,
        installation_id: str,
        known_resources: List[Dict[str, Any]],
    ) -> CleanupPlan:
        """Construct ownership-verified cleanup plan for Beam."""
        to_delete = []
        foreign_ignored = []

        all_res = list(known_resources)
        for s_id, s_res in self.cloud_store.items():
            if not any(r.get("resource_id") == s_id for r in all_res):
                all_res.append(s_res)

        for r in all_res:
            tags = r.get("ownership_tags", {})
            r_inst = tags.get("installation_id")
            managed_by = tags.get("managed_by")

            if r_inst == installation_id and managed_by == "manga-cleaner":
                to_delete.append({
                    "resource_id": r.get("resource_id"),
                    "resource_type": r.get("resource_type"),
                    "name": r.get("name"),
                    "verified_ownership": True,
                })
            else:
                foreign_ignored.append({
                    "resource_id": r.get("resource_id"),
                    "resource_type": r.get("resource_type"),
                    "name": r.get("name"),
                    "reason": "Foreign resource or missing manga-cleaner tag",
                })

        now = _utc_now()
        plan_dict = {
            "provider": "beam",
            "installation_id": installation_id,
            "resources_to_delete": to_delete,
            "foreign_resources_ignored": foreign_ignored,
            "persistent_storage_requires_explicit_confirmation": True,
        }
        plan_hash = compute_canonical_hash(plan_dict)

        return CleanupPlan(
            plan_id=f"cleanup-beam-{installation_id}",
            installation_id=installation_id,
            provider="beam",
            resources_to_delete=to_delete,
            foreign_resources_ignored=foreign_ignored,
            persistent_storage_requires_explicit_confirmation=True,
            plan_hash=plan_hash,
            created_at_utc=now,
        )

    def execute_cleanup(
        self,
        cleanup_plan: CleanupPlan,
        credentials: Dict[str, Any],
    ) -> Dict[str, Any]:
        """Delete only verified owned Beam resources."""
        deleted = []
        for item in cleanup_plan.resources_to_delete:
            r_id = item.get("resource_id")
            if r_id in self.cloud_store:
                del self.cloud_store[r_id]
                deleted.append(r_id)
        return {
            "deleted_resources": deleted,
            "total_deleted": len(deleted),
            "status": "cleaned_up",
        }
