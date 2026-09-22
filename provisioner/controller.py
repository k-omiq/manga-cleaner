"""Provisioning controller and request orchestrator.

Coordinates:
- Request parsing and protocol validation.
- Driver dispatching (Modal vs Beam).
- Resumable staged execution with journal persistence at each milestone.
- Approved plan enforcement (plan hash verification).
- Non-billable compatibility validation.
- Setup credential wiping / forgetting.
- Ownership-verified cleanup.
"""

from pathlib import Path
from typing import Any, Dict, Optional, Union

from provisioner.driver_base import BaseProviderDriver
from provisioner.real_drivers import RealBeamDriver, RealModalDriver
from provisioner.journal import (
    STAGE_CLEANED_UP,
    STAGE_CLEANUP_PLANNED,
    STAGE_COMPLETED,
    STAGE_CREDENTIAL_CREATED,
    STAGE_DEPLOYED,
    STAGE_DEPLOYING,
    STAGE_DISCOVERED,
    STAGE_FAILED,
    STAGE_PLANNED,
    STAGE_SEEDED,
    STAGE_SEEDING,
    STAGE_VALIDATED,
    InstallationJournal,
)
from provisioner.protocol import (
    ERR_ACTIONABLE_PERMISSION,
    ERR_EXECUTION_FAILED,
    ERR_UNAPPROVED_PLAN,
    ERR_UNSUPPORTED_OP,
    ERR_UNSUPPORTED_PROVIDER,
    ERR_VALIDATION,
    HelperRequest,
    HelperResponse,
    OP_APPLY,
    OP_CLEANUP_APPLY,
    OP_CLEANUP_PLAN,
    OP_FORGET_CREDENTIAL,
    OP_INSPECT,
    OP_PLAN,
    OP_PROBE_COMPATIBILITY,
    OP_RESUME,
    PROVIDER_BEAM,
    PROVIDER_MODAL,
    ProtocolError,
    make_error_response,
    make_success_response,
    parse_request,
    validate_hash,
    validate_https_url,
    validate_identifier,
)
from provisioner.redaction import GLOBAL_REGISTRY, is_sensitive_key, redact_data


class ProvisioningController:
    """Orchestrates helper protocol requests and cloud driver operations."""

    def __init__(
        self,
        journal_root: Path,
        modal_driver: Optional[BaseProviderDriver] = None,
        beam_driver: Optional[BaseProviderDriver] = None,
    ) -> None:
        self.journal_root = journal_root.resolve()
        self.modal_driver = modal_driver if modal_driver is not None else RealModalDriver()
        self.beam_driver = beam_driver if beam_driver is not None else RealBeamDriver()

    def get_driver(self, provider: str) -> BaseProviderDriver:
        if provider == PROVIDER_MODAL:
            return self.modal_driver
        elif provider == PROVIDER_BEAM:
            return self.beam_driver
        else:
            raise ProtocolError(
                ERR_UNSUPPORTED_PROVIDER,
                f"Unknown provider: '{provider}'",
            )

    def handle_request(self, raw_request: Union[str, bytes, Dict[str, Any]]) -> str:
        """Entry point for JSON/dict helper protocol messages. Returns serialized redacted response."""
        request_id = "req-unknown"
        try:
            req = parse_request(raw_request)
            request_id = req.request_id

            # Register any supplied credentials in session redaction registry
            self._register_credentials_from_params(req.params)

            if req.op == OP_INSPECT:
                response = self._handle_inspect(req)
            elif req.op == OP_PLAN:
                response = self._handle_plan(req)
            elif req.op == OP_APPLY:
                response = self._handle_apply(req)
            elif req.op == OP_RESUME:
                response = self._handle_resume(req)
            elif req.op == OP_FORGET_CREDENTIAL:
                response = self._handle_forget_credential(req)
            elif req.op == OP_CLEANUP_PLAN:
                response = self._handle_cleanup_plan(req)
            elif req.op == OP_CLEANUP_APPLY:
                response = self._handle_cleanup_apply(req)
            elif req.op == OP_PROBE_COMPATIBILITY:
                response = self._handle_probe_compatibility(req)
            else:
                raise ProtocolError(
                    ERR_UNSUPPORTED_OP,
                    f"Unsupported operation: '{req.op}'",
                )

            return response.serialize()

        except ProtocolError as e:
            err_resp = make_error_response(
                request_id=request_id,
                code=e.code,
                message=e.message,
                actionable_guidance=e.actionable_guidance,
                remedy_steps=e.remedy_steps,
            )
            return err_resp.serialize()
        except Exception as e:
            err_resp = make_error_response(
                request_id=request_id,
                code=ERR_EXECUTION_FAILED,
                message=f"Internal provisioner error: {e}",
            )
            return err_resp.serialize()

    def _register_credentials_from_params(self, params: Dict[str, Any]) -> None:
        def _extract(obj: Any, key_name: str = "") -> None:
            if isinstance(obj, str):
                if is_sensitive_key(key_name) or key_name == "credentials":
                    GLOBAL_REGISTRY.register(obj)
            elif isinstance(obj, dict):
                for k, v in obj.items():
                    str_k = str(k)
                    if str_k.lower() == "credentials":
                        _extract_all(v)
                    elif is_sensitive_key(str_k):
                        if isinstance(v, str):
                            GLOBAL_REGISTRY.register(v)
                        else:
                            _extract(v, str_k)
                    else:
                        _extract(v, str_k)
            elif isinstance(obj, (list, tuple, set)):
                for item in obj:
                    _extract(item, key_name)

        def _extract_all(obj: Any) -> None:
            if isinstance(obj, str):
                GLOBAL_REGISTRY.register(obj)
            elif isinstance(obj, dict):
                for v in obj.values():
                    _extract_all(v)
            elif isinstance(obj, (list, tuple, set)):
                for item in obj:
                    _extract_all(item)

        _extract(params)

    def _handle_inspect(self, req: HelperRequest) -> HelperResponse:
        driver = self.get_driver(req.provider)
        credentials = req.params.get("credentials", {})
        result = driver.inspect_account(credentials, req.params.get("options"))
        return make_success_response(req.request_id, result.to_dict())

    def _handle_plan(self, req: HelperRequest) -> HelperResponse:
        driver = self.get_driver(req.provider)
        credentials = req.params.get("credentials", {})
        installation_id = req.params.get("installation_id")
        if not installation_id:
            raise ProtocolError(ERR_VALIDATION, "Missing parameter: 'installation_id'")
        validate_identifier("installation_id", installation_id)

        inspection = driver.inspect_account(credentials, req.params.get("options"))
        plan = driver.create_deployment_plan(inspection, installation_id, req.params.get("options"))

        # Initialize or update journal
        journal = InstallationJournal(self.journal_root, installation_id, req.provider)
        journal.load_or_initialize(
            plan_hash=plan.plan_hash,
            approved_plan_hash="",
            app_name=plan.app_name,
        )

        return make_success_response(req.request_id, plan.to_dict())

    def _handle_apply(self, req: HelperRequest) -> HelperResponse:
        driver = self.get_driver(req.provider)
        credentials = req.params.get("credentials", {})
        installation_id = req.params.get("installation_id")
        approved_hash = req.params.get("approved_plan_hash")

        if not installation_id:
            raise ProtocolError(ERR_VALIDATION, "Missing parameter: 'installation_id'")
        validate_identifier("installation_id", installation_id)

        if not approved_hash:
            raise ProtocolError(
                ERR_UNAPPROVED_PLAN,
                "Execution rejected: 'approved_plan_hash' parameter is required to proceed with deployment",
                actionable_guidance="Review the proposed deployment plan and supply its exact SHA-256 hash to approve.",
            )
        validate_hash("approved_plan_hash", approved_hash)

        # Generate fresh expected plan
        inspection = driver.inspect_account(credentials, req.params.get("options"))
        plan = driver.create_deployment_plan(inspection, installation_id, req.params.get("options"))

        if approved_hash.lower() != plan.plan_hash.lower():
            raise ProtocolError(
                ERR_UNAPPROVED_PLAN,
                f"Approved plan hash mismatch: got '{approved_hash}', expected '{plan.plan_hash}'",
                actionable_guidance="The proposed resource plan was modified or generated with different parameters. Review and re-approve.",
            )

        # Load or initialize journal
        journal = InstallationJournal(self.journal_root, installation_id, req.provider)
        if journal.exists():
            rec = journal.load()
            if rec.provider != req.provider:
                raise ProtocolError(
                    ERR_VALIDATION,
                    f"Existing journal provider mismatch: stored '{rec.provider}', requested '{req.provider}'",
                )
            if rec.app_name and rec.app_name != plan.app_name:
                raise ProtocolError(
                    ERR_VALIDATION,
                    f"Existing journal app_name mismatch: stored '{rec.app_name}', expected '{plan.app_name}'",
                )
            if rec.stage in {STAGE_COMPLETED, STAGE_CLEANED_UP}:
                raise ProtocolError(
                    ERR_VALIDATION,
                    f"Cannot apply: installation '{installation_id}' is already in terminal stage '{rec.stage}'",
                    actionable_guidance="Completed or cleaned-up installations cannot be re-applied. Create a new deployment with a new unique installation ID, or use resume/cleanup.",
                    remedy_steps=[
                        f"Installation '{installation_id}' is in terminal stage '{rec.stage}'",
                        "Supply a new unique installation_id in params to deploy a new instance",
                    ],
                )
            if rec.plan_hash and rec.plan_hash.lower() != plan.plan_hash.lower():
                raise ProtocolError(
                    ERR_UNAPPROVED_PLAN,
                    f"Plan hash mismatch with existing journal: stored '{rec.plan_hash}', expected '{plan.plan_hash}'",
                    actionable_guidance="A different deployment plan is already recorded for this installation. Re-plan or use a new installation ID.",
                )
            if rec.approved_plan_hash and rec.approved_plan_hash.lower() != approved_hash.lower():
                raise ProtocolError(
                    ERR_UNAPPROVED_PLAN,
                    f"Approved plan hash mismatch with existing journal: stored '{rec.approved_plan_hash}', expected '{approved_hash}'",
                    actionable_guidance="An approved plan is already recorded with a different hash. Re-plan or use a new installation ID.",
                )
            rec.approved_plan_hash = approved_hash
            journal.save()
        else:
            rec = journal.load_or_initialize(
                plan_hash=plan.plan_hash,
                approved_plan_hash=approved_hash,
                app_name=plan.app_name,
            )
            rec.approved_plan_hash = approved_hash
            journal.save()

        # Run staged pipeline
        return self._run_staged_pipeline(journal, driver, plan, credentials, req)

    def _handle_resume(self, req: HelperRequest) -> HelperResponse:
        driver = self.get_driver(req.provider)
        credentials = req.params.get("credentials", {})
        installation_id = req.params.get("installation_id")

        if not installation_id:
            raise ProtocolError(ERR_VALIDATION, "Missing parameter: 'installation_id'")
        validate_identifier("installation_id", installation_id)

        journal = InstallationJournal(self.journal_root, installation_id, req.provider)
        if not journal.exists():
            raise ProtocolError(
                ERR_VALIDATION,
                f"No existing installation journal found for '{installation_id}' to resume",
            )

        rec = journal.load()
        if not journal.can_resume():
            return make_success_response(
                req.request_id,
                {
                    "installation_id": installation_id,
                    "stage": rec.stage,
                    "resumed": False,
                    "message": f"Installation is already in stage '{rec.stage}'",
                    "record": rec.to_dict(),
                },
            )

        # Invariant: Must have non-empty approved_plan_hash before resuming
        if not rec.approved_plan_hash:
            raise ProtocolError(
                ERR_UNAPPROVED_PLAN,
                f"Cannot resume installation '{installation_id}': no approved plan hash recorded in journal",
                actionable_guidance="Approve the deployment plan via apply operation before attempting resume.",
            )

        # Inspect and regenerate expected plan
        inspection = driver.inspect_account(credentials, req.params.get("options"))
        plan = driver.create_deployment_plan(inspection, installation_id, req.params.get("options"))

        # Compare regenerated plan against both stored plan_hash and approved_plan_hash before ANY driver mutation
        if plan.plan_hash.lower() != rec.plan_hash.lower():
            raise ProtocolError(
                ERR_UNAPPROVED_PLAN,
                f"Plan drift detected on resume: regenerated plan hash '{plan.plan_hash}' does not match stored plan hash '{rec.plan_hash}'",
                actionable_guidance="The deployment options or parameters have changed since the plan was recorded. Review and re-approve.",
            )

        if plan.plan_hash.lower() != rec.approved_plan_hash.lower():
            raise ProtocolError(
                ERR_UNAPPROVED_PLAN,
                f"Plan drift detected on resume: regenerated plan hash '{plan.plan_hash}' does not match approved plan hash '{rec.approved_plan_hash}'",
                actionable_guidance="The regenerated deployment plan does not match the approved plan hash. Review and re-approve.",
            )

        return self._run_staged_pipeline(journal, driver, plan, credentials, req)

    def _run_staged_pipeline(
        self,
        journal: InstallationJournal,
        driver: BaseProviderDriver,
        plan: Any,
        credentials: Dict[str, Any],
        req: HelperRequest,
    ) -> HelperResponse:
        installation_id = journal.installation_id
        app_name = plan.app_name
        rec = journal.record

        try:
            # Stage 1: Seed Volume (if not already seeded)
            vol_name = f"mc-{driver.provider_id}-vol-{installation_id}"
            if not journal.has_resource("volume", vol_name):
                journal.transition_to(STAGE_SEEDING)
                vol_res = driver.seed_volume(installation_id, credentials)
                journal.record_resource(
                    resource_id=vol_res["resource_id"],
                    resource_type="volume",
                    name=vol_res["name"],
                    stage_created=STAGE_SEEDING,
                    ownership_tags=vol_res.get("ownership_tags"),
                )
                journal.transition_to(STAGE_SEEDED)

            # Stage 2: Deploy Service (if not already deployed)
            app_type = "app" if driver.provider_id == "modal" else "service"
            if not journal.has_resource(app_type, app_name):
                journal.transition_to(STAGE_DEPLOYING)
                svc_res = driver.deploy_service(installation_id, app_name, credentials)
                journal.record_resource(
                    resource_id=svc_res["resource_id"],
                    resource_type=app_type,
                    name=svc_res["name"],
                    stage_created=STAGE_DEPLOYING,
                    ownership_tags=svc_res.get("ownership_tags"),
                )
                journal.transition_to(STAGE_DEPLOYED)

            # Stage 3: Endpoint Discovery
            if not rec.endpoint_url:
                endpoint_url = driver.discover_endpoint(installation_id, app_name, credentials)
                validate_https_url("endpoint_url", endpoint_url)
                rec.endpoint_url = endpoint_url
                journal.transition_to(STAGE_DISCOVERED)

            # Stage 4: Runtime Credential Creation
            cred_type = "proxy_token" if driver.provider_id == "modal" else "restricted_token"
            if not journal.has_resource(cred_type, f"tok-proxy-{installation_id}") and not journal.has_resource(
                cred_type, f"b9-tok-{installation_id}"
            ):
                tok_res = driver.create_runtime_credential(installation_id, credentials)
                journal.record_resource(
                    resource_id=tok_res["resource_id"],
                    resource_type=cred_type,
                    name=tok_res["name"],
                    stage_created=STAGE_DISCOVERED,
                    ownership_tags=tok_res.get("ownership_tags"),
                )
                # Store opaque runtime reference in journal (NEVER raw secret)
                rec.runtime_credential_ref = {
                    "resource_id": tok_res["resource_id"],
                    "resource_type": cred_type,
                    "name": tok_res["name"],
                    "token_type": tok_res.get("token_type", "scoped"),
                }
                journal.transition_to(STAGE_CREDENTIAL_CREATED)

            # Stage 5: Compatibility Validation
            if rec.compatibility_status != "healthy":
                validate_https_url("endpoint_url", rec.endpoint_url)
                runtime_cred = rec.runtime_credential_ref or {}
                compat = driver.validate_compatibility(rec.endpoint_url, runtime_cred)
                if not compat.healthy:
                    rec.compatibility_status = compat.status
                    journal.transition_to(STAGE_FAILED, error="Compatibility probe failed")
                    return make_error_response(
                        request_id=req.request_id,
                        code=ERR_EXECUTION_FAILED,
                        message=f"Endpoint compatibility validation failed: {compat.details}",
                    )
                rec.compatibility_status = "healthy"
                journal.transition_to(STAGE_VALIDATED)

            # Stage 6: Finalize Completion
            journal.transition_to(STAGE_COMPLETED)

            # Optional: Forget setup credential if requested
            if req.params.get("forget_setup_credential", False):
                journal.forget_setup_credential()
                GLOBAL_REGISTRY.clear()

            return make_success_response(
                req.request_id,
                {
                    "installation_id": installation_id,
                    "stage": rec.stage,
                    "endpoint_url": rec.endpoint_url,
                    "runtime_credential_ref": rec.runtime_credential_ref,
                    "compatibility_status": rec.compatibility_status,
                    "setup_credential_forgotten": rec.setup_credential_forgotten,
                    "resources_created": len(rec.resources),
                },
            )

        except ProtocolError as pe:
            journal.transition_to(STAGE_FAILED, error=pe.message)
            raise
        except Exception as e:
            journal.transition_to(STAGE_FAILED, error=str(e))
            raise

    def _handle_forget_credential(self, req: HelperRequest) -> HelperResponse:
        """Purge setup credentials from memory registry and mark journal."""
        GLOBAL_REGISTRY.clear()
        installation_id = req.params.get("installation_id")
        if installation_id:
            validate_identifier("installation_id", installation_id)
            journal = InstallationJournal(self.journal_root, installation_id, req.provider)
            if journal.exists():
                journal.forget_setup_credential()

        return make_success_response(
            req.request_id,
            {
                "status": "forgotten",
                "setup_credential_cleared": True,
            },
        )

    def _handle_cleanup_plan(self, req: HelperRequest) -> HelperResponse:
        driver = self.get_driver(req.provider)
        installation_id = req.params.get("installation_id")
        if not installation_id:
            raise ProtocolError(ERR_VALIDATION, "Missing parameter: 'installation_id'")
        validate_identifier("installation_id", installation_id)

        journal = InstallationJournal(self.journal_root, installation_id, req.provider)
        known_resources = [r.to_dict() for r in journal.record.resources] if journal.exists() else []

        cleanup_plan = driver.create_cleanup_plan(installation_id, known_resources)
        if journal.exists():
            journal.transition_to(STAGE_CLEANUP_PLANNED)

        return make_success_response(req.request_id, cleanup_plan.to_dict())

    def _handle_cleanup_apply(self, req: HelperRequest) -> HelperResponse:
        driver = self.get_driver(req.provider)
        credentials = req.params.get("credentials", {})
        installation_id = req.params.get("installation_id")
        approved_hash = req.params.get("approved_cleanup_plan_hash")

        if not installation_id:
            raise ProtocolError(ERR_VALIDATION, "Missing parameter: 'installation_id'")
        validate_identifier("installation_id", installation_id)

        if not approved_hash:
            raise ProtocolError(
                ERR_UNAPPROVED_PLAN,
                "Cleanup rejected: 'approved_cleanup_plan_hash' is required to execute deletion",
            )
        validate_hash("approved_cleanup_plan_hash", approved_hash)

        journal = InstallationJournal(self.journal_root, installation_id, req.provider)
        known_resources = [r.to_dict() for r in journal.record.resources] if journal.exists() else []

        cleanup_plan = driver.create_cleanup_plan(installation_id, known_resources)
        if approved_hash.lower() != cleanup_plan.plan_hash.lower():
            raise ProtocolError(
                ERR_UNAPPROVED_PLAN,
                f"Approved cleanup plan hash mismatch: got '{approved_hash}', expected '{cleanup_plan.plan_hash}'",
            )

        # Enforce explicit boolean confirmation for persistent storage volumes before any mutation
        if cleanup_plan.persistent_storage_requires_explicit_confirmation:
            has_volume = any(r.get("resource_type") == "volume" for r in cleanup_plan.resources_to_delete)
            if has_volume and req.params.get("confirm_delete_persistent_storage") is not True:
                raise ProtocolError(
                    ERR_VALIDATION,
                    "Cleanup rejected: deletion of persistent storage volume requires explicit boolean parameter: 'confirm_delete_persistent_storage=true'",
                    actionable_guidance="Set 'confirm_delete_persistent_storage': true in params to confirm volume deletion.",
                    remedy_steps=[
                        "Confirm that stored model data and cache can be permanently destroyed.",
                        "Resubmit cleanup_apply request with 'confirm_delete_persistent_storage': true.",
                    ],
                )

        if journal.exists():
            journal.transition_to(STAGE_CLEANUP_PLANNED)

        try:
            result = driver.execute_cleanup(cleanup_plan, credentials)
            if journal.exists():
                journal.transition_to(STAGE_CLEANED_UP)
            return make_success_response(req.request_id, result)
        except ProtocolError as pe:
            if journal.exists():
                journal.transition_to(STAGE_FAILED, error=pe.message)
            raise
        except Exception as e:
            if journal.exists():
                journal.transition_to(STAGE_FAILED, error=str(e))
            raise

    def _handle_probe_compatibility(self, req: HelperRequest) -> HelperResponse:
        driver = self.get_driver(req.provider)
        endpoint_url = req.params.get("endpoint_url")
        runtime_credential = req.params.get("runtime_credential", {})

        validate_https_url("endpoint_url", endpoint_url)

        res = driver.validate_compatibility(endpoint_url, runtime_credential)
        return make_success_response(req.request_id, res.to_dict())
