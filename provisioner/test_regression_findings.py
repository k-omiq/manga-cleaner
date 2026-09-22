"""Regression tests covering P7/P8 review findings.

Validates:
1. Cross-provider journal collision protection.
2. Installation ID mismatch and tampered stage/hash/resource shape fail closed.
3. Apply and resume plan drift prevention before any driver mutation.
4. Mandatory confirm_delete_persistent_storage=true before volume deletion.
5. Non-empty HTTPS endpoint_url guard before compatibility validation.
6. Legal state machine transitions (forbids arbitrary jumps, allows resume/cleanup).
7. Symlink hazards on journal file and directory fail closed with ERR_SECURITY_VIOLATION.
"""

import json
import os
from pathlib import Path
import tempfile
import unittest

from provisioner.controller import ProvisioningController
from provisioner.fake_drivers import FakeBeamDriver, FakeModalDriver
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
    InstallationRecord,
    ResourceRecord,
)
from provisioner.protocol import (
    ERR_SECURITY_VIOLATION,
    ERR_UNAPPROVED_PLAN,
    ERR_VALIDATION,
    HELPER_PROTOCOL_VERSION,
    OP_APPLY,
    OP_CLEANUP_APPLY,
    OP_CLEANUP_PLAN,
    OP_PLAN,
    OP_PROBE_COMPATIBILITY,
    OP_RESUME,
    PROVIDER_BEAM,
    PROVIDER_MODAL,
    ProtocolError,
)
from provisioner.redaction import GLOBAL_REGISTRY


class TestProvisionerRegressionFindings(unittest.TestCase):
    """Regression tests for accepted P7/P8 review findings."""

    def setUp(self):
        self.temp_dir = tempfile.TemporaryDirectory()
        self.root_path = Path(self.temp_dir.name)
        self.modal_driver = FakeModalDriver()
        self.beam_driver = FakeBeamDriver(platform_override="darwin")
        self.controller = ProvisioningController(
            journal_root=self.root_path,
            modal_driver=self.modal_driver,
            beam_driver=self.beam_driver,
        )
        GLOBAL_REGISTRY.clear()

    def tearDown(self):
        GLOBAL_REGISTRY.clear()
        self.temp_dir.cleanup()

    # --- Finding 1: Cross-provider journal collision & Installation mismatch ---

    def test_cross_provider_journal_collision_direct_load(self):
        """A journal saved for Modal cannot be loaded by Beam journal identity."""
        inst_id = "inst-x-prov"
        modal_journal = InstallationJournal(self.root_path, inst_id, PROVIDER_MODAL)
        modal_journal.load_or_initialize("a" * 64, "a" * 64, f"mc-app-{inst_id}")

        # Attempt to load as Beam journal
        beam_journal = InstallationJournal(self.root_path, inst_id, PROVIDER_BEAM)
        with self.assertRaises(ProtocolError) as ctx:
            beam_journal.load()
        self.assertEqual(ctx.exception.code, ERR_VALIDATION)
        self.assertIn("Journal provider mismatch", ctx.exception.message)

    def test_cross_provider_journal_collision_controller_apply(self):
        """Apply request with Beam for an installation already planned under Modal fails closed."""
        inst_id = "inst-x-prov-ctrl"
        modal_creds = {"token_id": "ak-test", "token_secret": "as-test"}

        # 1. Plan under Modal
        req_plan = {
            "protocol_version": HELPER_PROTOCOL_VERSION,
            "request_id": "req-p1",
            "op": OP_PLAN,
            "provider": PROVIDER_MODAL,
            "params": {"installation_id": inst_id, "credentials": modal_creds},
        }
        res_plan = json.loads(self.controller.handle_request(req_plan))
        self.assertTrue(res_plan["success"])

        # 2. Attempt to apply under Beam
        beam_creds = {"beam_token": "b9_test_token"}
        insp = self.beam_driver.inspect_account(beam_creds)
        beam_plan = self.beam_driver.create_deployment_plan(insp, inst_id)

        req_apply_beam = {
            "protocol_version": HELPER_PROTOCOL_VERSION,
            "request_id": "req-apply-x",
            "op": OP_APPLY,
            "provider": PROVIDER_BEAM,
            "params": {
                "installation_id": inst_id,
                "approved_plan_hash": beam_plan.plan_hash,
                "credentials": beam_creds,
            },
        }
        res_apply = json.loads(self.controller.handle_request(req_apply_beam))
        self.assertFalse(res_apply["success"])
        self.assertEqual(res_apply["error"]["code"], ERR_VALIDATION)
        self.assertIn("provider mismatch", res_apply["error"]["message"].lower())

    def test_installation_id_mismatch_fails_closed(self):
        """A journal file containing a mismatched installation_id fails closed on load."""
        inst_id = "inst-mismatch-id"
        journal = InstallationJournal(self.root_path, inst_id, PROVIDER_MODAL)
        journal.load_or_initialize("b" * 64, "b" * 64, f"mc-app-{inst_id}")

        # Tamper installation_id on disk
        data = json.loads(journal.journal_file.read_text(encoding="utf-8"))
        data["installation_id"] = "tampered-other-id"
        journal.journal_file.write_text(json.dumps(data), encoding="utf-8")

        with self.assertRaises(ProtocolError) as ctx:
            journal.load()
        self.assertEqual(ctx.exception.code, ERR_VALIDATION)
        self.assertIn("installation_id mismatch", ctx.exception.message)

    # --- Finding 1: Tampered stage / hash / resource shapes ---

    def test_tampered_stage_fails_closed(self):
        """A journal containing an unallowlisted or corrupted stage fails closed on load."""
        inst_id = "inst-bad-stage"
        journal = InstallationJournal(self.root_path, inst_id, PROVIDER_MODAL)
        journal.load_or_initialize("c" * 64, "c" * 64, f"mc-app-{inst_id}")

        data = json.loads(journal.journal_file.read_text(encoding="utf-8"))
        data["stage"] = "unauthorized_super_stage"
        journal.journal_file.write_text(json.dumps(data), encoding="utf-8")

        with self.assertRaises(ProtocolError) as ctx:
            journal.load()
        self.assertEqual(ctx.exception.code, ERR_VALIDATION)
        self.assertIn("Invalid or tampered stage", ctx.exception.message)

    def test_tampered_plan_hash_fails_closed(self):
        """Corrupt or non-hex plan_hash fails closed on load."""
        inst_id = "inst-bad-hash"
        journal = InstallationJournal(self.root_path, inst_id, PROVIDER_MODAL)
        journal.load_or_initialize("d" * 64, "d" * 64, f"mc-app-{inst_id}")

        data = json.loads(journal.journal_file.read_text(encoding="utf-8"))
        data["plan_hash"] = "tampered_non_hex_hash_value"
        journal.journal_file.write_text(json.dumps(data), encoding="utf-8")

        with self.assertRaises(ProtocolError) as ctx:
            journal.load()
        self.assertEqual(ctx.exception.code, ERR_VALIDATION)
        self.assertIn("plan_hash", ctx.exception.message)

    def test_tampered_approved_plan_hash_fails_closed(self):
        """Corrupt or invalid approved_plan_hash fails closed on load."""
        inst_id = "inst-bad-approved-hash"
        journal = InstallationJournal(self.root_path, inst_id, PROVIDER_MODAL)
        journal.load_or_initialize("e" * 64, "e" * 64, f"mc-app-{inst_id}")

        data = json.loads(journal.journal_file.read_text(encoding="utf-8"))
        data["approved_plan_hash"] = "short_hash"
        journal.journal_file.write_text(json.dumps(data), encoding="utf-8")

        with self.assertRaises(ProtocolError) as ctx:
            journal.load()
        self.assertEqual(ctx.exception.code, ERR_VALIDATION)
        self.assertIn("approved_plan_hash", ctx.exception.message)

    def test_tampered_resource_ownership_shape_fails_closed(self):
        """Resource records missing manga-cleaner ownership tag shape fail closed on load."""
        inst_id = "inst-bad-resource"
        journal = InstallationJournal(self.root_path, inst_id, PROVIDER_MODAL)
        journal.load_or_initialize("f" * 64, "f" * 64, f"mc-app-{inst_id}")
        journal.record_resource("vol-1", "volume", f"mc-modal-vol-{inst_id}", STAGE_SEEDING)

        data = json.loads(journal.journal_file.read_text(encoding="utf-8"))
        # Tamper the ownership tag
        data["resources"][0]["ownership_tags"]["managed_by"] = "attacker"
        journal.journal_file.write_text(json.dumps(data), encoding="utf-8")

        with self.assertRaises(ProtocolError) as ctx:
            journal.load()
        self.assertEqual(ctx.exception.code, ERR_VALIDATION)
        self.assertIn("managed_by tag", ctx.exception.message)

    # --- Finding 2: Apply & Resume plan drift before mutation ---

    def test_apply_with_existing_journal_plan_drift_refused_before_mutation(self):
        """_handle_apply refuses execution if existing journal already has a different plan hash."""
        inst_id = "inst-apply-drift"
        creds = {"token_id": "ak-test", "token_secret": "as-test"}

        # Create existing journal with hash "1" * 64
        journal = InstallationJournal(self.root_path, inst_id, PROVIDER_MODAL)
        journal.load_or_initialize("1" * 64, "", f"mc-app-{inst_id}")

        insp = self.modal_driver.inspect_account(creds)
        plan = self.modal_driver.create_deployment_plan(insp, inst_id)

        req_apply = {
            "protocol_version": HELPER_PROTOCOL_VERSION,
            "request_id": "req-apply-drift",
            "op": OP_APPLY,
            "provider": PROVIDER_MODAL,
            "params": {
                "installation_id": inst_id,
                "approved_plan_hash": plan.plan_hash,
                "credentials": creds,
            },
        }
        res_json = json.loads(self.controller.handle_request(req_apply))
        self.assertFalse(res_json["success"])
        self.assertEqual(res_json["error"]["code"], ERR_UNAPPROVED_PLAN)
        self.assertIn("Plan hash mismatch with existing journal", res_json["error"]["message"])
        # Invariant: Zero driver mutation
        self.assertEqual(len(self.modal_driver.cloud_store), 0)

    def test_resume_without_approved_hash_fails_closed_before_mutation(self):
        """_handle_resume rejects unapproved journal (empty approved_plan_hash) before any driver mutation."""
        inst_id = "inst-resume-unapproved"
        creds = {"token_id": "ak-test", "token_secret": "as-test"}

        insp = self.modal_driver.inspect_account(creds)
        plan = self.modal_driver.create_deployment_plan(insp, inst_id)

        # Initialize journal via OP_PLAN (approved_plan_hash remains empty)
        journal = InstallationJournal(self.root_path, inst_id, PROVIDER_MODAL)
        journal.load_or_initialize(plan.plan_hash, "", plan.app_name)

        req_resume = {
            "protocol_version": HELPER_PROTOCOL_VERSION,
            "request_id": "req-resume-unappr",
            "op": OP_RESUME,
            "provider": PROVIDER_MODAL,
            "params": {
                "installation_id": inst_id,
                "credentials": creds,
            },
        }
        res_json = json.loads(self.controller.handle_request(req_resume))
        self.assertFalse(res_json["success"])
        self.assertEqual(res_json["error"]["code"], ERR_UNAPPROVED_PLAN)
        self.assertIn("no approved plan hash recorded", res_json["error"]["message"])
        # Zero driver mutation
        self.assertEqual(len(self.modal_driver.cloud_store), 0)

    def test_resume_plan_drift_options_changed_fails_before_mutation(self):
        """_handle_resume regenerates plan and compares against stored hash; refuses on drift before mutation."""
        inst_id = "inst-resume-drift"
        creds = {"token_id": "ak-test", "token_secret": "as-test"}

        # Simulate failure during deploy
        failing_driver = FakeModalDriver(simulate_failure_at_step="deploy")
        controller = ProvisioningController(
            journal_root=self.root_path,
            modal_driver=failing_driver,
            beam_driver=self.beam_driver,
        )

        insp = failing_driver.inspect_account(creds)
        plan = failing_driver.create_deployment_plan(insp, inst_id)

        # Initial apply creates volume, then fails at deploy
        req_apply = {
            "protocol_version": HELPER_PROTOCOL_VERSION,
            "request_id": "req-drift-apply",
            "op": OP_APPLY,
            "provider": PROVIDER_MODAL,
            "params": {
                "installation_id": inst_id,
                "approved_plan_hash": plan.plan_hash,
                "credentials": creds,
            },
        }
        controller.handle_request(req_apply)
        self.assertEqual(len(failing_driver.cloud_store), 1)

        # Clear simulated failure
        failing_driver.simulate_failure_at_step = None

        # Manually alter the stored plan hash in journal to simulate option drift
        journal = InstallationJournal(self.root_path, inst_id, PROVIDER_MODAL)
        rec = journal.load()
        rec.plan_hash = "9" * 64
        rec.approved_plan_hash = "9" * 64
        journal.save()

        # Resume attempt must detect drift and fail BEFORE mutating cloud store
        req_resume = {
            "protocol_version": HELPER_PROTOCOL_VERSION,
            "request_id": "req-drift-resume",
            "op": OP_RESUME,
            "provider": PROVIDER_MODAL,
            "params": {
                "installation_id": inst_id,
                "credentials": creds,
            },
        }
        res_resume = json.loads(controller.handle_request(req_resume))
        self.assertFalse(res_resume["success"])
        self.assertEqual(res_resume["error"]["code"], ERR_UNAPPROVED_PLAN)
        self.assertIn("Plan drift detected on resume", res_resume["error"]["message"])

        # Cloud store must STILL have only the single volume created before the resume attempt
        self.assertEqual(len(failing_driver.cloud_store), 1)
        self.assertNotIn(f"app_modal_{inst_id}", failing_driver.cloud_store)

    # --- Finding 3: cleanup_apply volume deletion confirmation ---

    def test_cleanup_apply_requires_confirm_delete_persistent_storage(self):
        """cleanup_apply refuses volume deletion without explicit boolean confirmation."""
        inst_id = "inst-vol-confirm"
        creds = {"token_id": "ak-test", "token_secret": "as-test"}

        # Provision installation
        insp = self.modal_driver.inspect_account(creds)
        plan = self.modal_driver.create_deployment_plan(insp, inst_id)

        req_apply = {
            "protocol_version": HELPER_PROTOCOL_VERSION,
            "request_id": "req-clean-prep",
            "op": OP_APPLY,
            "provider": PROVIDER_MODAL,
            "params": {
                "installation_id": inst_id,
                "approved_plan_hash": plan.plan_hash,
                "credentials": creds,
            },
        }
        self.controller.handle_request(req_apply)

        # Create cleanup plan via controller
        req_plan = {
            "protocol_version": HELPER_PROTOCOL_VERSION,
            "request_id": "req-clean-plan-prep",
            "op": OP_CLEANUP_PLAN,
            "provider": PROVIDER_MODAL,
            "params": {"installation_id": inst_id},
        }
        res_plan = json.loads(self.controller.handle_request(req_plan))
        self.assertTrue(res_plan["success"])
        cleanup_plan_hash = res_plan["data"]["plan_hash"]

        # 1. Attempt cleanup_apply WITHOUT confirm_delete_persistent_storage
        req_clean_no_confirm = {
            "protocol_version": HELPER_PROTOCOL_VERSION,
            "request_id": "req-clean-nc",
            "op": OP_CLEANUP_APPLY,
            "provider": PROVIDER_MODAL,
            "params": {
                "installation_id": inst_id,
                "approved_cleanup_plan_hash": cleanup_plan_hash,
                "credentials": creds,
            },
        }
        res_nc = json.loads(self.controller.handle_request(req_clean_no_confirm))
        self.assertFalse(res_nc["success"])
        self.assertEqual(res_nc["error"]["code"], ERR_VALIDATION)
        self.assertIn("confirm_delete_persistent_storage=true", res_nc["error"]["message"])

        # Invariant: Volume was NOT deleted
        vol_id = f"vol_modal_{inst_id}"
        self.assertIn(vol_id, self.modal_driver.cloud_store)

        # 2. Attempt cleanup_apply with confirm_delete_persistent_storage=False (string or non-True)
        req_clean_false = {
            "protocol_version": HELPER_PROTOCOL_VERSION,
            "request_id": "req-clean-false",
            "op": OP_CLEANUP_APPLY,
            "provider": PROVIDER_MODAL,
            "params": {
                "installation_id": inst_id,
                "approved_cleanup_plan_hash": cleanup_plan_hash,
                "credentials": creds,
                "confirm_delete_persistent_storage": False,
            },
        }
        res_false = json.loads(self.controller.handle_request(req_clean_false))
        self.assertFalse(res_false["success"])
        self.assertEqual(res_false["error"]["code"], ERR_VALIDATION)

        # 3. Supply explicit boolean confirm_delete_persistent_storage=True -> succeeds
        req_clean_ok = {
            "protocol_version": HELPER_PROTOCOL_VERSION,
            "request_id": "req-clean-ok",
            "op": OP_CLEANUP_APPLY,
            "provider": PROVIDER_MODAL,
            "params": {
                "installation_id": inst_id,
                "approved_cleanup_plan_hash": cleanup_plan_hash,
                "credentials": creds,
                "confirm_delete_persistent_storage": True,
            },
        }
        res_ok = json.loads(self.controller.handle_request(req_clean_ok))
        self.assertTrue(res_ok["success"])
        self.assertNotIn(vol_id, self.modal_driver.cloud_store)

    # --- Finding 4: Guard non-empty HTTPS endpoint_url ---

    def test_probe_compatibility_refuses_non_https_and_empty_endpoints(self):
        """probe_compatibility fails closed on missing, empty, or non-HTTPS URLs."""
        invalid_endpoints = [
            "",
            "   ",
            "http://insecure.example.com",
            "ftp://ftp.example.com/api",
            "just_a_string",
            "https://",
            None,
        ]

        for ep in invalid_endpoints:
            req = {
                "protocol_version": HELPER_PROTOCOL_VERSION,
                "request_id": "req-probe-bad",
                "op": OP_PROBE_COMPATIBILITY,
                "provider": PROVIDER_MODAL,
                "params": {"endpoint_url": ep},
            }
            res = json.loads(self.controller.handle_request(req))
            self.assertFalse(res["success"])
            self.assertEqual(res["error"]["code"], ERR_VALIDATION)

    def test_pipeline_refuses_insecure_endpoint_discovery(self):
        """Pipeline fails closed before compatibility validation if discovered endpoint is not HTTPS."""
        inst_id = "inst-insecure-ep"
        creds = {"token_id": "ak-test", "token_secret": "as-test"}

        # Custom driver returning insecure HTTP
        class InsecureEndpointDriver(FakeModalDriver):
            def discover_endpoint(self, installation_id, app_name, credentials):
                return "http://insecure.modal.run"

        insecure_driver = InsecureEndpointDriver()
        controller = ProvisioningController(
            journal_root=self.root_path,
            modal_driver=insecure_driver,
            beam_driver=self.beam_driver,
        )

        insp = insecure_driver.inspect_account(creds)
        plan = insecure_driver.create_deployment_plan(insp, inst_id)

        req_apply = {
            "protocol_version": HELPER_PROTOCOL_VERSION,
            "request_id": "req-insecure-apply",
            "op": OP_APPLY,
            "provider": PROVIDER_MODAL,
            "params": {
                "installation_id": inst_id,
                "approved_plan_hash": plan.plan_hash,
                "credentials": creds,
            },
        }
        res = json.loads(controller.handle_request(req_apply))
        self.assertFalse(res["success"])
        self.assertEqual(res["error"]["code"], ERR_VALIDATION)
        self.assertIn("HTTPS", res["error"]["message"])

    # --- Finding 5: Legal state-transition validation ---

    def test_arbitrary_and_backwards_state_transitions_rejected(self):
        """Illegal state transitions are rejected with ProtocolError."""
        journal = InstallationJournal(self.root_path, "inst-state-rules", PROVIDER_MODAL)
        journal.load_or_initialize("a" * 64, "a" * 64, "mc-app-state")

        # Initial stage is STAGE_PLANNED
        self.assertEqual(journal.record.stage, STAGE_PLANNED)

        # Illegal: planned directly to completed
        with self.assertRaises(ProtocolError) as ctx:
            journal.transition_to(STAGE_COMPLETED)
        self.assertEqual(ctx.exception.code, ERR_VALIDATION)
        self.assertIn("Illegal state transition", ctx.exception.message)

        # Legal: planned -> seeding -> seeded
        journal.transition_to(STAGE_SEEDING)
        journal.transition_to(STAGE_SEEDED)

        # Illegal: seeded backwards to planned
        with self.assertRaises(ProtocolError) as ctx:
            journal.transition_to(STAGE_PLANNED)
        self.assertEqual(ctx.exception.code, ERR_VALIDATION)

        # Legal: seeded -> cleanup_planned -> cleaned_up
        journal.transition_to(STAGE_CLEANUP_PLANNED)
        journal.transition_to(STAGE_CLEANED_UP)

        # Illegal: transition out of terminal cleaned_up stage
        for invalid_target in [STAGE_PLANNED, STAGE_SEEDING, STAGE_DEPLOYING, STAGE_COMPLETED]:
            with self.assertRaises(ProtocolError) as ctx:
                journal.transition_to(invalid_target)
            self.assertEqual(ctx.exception.code, ERR_VALIDATION)

    # --- Finding 6: Symlink hazards ---

    def test_symlink_journal_file_hazard_fails_closed(self):
        """Symlinked journal file is rejected with ERR_SECURITY_VIOLATION before reading."""
        inst_id = "inst-symlink-hazard"
        inst_dir = self.root_path / "installations" / inst_id
        inst_dir.mkdir(parents=True, exist_ok=True)
        journal_target = self.root_path / "secret_file.txt"
        journal_target.write_text("sensitive data", encoding="utf-8")

        symlink_journal = inst_dir / "journal.json"
        os.symlink(str(journal_target), str(symlink_journal))

        journal = InstallationJournal(self.root_path, inst_id, PROVIDER_MODAL)

        with self.assertRaises(ProtocolError) as ctx:
            journal.exists()
        self.assertEqual(ctx.exception.code, ERR_SECURITY_VIOLATION)

        with self.assertRaises(ProtocolError) as ctx:
            journal.load()
        self.assertEqual(ctx.exception.code, ERR_SECURITY_VIOLATION)

    def test_symlink_installation_dir_and_journal_file_save_fails_closed(self):
        """InstallationJournal.save must fail closed on symlinked installation_dir or journal_file before mkdir/open/replace."""
        # 1. Symlinked installation directory
        inst_id_dir = "inst-symlink-dir-save"
        target_dir = self.root_path / "target_dir"
        target_dir.mkdir(parents=True, exist_ok=True)
        installations_root = self.root_path / "installations"
        installations_root.mkdir(parents=True, exist_ok=True)
        symlink_dir = installations_root / inst_id_dir
        os.symlink(str(target_dir), str(symlink_dir))

        journal_dir_sym = InstallationJournal(self.root_path, inst_id_dir, PROVIDER_MODAL)
        with self.assertRaises(ProtocolError) as ctx:
            journal_dir_sym.load_or_initialize("a" * 64, "a" * 64, "mc-app-symdir")
        self.assertEqual(ctx.exception.code, ERR_SECURITY_VIOLATION)
        self.assertIn("Journal directory cannot be a symbolic link", ctx.exception.message)

        # 2. Symlinked journal file before save
        inst_id_file = "inst-symlink-file-save"
        inst_dir = installations_root / inst_id_file
        inst_dir.mkdir(parents=True, exist_ok=True)
        target_file = self.root_path / "target_file.json"
        target_file.write_text("{}", encoding="utf-8")
        symlink_file = inst_dir / "journal.json"
        os.symlink(str(target_file), str(symlink_file))

        journal_file_sym = InstallationJournal(self.root_path, inst_id_file, PROVIDER_MODAL)
        with self.assertRaises(ProtocolError) as ctx:
            journal_file_sym.save()
        self.assertEqual(ctx.exception.code, ERR_SECURITY_VIOLATION)
        self.assertIn("Journal file cannot be a symbolic link", ctx.exception.message)

    # --- Finding 7: cleanup_apply pre-mutation transition and interrupted-stage regression ---

    def test_cleanup_apply_transitions_to_cleanup_planned_before_mutation_and_handles_interrupted_stage(self):
        """cleanup_apply must transition journal to cleanup_planned before mutation so failure preserves interrupted state."""
        inst_id = "inst-clean-interrupt"
        creds = {"token_id": "ak-test", "token_secret": "as-test"}

        # Custom driver that fails during cleanup execution after creating plan
        class InterruptedCleanupDriver(FakeModalDriver):
            def execute_cleanup(self, cleanup_plan, credentials):
                raise RuntimeError("Cloud provider communication failure during teardown")

        interrupted_driver = InterruptedCleanupDriver()
        controller = ProvisioningController(
            journal_root=self.root_path,
            modal_driver=interrupted_driver,
            beam_driver=self.beam_driver,
        )

        # 1. Provision to completion
        insp = interrupted_driver.inspect_account(creds)
        plan = interrupted_driver.create_deployment_plan(insp, inst_id)
        req_apply = {
            "protocol_version": HELPER_PROTOCOL_VERSION,
            "request_id": "req-clean-app",
            "op": OP_APPLY,
            "provider": PROVIDER_MODAL,
            "params": {
                "installation_id": inst_id,
                "approved_plan_hash": plan.plan_hash,
                "credentials": creds,
            },
        }
        res_apply = json.loads(controller.handle_request(req_apply))
        self.assertTrue(res_apply["success"])

        # Check journal is completed
        journal = InstallationJournal(self.root_path, inst_id, PROVIDER_MODAL)
        self.assertEqual(journal.load().stage, STAGE_COMPLETED)

        # 2. Plan cleanup
        req_clean_plan = {
            "protocol_version": HELPER_PROTOCOL_VERSION,
            "request_id": "req-cplan",
            "op": OP_CLEANUP_PLAN,
            "provider": PROVIDER_MODAL,
            "params": {"installation_id": inst_id},
        }
        res_cplan = json.loads(controller.handle_request(req_clean_plan))
        self.assertTrue(res_cplan["success"])
        cleanup_hash = res_cplan["data"]["plan_hash"]

        # 3. Apply cleanup with failing driver
        req_clean_apply = {
            "protocol_version": HELPER_PROTOCOL_VERSION,
            "request_id": "req-capply-fail",
            "op": OP_CLEANUP_APPLY,
            "provider": PROVIDER_MODAL,
            "params": {
                "installation_id": inst_id,
                "approved_cleanup_plan_hash": cleanup_hash,
                "credentials": creds,
                "confirm_delete_persistent_storage": True,
            },
        }
        res_capply = json.loads(controller.handle_request(req_clean_apply))
        self.assertFalse(res_capply["success"])

        # The journal MUST NOT remain in 'completed' (which would desynchronize the journal)
        reloaded_rec = journal.load()
        self.assertNotEqual(reloaded_rec.stage, STAGE_COMPLETED)
        self.assertEqual(reloaded_rec.stage, STAGE_FAILED)

    # --- Finding 8: apply against completed/cleaned_up terminal journals ---

    def test_apply_against_completed_journal_returns_validation_error_without_mutation(self):
        """apply against completed terminal journal must return a clear non-mutating validation result."""
        inst_id = "inst-apply-completed"
        creds = {"token_id": "ak-test", "token_secret": "as-test"}

        insp = self.modal_driver.inspect_account(creds)
        plan = self.modal_driver.create_deployment_plan(insp, inst_id)

        # 1. First apply succeeds
        req_apply = {
            "protocol_version": HELPER_PROTOCOL_VERSION,
            "request_id": "req-apply-first",
            "op": OP_APPLY,
            "provider": PROVIDER_MODAL,
            "params": {
                "installation_id": inst_id,
                "approved_plan_hash": plan.plan_hash,
                "credentials": creds,
            },
        }
        res_first = json.loads(self.controller.handle_request(req_apply))
        self.assertTrue(res_first["success"])

        journal = InstallationJournal(self.root_path, inst_id, PROVIDER_MODAL)
        saved_rec = journal.load()
        self.assertEqual(saved_rec.stage, STAGE_COMPLETED)
        saved_updated_at = saved_rec.updated_at_utc
        initial_cloud_store_len = len(self.modal_driver.cloud_store)

        # 2. Re-applying against completed journal must fail with ERR_VALIDATION
        req_apply_repeat = {
            "protocol_version": HELPER_PROTOCOL_VERSION,
            "request_id": "req-apply-repeat",
            "op": OP_APPLY,
            "provider": PROVIDER_MODAL,
            "params": {
                "installation_id": inst_id,
                "approved_plan_hash": plan.plan_hash,
                "credentials": creds,
            },
        }
        res_repeat = json.loads(self.controller.handle_request(req_apply_repeat))
        self.assertFalse(res_repeat["success"])
        self.assertEqual(res_repeat["error"]["code"], ERR_VALIDATION)
        self.assertIn("terminal stage 'completed'", res_repeat["error"]["message"])

        # Invariant: Non-mutating (no changes to journal file or cloud store)
        reloaded = journal.load()
        self.assertEqual(reloaded.stage, STAGE_COMPLETED)
        self.assertEqual(reloaded.updated_at_utc, saved_updated_at)
        self.assertEqual(len(self.modal_driver.cloud_store), initial_cloud_store_len)

    def test_apply_against_cleaned_up_journal_returns_validation_error_without_illegal_transition(self):
        """apply against cleaned_up terminal journal must return a clear non-mutating validation result and never attempt illegal transitions."""
        inst_id = "inst-apply-cleaned-up"
        creds = {"token_id": "ak-test", "token_secret": "as-test"}

        # Initialize journal directly in STAGE_CLEANED_UP
        journal = InstallationJournal(self.root_path, inst_id, PROVIDER_MODAL)
        journal.load_or_initialize("a" * 64, "a" * 64, f"mc-app-{inst_id}")
        journal.transition_to(STAGE_CLEANUP_PLANNED)
        journal.transition_to(STAGE_CLEANED_UP)
        saved_updated_at = journal.load().updated_at_utc

        insp = self.modal_driver.inspect_account(creds)
        plan = self.modal_driver.create_deployment_plan(insp, inst_id)

        req_apply = {
            "protocol_version": HELPER_PROTOCOL_VERSION,
            "request_id": "req-apply-cleaned",
            "op": OP_APPLY,
            "provider": PROVIDER_MODAL,
            "params": {
                "installation_id": inst_id,
                "approved_plan_hash": plan.plan_hash,
                "credentials": creds,
            },
        }
        res = json.loads(self.controller.handle_request(req_apply))
        self.assertFalse(res["success"])
        self.assertEqual(res["error"]["code"], ERR_VALIDATION)
        self.assertIn("terminal stage 'cleaned_up'", res["error"]["message"])

        # Invariant: Journal remains in cleaned_up without mutation or illegal transition attempt
        reloaded = journal.load()
        self.assertEqual(reloaded.stage, STAGE_CLEANED_UP)
        self.assertEqual(reloaded.updated_at_utc, saved_updated_at)
        self.assertEqual(len(self.modal_driver.cloud_store), 0)


if __name__ == "__main__":
    unittest.main()
