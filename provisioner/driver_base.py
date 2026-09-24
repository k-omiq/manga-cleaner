"""Provider driver contract and the data the helper returns.

A driver turns one provider SDK into the pipeline steps the controller runs:
authenticate and inspect the account, plan, then create, find and delete the
resources of one installation. Drivers import their SDK lazily, inside a session,
so planning code and tests run without `modal` or `beam` installed.
"""

from abc import ABC, abstractmethod
from contextlib import contextmanager
from dataclasses import asdict, dataclass, field
from datetime import datetime, timezone
import hashlib
import json
import re
import time
from typing import Any, Callable, Dict, Iterator, List, NoReturn, Optional, Sequence, Set, Tuple

from provisioner.progress import StepReporter
from provisioner.protocol import ERR_EXECUTION_FAILED, ERR_EXECUTION_TIMEOUT, ERR_VALIDATION, ProtocolError

# deploy/ is plain Python with no provider SDK imports.
from deploy.cloud.common.deployment import DEFAULT_IDLE_SECONDS, parse_gpu, parse_idle_seconds
from deploy.cloud.common.manifest import (
    PROD_SNAPSHOT_TOTAL_BYTES,
    REVISION_PROD_FLUX,
    get_production_model_info,
)

PRICING_DATE = "2026-09-23"


def compute_canonical_hash(data: Dict[str, Any], fields_to_exclude: Optional[Set[str]] = None) -> str:
    """Compute deterministic SHA-256 hash over canonical JSON representation."""
    exclude = fields_to_exclude or {"plan_hash", "created_at_utc"}
    filtered = {k: v for k, v in data.items() if k not in exclude}
    canonical_json = json.dumps(filtered, sort_keys=True, separators=(",", ":"))
    return hashlib.sha256(canonical_json.encode("utf-8")).hexdigest()


def utc_now() -> str:
    return datetime.now(timezone.utc).isoformat()


_SLUG_REGEX = re.compile(r"^[a-z0-9](?:[a-z0-9-]{0,22}[a-z0-9])?$")


def installation_prefix(installation_id: str) -> str:
    """The name every resource of an installation starts with.

    Short lowercase letters, digits and hyphens, so that it is a valid Modal app,
    volume and dict name, a Beam deployment, volume and map name, and (upper-cased)
    part of an environment variable name. Ids that do not fit are shortened and made
    unique with a hash.
    """
    slug = installation_id
    if not _SLUG_REGEX.match(slug):
        base = re.sub(r"[^a-z0-9]+", "-", installation_id.lower()).strip("-")[:15].strip("-")
        digest = hashlib.sha256(installation_id.encode("utf-8")).hexdigest()[:8]
        slug = f"{base}-{digest}" if base else f"id-{digest}"
    return slug if slug.startswith("mc-") else f"mc-{slug}"


def normalize_options(
    options: Any,
    gpu_allowlist: Sequence[str],
    default_gpu: str,
) -> Dict[str, Any]:
    """The approved knobs: `gpu` from the provider allowlist and `idle_seconds` 60..600."""
    if options is None:
        options = {}
    if not isinstance(options, dict):
        raise ProtocolError(ERR_VALIDATION, "Parameter 'options' must be an object")
    unknown = set(options) - {"gpu", "idle_seconds"}
    if unknown:
        raise ProtocolError(
            ERR_VALIDATION,
            f"Unknown options: {sorted(unknown)}. Supported: gpu, idle_seconds",
        )
    gpu = options.get("gpu")
    idle = options.get("idle_seconds")
    try:
        gpu = parse_gpu(default_gpu if gpu is None else gpu, tuple(gpu_allowlist))
        if isinstance(idle, bool) or (idle is not None and not isinstance(idle, int)):
            raise ValueError("idle_seconds must be a whole number of seconds")
        idle = parse_idle_seconds(DEFAULT_IDLE_SECONDS if idle is None else idle)
    except ValueError as exc:
        raise ProtocolError(
            ERR_VALIDATION,
            f"Invalid options: {exc}",
            actionable_guidance=f"Choose gpu from {', '.join(gpu_allowlist)} and idle_seconds from 60 to 600.",
        ) from None
    return {"gpu": gpu, "idle_seconds": idle}


def production_model() -> Dict[str, str]:
    info = get_production_model_info("modal")
    return {
        "model_id": info.model_id,
        "model_revision": info.model_revision,
        "recipe_id": info.recipe_id,
        "preprocessing_version": info.preprocessing_version,
    }


def read_seed_state(doc: Any) -> Tuple[str, Optional[int], str]:
    """(state, pct, problem) from the seeding status document the seed job publishes.

    state is running, done, failed, or unknown for a missing or foreign document.
    """
    if not isinstance(doc, dict) or doc.get("model_revision") != REVISION_PROD_FLUX:
        return "unknown", None, ""
    state = doc.get("state")
    if state == "done":
        return "done", 100, ""
    if state == "failed":
        code = str(doc.get("error_code") or "weights_download_failed")[:64]
        message = str(doc.get("message") or "")[:512]
        return "failed", None, f"{code}: {message}" if message else code
    if state == "running":
        done, total = doc.get("bytes_done"), doc.get("bytes_total") or PROD_SNAPSHOT_TOTAL_BYTES
        if isinstance(done, int) and isinstance(total, int) and total > 0:
            return "running", max(0, min(99, done * 100 // total)), ""
        return "running", None, ""
    return "unknown", None, ""


# A seeding job writes its status document within seconds of starting and then
# rewrites it, with a new updated_at heartbeat, every few seconds until it ends.
SEED_START_GRACE_SECONDS = 180.0
SEED_STALE_SECONDS = 60.0
# Journal state key: wall-clock second at which this helper last started a seeding job.
SEED_INTENT_KEY = "seed_started_at"


def seed_failed(journal: Any, id_key: str, message: str) -> NoReturn:
    """Forget the seeding job, so the next run starts a new one, and fail the step."""
    journal.set_state(**{id_key: None, SEED_INTENT_KEY: None})
    raise ProtocolError(
        ERR_EXECUTION_FAILED, message, actionable_guidance="Run Resume to download again; finished files are kept."
    )


class SeedWatch:
    """Whether a seeding job the provider cannot report on is gone.

    Used when the job's id was never journaled (the helper stopped between starting
    the job and recording it) or the provider no longer lists the job. The job is gone
    when no running document appeared within SEED_START_GRACE_SECONDS of the journaled
    intent, or when the document kept its heartbeat for SEED_STALE_SECONDS. Silence is
    timed with this helper's monotonic clock, so no clock is compared across machines;
    only the intent, written on this computer, is wall-clock time.
    """

    def __init__(self, intent_at: Any, wall_clock: Callable[[], float], clock: Callable[[], float]) -> None:
        self._intent_at = intent_at if isinstance(intent_at, int) and not isinstance(intent_at, bool) else None
        self._wall_clock = wall_clock
        self._clock = clock
        self._beat: Any = None
        self._since = clock()

    def observe(self, running_doc: Any) -> None:
        """This poll's running status document, or None when there is none."""
        beat = running_doc.get("updated_at") if isinstance(running_doc, dict) else None
        if beat != self._beat:
            self._beat, self._since = beat, self._clock()

    def gone(self) -> bool:
        silent = self._clock() - self._since
        if self._beat is not None:
            return silent >= SEED_STALE_SECONDS
        if self._intent_at is not None:
            silent = max(silent, self._wall_clock() - self._intent_at)
        return silent >= SEED_START_GRACE_SECONDS


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
    # Modal environment the installation lands in; empty for Beam.
    environment_name: str = ""

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
    notes: List[str] = field(default_factory=list)

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


class Deadline:
    """Wall-clock budget of one helper operation (the desktop kills the helper later)."""

    def __init__(self, seconds: float, clock: Callable[[], float] = time.monotonic) -> None:
        self._clock = clock
        self._end = clock() + seconds

    def remaining(self) -> float:
        return max(0.0, self._end - self._clock())

    def expired(self) -> bool:
        return self.remaining() <= 0.0

    def require(self, seconds: float, what: str) -> None:
        if self.remaining() < seconds:
            raise ProtocolError(
                ERR_EXECUTION_TIMEOUT,
                f"Not enough time left to {what}",
                actionable_guidance="Nothing was lost. Run Resume to continue where setup stopped.",
            )


@dataclass
class StepContext:
    """What one pipeline step gets besides the SDK session."""

    journal: Any  # InstallationJournal
    options: Dict[str, Any]
    deadline: Deadline
    reporter: Optional[StepReporter] = None
    # The runtime credential issued in this run (IC-1). Memory only.
    runtime_credential: Optional[Dict[str, str]] = None
    health: Optional[Dict[str, Any]] = None


@dataclass
class PlanResource:
    type: str
    name: str
    description: str

    def to_dict(self) -> Dict[str, str]:
        return {"type": self.type, "name": self.name, "description": self.description}


def build_plan(
    *,
    provider: str,
    installation_id: str,
    inspection: AccountInspectionResult,
    app_name: str,
    options: Dict[str, Any],
    gpu_allowlist: Sequence[str],
    allocation: Dict[str, Any],
    resources: Sequence[PlanResource],
    required_permissions: Sequence[str],
    cost: str,
    notes: Sequence[str],
    cleanup_summary: Sequence[str],
) -> DeploymentPlan:
    model = production_model()
    resource_allocation = {
        "gpu": options["gpu"],
        "gpu_options": list(gpu_allowlist),
        "idle_seconds": options["idle_seconds"],
        "model_weights_bytes": PROD_SNAPSHOT_TOTAL_BYTES,
        "model_id": model["model_id"],
        "model_revision": model["model_revision"],
        **allocation,
    }
    resource_list = [r.to_dict() for r in resources]
    # The hash covers what the plan does, not how it is worded, so a later helper
    # version with better text still resumes an installation approved earlier.
    identity = {
        "provider": provider,
        "installation_id": installation_id,
        "account_id": inspection.account_id,
        "target_environment": inspection.environment_name,
        "app_name": app_name,
        "options": dict(options),
        "resources": [(r.type, r.name) for r in resources],
        "model_revision": model["model_revision"],
        "recipe_id": model["recipe_id"],
    }
    plan_hash = compute_canonical_hash(identity, set())
    return DeploymentPlan(
        plan_id=f"plan-{plan_hash[:16]}",
        installation_id=installation_id,
        provider=provider,
        app_name=app_name,
        target_environment=inspection.environment_name or inspection.workspace_name,
        resource_allocation=resource_allocation,
        resources_to_create=resource_list,
        required_permissions=list(required_permissions),
        estimated_monthly_cost=cost,
        cleanup_plan_summary=list(cleanup_summary),
        plan_hash=plan_hash,
        created_at_utc=utc_now(),
        notes=list(notes),
    )


class BaseProviderDriver(ABC):
    """One provider. The controller owns the journal, progress and error envelope."""

    provider_id: str = ""
    # Pipeline steps this provider performs, in IC-2 order. Others are reported skip.
    pipeline_steps: Tuple[str, ...] = ()
    # Steps repeated by every apply and resume even when the journal marks them done.
    repeat_steps: Tuple[str, ...] = ()
    # Resource types in the order cleanup deletes them: access first, storage last.
    cleanup_order: Tuple[str, ...] = ()

    @abstractmethod
    def parse_credentials(self, raw: Any) -> Any:
        """Validate the pasted setup credential. Raises ERR_VALIDATION."""

    @abstractmethod
    @contextmanager
    def session(self, credentials: Any, deadline: Deadline) -> Iterator[Any]:
        """An authenticated SDK session; raises typed errors when it cannot start."""
        yield None

    @abstractmethod
    def inspect(self, session: Any) -> AccountInspectionResult:
        """Who the credential belongs to."""

    @abstractmethod
    def plan(self, inspection: AccountInspectionResult, installation_id: str, options: Dict[str, Any]) -> DeploymentPlan:
        """The resources apply creates, with honest costs."""

    @abstractmethod
    def normalize_options(self, options: Any) -> Dict[str, Any]:
        """Validated gpu and idle_seconds for this provider."""

    @abstractmethod
    def run_step(self, step: str, session: Any, ctx: StepContext) -> None:
        """Do one pipeline step for the installation in ctx.journal."""

    @abstractmethod
    def issued_credential(self, session: Any, ctx: StepContext) -> Dict[str, str]:
        """The IC-1 runtime credential this run hands to the desktop."""

    @abstractmethod
    def delete_resource(self, session: Any, journal: Any, resource: Any, deadline: Deadline) -> str:
        """Delete one journaled resource: 'deleted' or 'missing'."""

    @abstractmethod
    def probe(self, endpoint_url: str, runtime_credential: Dict[str, Any], deadline: Deadline) -> Dict[str, Any]:
        """GET /health and /model-info with a runtime credential. Never starts a GPU."""
