"""Concrete minimal CPU-only staging resource and cleanup plans.

Defines non-mutating feasibility specifications for Modal and Beam:
- Strictly CPU-only (zero GPU allocation, zero model weights, zero GPU idle retention).
- Strictly isolated auth (zero writes to ~/.modal.toml or ~/.beam/config.ini).
- Real account checks are P1 PREREQUISITES, not deferred to P5–P8.
- NO CLEANUP GUARANTEE UNTIL OBSERVED: All teardown procedures are unobserved
  and unverified until executed and validated against an authorized live test account.
- Handler docstrings do NOT prove authentication; authentication requires live credential exchange.
"""

from dataclasses import asdict, dataclass, field
from typing import Any, Dict, List, Optional


@dataclass(frozen=True)
class ResourceAllocation:
    cpu: float
    memory_mb: int
    gpu: Optional[str]
    model_weights_required: bool
    endpoints: List[str]


@dataclass(frozen=True)
class AuthSpecification:
    provider: str
    credential_source: str
    global_file_write_permitted: bool
    in_memory_injection_method: str
    audit_notes: List[str]


@dataclass(frozen=True)
class TokenLifecyclePlan:
    token_type: str
    creation_method: str
    scoping_method: str
    verification_steps: List[str]
    revocation_method: str
    deletion_method: str
    hosted_risk_disclosure: str


@dataclass(frozen=True)
class CleanupStep:
    sequence: int
    action: str
    command_or_api: str
    verification_check: str
    observed_in_production: bool = False


@dataclass(frozen=True)
class ProviderStagingPlan:
    provider_id: str
    plan_name: str
    resource: ResourceAllocation
    auth: AuthSpecification
    token_lifecycle: TokenLifecyclePlan
    cleanup_steps: List[CleanupStep]
    p1_prerequisite_gates: List[str]
    explicit_blockers: List[str]
    cleanup_guarantee: str = "Unobserved and unverified until executed on live account"


# Modal Minimal CPU-Only Staging Plan
MODAL_STAGING_PLAN = ProviderStagingPlan(
    provider_id="modal",
    plan_name="modal-cpu-offline-staging-v1",
    resource=ResourceAllocation(
        cpu=0.125,
        memory_mb=128,
        gpu=None,
        model_weights_required=False,
        endpoints=["/mc/v1/health"],
    ),
    auth=AuthSpecification(
        provider="modal",
        credential_source="In-memory credentials passed to client constructor (MODAL_TOKEN_ID, MODAL_TOKEN_SECRET)",
        global_file_write_permitted=False,
        in_memory_injection_method="modal.Client.from_credentials(token_id, token_secret)",
        audit_notes=[
            "Pass explicit Client instance to App.deploy(client=client) and Workspace.from_context(client=client).",
            "Do NOT run `modal token set` or write to ~/.modal.toml.",
            "Client.from_credentials isolates session authentication strictly in memory.",
        ],
    ),
    token_lifecycle=TokenLifecyclePlan(
        token_type="modal_proxy_token",
        creation_method="workspace.proxy_tokens.create() -> TokenData",
        scoping_method="workspace.proxy_tokens.allow(proxy_token_id, environment_name)",
        verification_steps=[
            "Invoke GET /mc/v1/health with proxy token credentials via auth headers.",
            "Negative test: attempt invocation against unauthorized environment or non-whitelisted endpoint.",
            "Verify HTTP 401/403 refusal on unauthorized access.",
        ],
        revocation_method="workspace.proxy_tokens.revoke(proxy_token_id, environment_name)",
        deletion_method="workspace.proxy_tokens.delete(proxy_token_id)",
        hosted_risk_disclosure=(
            "Source code confirms proxy_tokens API exists on Workspace; however, actual server-side "
            "header validation against web endpoints must be proven on an authorized test account as a P1 prerequisite."
        ),
    ),
    cleanup_steps=[
        CleanupStep(
            sequence=1,
            action="Revoke and delete test proxy token",
            command_or_api="workspace.proxy_tokens.delete(proxy_token_id)",
            verification_check="Assert proxy_token_id not in [t.token_id for t in workspace.proxy_tokens.list()]",
            observed_in_production=False,
        ),
        CleanupStep(
            sequence=2,
            action="Stop deployed staging app",
            command_or_api="client.app_stop(app_id) or app.stop()",
            verification_check="Assert app status is stopped/deleted in workspace apps list",
            observed_in_production=False,
        ),
        CleanupStep(
            sequence=3,
            action="Verify zero remaining staging artifacts",
            command_or_api="Inspect active workspace containers and endpoints",
            verification_check="Zero active containers, zero running app instances with prefix 'mc-staging-'",
            observed_in_production=False,
        ),
    ],
    p1_prerequisite_gates=[
        "Real account check: Verify workspace identity and proxy token scoping on an authorized test account before building wizard.",
        "Runtime check: Verify packaging bundles Python >= 3.10 runtime (system Python 3.9.6 is incompatible).",
        "Zero-mutation check: Verify ~/.modal.toml is neither created nor modified during execution.",
        "Cleanup check: Observe and verify complete resource deallocation in Modal dashboard after test run.",
    ],
    explicit_blockers=[
        "System Python on macOS runner is 3.9.6; modal requires >= 3.10 (packaging must bundle Python 3.10+).",
        "Live account verification is a mandatory P1 prerequisite before any deployment wizard implementation.",
    ],
)


# Beam Minimal CPU-Only Staging Plan
BEAM_STAGING_PLAN = ProviderStagingPlan(
    provider_id="beam",
    plan_name="beam-cpu-offline-staging-v1",
    resource=ResourceAllocation(
        cpu=0.5,
        memory_mb=512,
        gpu=None,
        model_weights_required=False,
        endpoints=["/mc/v1/health"],
    ),
    auth=AuthSpecification(
        provider="beam",
        credential_source="In-memory token or session environment variable BEAM_TOKEN",
        global_file_write_permitted=False,
        in_memory_injection_method="beam.client.client.Client(token=...) or SDKSettings(api_token=BEAM_TOKEN)",
        audit_notes=[
            "CRITICAL: Do NOT execute `beam configure` CLI command, as it mutates ~/.beam/config.ini.",
            "CRITICAL: Do NOT execute `beam config list --show-token` or `export` in shared/logged sessions.",
            "SDKSettings respects BEAM_TOKEN environment variable with explicit configuration; global profile read isolation still requires verification.",
        ],
    ),
    token_lifecycle=TokenLifecyclePlan(
        token_type="workspace_restricted",
        creation_method="beta9.cli.token.create_token(token_type='workspace_restricted') / gateway.create_token",
        scoping_method="Token type parameter 'workspace_restricted' passed to CreateTokenRequest",
        verification_steps=[
            "Invoke GET /mc/v1/health on CPU staging deployment using restricted token.",
            "Negative test: attempt deployment creation or workspace modification with restricted token.",
            "Record exact HTTP / gRPC error response to prove hosted lease restriction.",
        ],
        revocation_method="beta9.cli.token.toggle_token(token_id) -> active=False",
        deletion_method="beta9.cli.token.delete_token(token_id)",
        hosted_risk_disclosure=(
            "CRITICAL SECURITY INVARIANT: Never infer hosted token scope from the enum string "
            "'workspace_restricted'. Server-side enforcement of restricted token capabilities vs deployment "
            "powers MUST be empirically verified on an authorized test account as a P1 prerequisite."
        ),
    ),
    cleanup_steps=[
        CleanupStep(
            sequence=1,
            action="Delete deployed staging service",
            command_or_api="beta9.cli.deployment.delete_deployment(service, deployment_id)",
            verification_check="Assert deployment_id not in list_deployments(service)",
            observed_in_production=False,
        ),
        CleanupStep(
            sequence=2,
            action="Delete created restricted token",
            command_or_api="beta9.cli.token.delete_token(service, token_id)",
            verification_check="Assert token_id not in list_tokens(service)",
            observed_in_production=False,
        ),
        CleanupStep(
            sequence=3,
            action="Verify zero global profile modifications",
            command_or_api="Verify ~/.beam/config.ini unchanged or absent",
            verification_check="File hash of ~/.beam/config.ini bit-identical to pre-test baseline",
            observed_in_production=False,
        ),
    ],
    p1_prerequisite_gates=[
        "Real account check: Verify restricted token permissions and denied operations on an authorized test account before building wizard.",
        "Zero-mutation check: Verify ~/.beam/config.ini is neither created nor modified during execution.",
        "Cleanup check: Observe and verify complete resource deallocation in Beam dashboard after test run.",
        "Platform check: Native Windows packaging remains unverified without Windows native execution evidence.",
    ],
    explicit_blockers=[
        "Windows packaging: Beam officially documents Windows ONLY through WSL (Ubuntu 22.04). Native Windows packaging is unverified.",
        "Hosted token restriction: Client enum 'workspace_restricted' is not proof of server-side least privilege.",
        "Live account verification is a mandatory P1 prerequisite before any deployment wizard implementation.",
    ],
)

STAGING_PLANS: Dict[str, ProviderStagingPlan] = {
    "modal": MODAL_STAGING_PLAN,
    "beam": BEAM_STAGING_PLAN,
}
