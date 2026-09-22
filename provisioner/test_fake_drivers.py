"""Deterministic fake provider driver tests covering P7/P8 requirements.

Validates:
1. End-to-end staged provisioning for Modal and Beam.
2. Approved plan hash verification (fails closed on mismatch or absence).
3. Interruption and resume without duplicate resource allocation.
4. Actionable missing privileges diagnostics.
5. Isolated config semantics (Beam never touches ~/.beam/config.ini).
6. Truthful Windows platform gating with WSL / manual fallbacks.
7. Explicit-client semantics for Modal (no ~/.modal.toml).
8. Real SDK imports/actions disabled by default.
9. Forget-setup-credential execution and journal tracking.
10. Ownership-verified cleanup plans protecting foreign resources.
"""

from pathlib import Path
import tempfile
import unittest

from provisioner.controller import ProvisioningController
from provisioner.fake_drivers import FakeBeamDriver, FakeModalDriver
from provisioner.journal import (
    STAGE_CLEANED_UP,
    STAGE_COMPLETED,
    STAGE_CREDENTIAL_CREATED,
    STAGE_DEPLOYED,
    STAGE_DISCOVERED,
    STAGE_PLANNED,
    STAGE_SEEDED,
    InstallationJournal,
)
from provisioner.protocol import (
    ERR_ACTIONABLE_PERMISSION,
    ERR_PLATFORM_GATED,
    ERR_UNAPPROVED_PLAN,
    HELPER_PROTOCOL_VERSION,
    OP_APPLY,
    OP_CLEANUP_APPLY,
    OP_CLEANUP_PLAN,
    OP_FORGET_CREDENTIAL,
    OP_INSPECT,
    OP_PLAN,
    OP_RESUME,
    PROVIDER_BEAM,
    PROVIDER_MODAL,
    ProtocolError,
)
from provisioner.redaction import GLOBAL_REGISTRY


class TestFakeProviderDrivers(unittest.TestCase):
    """Tests for deterministic Modal and Beam fake drivers and controller orchestration."""

    def setUp(self):
        self.temp_dir = tempfile.TemporaryDirectory()
        self.root_path = Path(self.temp_dir.name)
        self.modal_driver = FakeModalDriver()
        self.beam_driver = FakeBeamDriver()
        self.controller = ProvisioningController(
            journal_root=self.root_path,
            modal_driver=self.modal_driver,
            beam_driver=self.beam_driver,
        )
        GLOBAL_REGISTRY.clear()

    def tearDown(self):
        GLOBAL_REGISTRY.clear()
        self.temp_dir.cleanup()

    # --- Modal End-to-End ---

    def test_modal_end_to_end_provisioning(self):
        """Tests full Modal setup pipeline from inspect to completion."""
        inst_id = "inst-modal-e2e"
        creds = {"token_id": "ak-modal-test", "token_secret": "as-modal-test"}

        # 1. Inspect
        insp_res = self.modal_driver.inspect_account(creds)
        self.assertTrue(insp_res.authenticated)
        self.assertTrue(insp_res.eligible)
        self.assertEqual(insp_res.permissions_missing, [])

        # 2. Plan
        plan = self.modal_driver.create_deployment_plan(insp_res, inst_id)
        self.assertEqual(plan.installation_id, inst_id)
        self.assertEqual(plan.app_name, f"mc-app-{inst_id}")
        self.assertEqual(len(plan.resources_to_create), 3)
        self.assertTrue(len(plan.plan_hash) == 64)

        # 3. Apply via Controller with Approved Hash
        req_apply = {
            "protocol_version": HELPER_PROTOCOL_VERSION,
            "request_id": "req-apply-1",
            "op": OP_APPLY,
            "provider": PROVIDER_MODAL,
            "params": {
                "installation_id": inst_id,
                "approved_plan_hash": plan.plan_hash,
                "credentials": creds,
            },
        }
        res_json = self.controller.handle_request(req_apply)
        self.assertIn('"success": true', res_json)
        self.assertIn('"stage": "completed"', res_json)

        # Verify Journal state on disk
        journal = InstallationJournal(self.root_path, inst_id, PROVIDER_MODAL)
        self.assertTrue(journal.exists())
        rec = journal.load()
        self.assertEqual(rec.stage, STAGE_COMPLETED)
        self.assertEqual(len(rec.resources), 3)  # volume, app, token
        self.assertIsNotNone(rec.endpoint_url)
        self.assertEqual(rec.compatibility_status, "healthy")

        # Invariant: Modal explicit client semantics (zero global config)
        self.assertFalse(self.modal_driver.global_config_touched)

    # --- Beam End-to-End ---

    def test_beam_end_to_end_provisioning(self):
        """Tests full Beam setup pipeline on supported platform."""
        inst_id = "inst-beam-e2e"
        creds = {"beam_token": "b9_beam_test_token_12345"}

        # Force platform to macos or linux
        self.beam_driver.platform_override = "darwin"

        # 1. Inspect
        insp_res = self.beam_driver.inspect_account(creds)
        self.assertTrue(insp_res.authenticated)
        self.assertTrue(insp_res.eligible)
        self.assertTrue(insp_res.platform_supported)

        # 2. Plan
        plan = self.beam_driver.create_deployment_plan(insp_res, inst_id)
        self.assertEqual(plan.installation_id, inst_id)
        self.assertEqual(len(plan.plan_hash), 64)

        # 3. Apply via Controller
        req_apply = {
            "protocol_version": HELPER_PROTOCOL_VERSION,
            "request_id": "req-beam-apply",
            "op": OP_APPLY,
            "provider": PROVIDER_BEAM,
            "params": {
                "installation_id": inst_id,
                "approved_plan_hash": plan.plan_hash,
                "credentials": creds,
            },
        }
        res_json = self.controller.handle_request(req_apply)
        self.assertIn('"success": true', res_json)
        self.assertIn('"stage": "completed"', res_json)

        # Invariant: Beam isolated config semantics (never touch ~/.beam/config.ini)
        self.assertFalse(self.beam_driver.beam_config_ini_accessed)

    # --- Plan Approval Verification ---

    def test_apply_rejected_without_approved_hash(self):
        inst_id = "inst-unapproved"
        creds = {"token_id": "ak-test", "token_secret": "as-test"}

        req = {
            "protocol_version": HELPER_PROTOCOL_VERSION,
            "request_id": "req-no-hash",
            "op": OP_APPLY,
            "provider": PROVIDER_MODAL,
            "params": {
                "installation_id": inst_id,
                "credentials": creds,
                # Missing 'approved_plan_hash'
            },
        }
        res_json = self.controller.handle_request(req)
        self.assertIn('"success": false', res_json)
        self.assertIn("ERR_UNAPPROVED_PLAN", res_json)

    def test_apply_rejected_with_mismatched_approved_hash(self):
        inst_id = "inst-mismatch"
        creds = {"token_id": "ak-test", "token_secret": "as-test"}

        req = {
            "protocol_version": HELPER_PROTOCOL_VERSION,
            "request_id": "req-bad-hash",
            "op": OP_APPLY,
            "provider": PROVIDER_MODAL,
            "params": {
                "installation_id": inst_id,
                "approved_plan_hash": "f" * 64,  # wrong hash
                "credentials": creds,
            },
        }
        res_json = self.controller.handle_request(req)
        self.assertIn('"success": false', res_json)
        self.assertIn("ERR_UNAPPROVED_PLAN", res_json)
        self.assertIn("Approved plan hash mismatch", res_json)

    # --- Interruption and Resume Without Duplicates ---

    def test_interruption_at_deploy_and_resumption_no_duplicates(self):
        """Proves interrupted deployment can resume from journal without duplicating resources."""
        inst_id = "inst-interrupt-deploy"
        creds = {"token_id": "ak-test-resume", "token_secret": "as-test-resume"}

        # Simulate failure during 'deploy'
        failing_driver = FakeModalDriver(simulate_failure_at_step="deploy")
        controller = ProvisioningController(
            journal_root=self.root_path,
            modal_driver=failing_driver,
            beam_driver=self.beam_driver,
        )

        # Plan
        insp = failing_driver.inspect_account(creds)
        plan = failing_driver.create_deployment_plan(insp, inst_id)

        # First Apply attempt fails at deploy step
        req_apply = {
            "protocol_version": HELPER_PROTOCOL_VERSION,
            "request_id": "req-failing-1",
            "op": OP_APPLY,
            "provider": PROVIDER_MODAL,
            "params": {
                "installation_id": inst_id,
                "approved_plan_hash": plan.plan_hash,
                "credentials": creds,
            },
        }
        res_fail = controller.handle_request(req_apply)
        self.assertIn('"success": false', res_fail)

        # Verify volume was created before failure
        journal = InstallationJournal(self.root_path, inst_id, PROVIDER_MODAL)
        self.assertTrue(journal.has_resource("volume", f"mc-modal-vol-{inst_id}"))
        self.assertEqual(len(journal.record.resources), 1)
        self.assertEqual(len(failing_driver.cloud_store), 1)

        # Fix the issue (clear failure simulation) and RESUME
        failing_driver.simulate_failure_at_step = None

        req_resume = {
            "protocol_version": HELPER_PROTOCOL_VERSION,
            "request_id": "req-resume-2",
            "op": OP_RESUME,
            "provider": PROVIDER_MODAL,
            "params": {
                "installation_id": inst_id,
                "credentials": creds,
            },
        }
        res_resume = controller.handle_request(req_resume)
        self.assertIn('"success": true', res_resume)
        self.assertIn('"stage": "completed"', res_resume)

        # CRITICAL ASSERTION: Exactly 3 resources total (1 volume, 1 app, 1 token)
        # Volume was NOT recreated or duplicated!
        rec_after = journal.load()
        self.assertEqual(len(rec_after.resources), 3)
        volume_count = sum(1 for r in rec_after.resources if r.resource_type == "volume")
        self.assertEqual(volume_count, 1, "Volume resource was duplicated during resume!")

        cloud_vol_count = sum(
            1 for r in failing_driver.cloud_store.values() if r.get("resource_type") == "volume"
        )
        self.assertEqual(cloud_vol_count, 1, "Cloud store contains duplicated volume!")

    # --- Actionable Missing Privileges ---

    def test_modal_actionable_missing_privilege_reporting(self):
        """Simulate missing proxy_tokens:create and verify actionable error."""
        driver = FakeModalDriver(simulated_missing_permissions={"proxy_tokens:create"})
        creds = {"token_id": "ak-test-missing", "token_secret": "as-test-missing"}

        insp = driver.inspect_account(creds)
        self.assertFalse(insp.eligible)
        self.assertIn("proxy_tokens:create", insp.permissions_missing)
        self.assertIn("Workspace Developer", insp.actionable_remedy)
        self.assertIn("Roles", insp.actionable_remedy)

        # Creating plan fails with actionable permission error
        with self.assertRaises(ProtocolError) as ctx:
            driver.create_deployment_plan(insp, "inst-missing")
        self.assertEqual(ctx.exception.code, ERR_ACTIONABLE_PERMISSION)
        self.assertIn("proxy_tokens:create", ctx.exception.message)
        self.assertIn("https://modal.com/settings", ctx.exception.remedy_steps[0])

    def test_beam_actionable_missing_privilege_reporting(self):
        """Simulate missing deployments:create for Beam and verify actionable error."""
        driver = FakeBeamDriver(
            simulated_missing_permissions={"deployments:create"},
            platform_override="linux",
        )
        creds = {"beam_token": "b9_beam_test"}

        insp = driver.inspect_account(creds)
        self.assertFalse(insp.eligible)
        self.assertIn("deployments:create", insp.permissions_missing)
        self.assertIn("API Keys", insp.actionable_remedy)

        with self.assertRaises(ProtocolError) as ctx:
            driver.create_deployment_plan(insp, "inst-beam-missing")
        self.assertEqual(ctx.exception.code, ERR_ACTIONABLE_PERMISSION)
        self.assertIn("https://beam.cloud/dashboard", ctx.exception.remedy_steps[0])

    # --- Beam Truthful Windows Platform Gating ---

    def test_beam_truthful_windows_platform_gating(self):
        """Proves Beam truthfully gates native Windows setup and provides WSL/manual fallback."""
        driver = FakeBeamDriver(platform_override="windows")
        creds = {"beam_token": "b9_test"}

        insp = driver.inspect_account(creds)
        self.assertFalse(insp.platform_supported)
        self.assertFalse(insp.eligible)
        self.assertIn("unverified and unsupported", insp.actionable_remedy)
        self.assertIn("WSL Setup", insp.actionable_remedy)
        self.assertIn("Manual Endpoint", insp.actionable_remedy)

        # Attempting to plan fails with ERR_PLATFORM_GATED
        with self.assertRaises(ProtocolError) as ctx:
            driver.create_deployment_plan(insp, "inst-win")
        self.assertEqual(ctx.exception.code, ERR_PLATFORM_GATED)
        self.assertIn("WSL 2", ctx.exception.remedy_steps[0])
        self.assertIn("manual endpoint", ctx.exception.remedy_steps[1].lower())

    # --- Real SDK Disabled by Default ---

    def test_real_sdk_disabled_by_default(self):
        self.assertFalse(self.modal_driver.allow_real_sdk)
        self.assertFalse(self.beam_driver.allow_real_sdk)

    # --- Forget Setup Credential ---

    def test_forget_setup_credential_lifecycle(self):
        inst_id = "inst-forget-cycle"
        creds = {"token_id": "ak-forget-123", "token_secret": "as-forget-123"}

        insp = self.modal_driver.inspect_account(creds)
        plan = self.modal_driver.create_deployment_plan(insp, inst_id)

        req_apply = {
            "protocol_version": HELPER_PROTOCOL_VERSION,
            "request_id": "req-forget-apply",
            "op": OP_APPLY,
            "provider": PROVIDER_MODAL,
            "params": {
                "installation_id": inst_id,
                "approved_plan_hash": plan.plan_hash,
                "credentials": creds,
                "forget_setup_credential": True,
            },
        }
        res_json = self.controller.handle_request(req_apply)
        self.assertIn('"success": true', res_json)
        self.assertIn('"setup_credential_forgotten": true', res_json)

        journal = InstallationJournal(self.root_path, inst_id, PROVIDER_MODAL)
        self.assertTrue(journal.load().setup_credential_forgotten)

    # --- Ownership-Verified Cleanup Plans ---

    def test_ownership_verified_cleanup_ignores_foreign_resources(self):
        """Verifies cleanup only removes resources owned by target installation."""
        inst_target = "inst-target-clean"
        inst_foreign = "inst-foreign-user"

        creds = {"token_id": "ak-test", "token_secret": "as-test"}

        # 1. Provision target installation
        insp = self.modal_driver.inspect_account(creds)
        plan = self.modal_driver.create_deployment_plan(insp, inst_target)

        req_apply = {
            "protocol_version": HELPER_PROTOCOL_VERSION,
            "request_id": "req-clean-apply",
            "op": OP_APPLY,
            "provider": PROVIDER_MODAL,
            "params": {
                "installation_id": inst_target,
                "approved_plan_hash": plan.plan_hash,
                "credentials": creds,
            },
        }
        self.controller.handle_request(req_apply)

        # 2. Add foreign resource to mock cloud store
        foreign_id = "app_foreign_999"
        self.modal_driver.cloud_store[foreign_id] = {
            "resource_id": foreign_id,
            "resource_type": "app",
            "name": f"mc-app-{inst_foreign}",
            "ownership_tags": {
                "installation_id": inst_foreign,
                "managed_by": "unrelated-user",
            },
        }

        # 3. Generate Cleanup Plan for target
        req_clean_plan = {
            "protocol_version": HELPER_PROTOCOL_VERSION,
            "request_id": "req-clean-plan",
            "op": OP_CLEANUP_PLAN,
            "provider": PROVIDER_MODAL,
            "params": {
                "installation_id": inst_target,
            },
        }
        res_plan_json = self.controller.handle_request(req_clean_plan)
        self.assertIn('"success": true', res_plan_json)

        journal = InstallationJournal(self.root_path, inst_target, PROVIDER_MODAL)
        cleanup_plan = self.modal_driver.create_cleanup_plan(
            inst_target, [r.to_dict() for r in journal.record.resources]
        )

        # Verify plan: target resources are marked for deletion; foreign resource is ignored
        del_ids = [r["resource_id"] for r in cleanup_plan.resources_to_delete]
        ign_ids = [r["resource_id"] for r in cleanup_plan.foreign_resources_ignored]

        self.assertNotIn(foreign_id, del_ids)
        self.assertIn(foreign_id, ign_ids)

        # 4. Execute Cleanup with approved hash
        req_clean_apply = {
            "protocol_version": HELPER_PROTOCOL_VERSION,
            "request_id": "req-clean-exec",
            "op": OP_CLEANUP_APPLY,
            "provider": PROVIDER_MODAL,
            "params": {
                "installation_id": inst_target,
                "approved_cleanup_plan_hash": cleanup_plan.plan_hash,
                "credentials": creds,
                "confirm_delete_persistent_storage": True,
            },
        }
        res_clean_exec = self.controller.handle_request(req_clean_apply)
        self.assertIn('"success": true', res_clean_exec)
        self.assertEqual(journal.load().stage, STAGE_CLEANED_UP)

        # Invariant: Foreign resource is STILL in cloud store!
        self.assertIn(foreign_id, self.modal_driver.cloud_store)
        self.assertEqual(len(self.modal_driver.cloud_store), 1)


if __name__ == "__main__":
    unittest.main()
