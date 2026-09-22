"""Unit tests for the resumable installation and resource journal."""

import json
from pathlib import Path
import tempfile
import unittest

from provisioner.journal import (
    MAX_JOURNAL_BYTES,
    STAGE_CLEANED_UP,
    STAGE_COMPLETED,
    STAGE_CREDENTIAL_CREATED,
    STAGE_DEPLOYED,
    STAGE_DEPLOYING,
    STAGE_DISCOVERED,
    STAGE_PLANNED,
    STAGE_SEEDED,
    STAGE_SEEDING,
    STAGE_VALIDATED,
    InstallationJournal,
    InstallationRecord,
    ResourceRecord,
)
from provisioner.protocol import ERR_SECURITY_VIOLATION, ERR_VALIDATION, ProtocolError


class TestInstallationJournal(unittest.TestCase):
    """Tests for installation journal durability, stage tracking, and duplicate prevention."""

    def setUp(self):
        self.temp_dir = tempfile.TemporaryDirectory()
        self.root_path = Path(self.temp_dir.name)

    def tearDown(self):
        self.temp_dir.cleanup()

    def test_journal_initialization_and_atomic_save(self):
        journal = InstallationJournal(self.root_path, "inst-001", "modal")
        self.assertFalse(journal.exists())

        rec = journal.load_or_initialize(
            plan_hash="a" * 64,
            approved_plan_hash="a" * 64,
            app_name="mc-app-inst-001",
        )
        self.assertTrue(journal.exists())
        self.assertEqual(rec.stage, STAGE_PLANNED)
        self.assertEqual(rec.installation_id, "inst-001")
        self.assertEqual(rec.provider, "modal")

        # Reload from fresh instance
        reloaded_journal = InstallationJournal(self.root_path, "inst-001", "modal")
        loaded_rec = reloaded_journal.load()
        self.assertEqual(loaded_rec.installation_id, "inst-001")
        self.assertEqual(loaded_rec.stage, STAGE_PLANNED)
        self.assertEqual(loaded_rec.app_name, "mc-app-inst-001")

    def test_stage_transitions(self):
        journal = InstallationJournal(self.root_path, "inst-stages", "beam")
        journal.load_or_initialize("b" * 64, "b" * 64, "mc-beam-svc")

        stages = [
            STAGE_SEEDING,
            STAGE_SEEDED,
            STAGE_DEPLOYING,
            STAGE_DEPLOYED,
            STAGE_DISCOVERED,
            STAGE_CREDENTIAL_CREATED,
            STAGE_VALIDATED,
            STAGE_COMPLETED,
        ]
        for st in stages:
            journal.transition_to(st)
            self.assertEqual(journal.record.stage, st)
            # Re-read from disk to prove durability
            reloaded = InstallationJournal(self.root_path, "inst-stages", "beam").load()
            self.assertEqual(reloaded.stage, st)

    def test_duplicate_resource_prevention(self):
        journal = InstallationJournal(self.root_path, "inst-dup", "modal")
        journal.load_or_initialize("c" * 64, "c" * 64, "mc-app-dup")

        # First recording
        r1 = journal.record_resource(
            resource_id="vol-100",
            resource_type="volume",
            name="mc-flux-vol-inst-dup",
            stage_created=STAGE_SEEDING,
        )
        self.assertEqual(len(journal.record.resources), 1)

        # Second recording with identical resource_id
        r2 = journal.record_resource(
            resource_id="vol-100",
            resource_type="volume",
            name="mc-flux-vol-inst-dup",
            stage_created=STAGE_SEEDING,
        )
        self.assertEqual(len(journal.record.resources), 1)
        self.assertEqual(r1.resource_id, r2.resource_id)

        # Third recording with same type and name but different id -> must not duplicate
        r3 = journal.record_resource(
            resource_id="vol-101",
            resource_type="volume",
            name="mc-flux-vol-inst-dup",
            stage_created=STAGE_SEEDING,
        )
        self.assertEqual(len(journal.record.resources), 1)
        self.assertEqual(r3.resource_id, "vol-100")

    def test_zero_raw_credentials_stored_on_disk(self):
        """Invariant: raw credentials must NEVER be persisted in the journal JSON."""
        journal = InstallationJournal(self.root_path, "inst-no-secret", "modal")
        rec = journal.load_or_initialize("d" * 64, "d" * 64, "mc-app-sec")

        # Simulate recording runtime credential reference
        rec.runtime_credential_ref = {
            "resource_id": "tok-1",
            "token_type": "proxy_token",
            "name": "tok-proxy-inst",
            # If an untrusted caller attempted to pass secret data:
            "secret_key": "as-SUPER-SECRET-MODAL-KEY",
        }
        journal.save()

        # Inspect raw bytes on disk
        raw_text = journal.journal_file.read_text(encoding="utf-8")
        self.assertNotIn("as-SUPER-SECRET-MODAL-KEY", raw_text)
        self.assertIn("[REDACTED_CREDENTIAL]", raw_text)

    def test_forget_setup_credential_tracking(self):
        journal = InstallationJournal(self.root_path, "inst-forget", "modal")
        journal.load_or_initialize("e" * 64, "e" * 64, "mc-app-forget")

        self.assertFalse(journal.record.setup_credential_forgotten)
        journal.forget_setup_credential()
        self.assertTrue(journal.record.setup_credential_forgotten)

        # Re-read from disk
        reloaded = InstallationJournal(self.root_path, "inst-forget", "modal").load()
        self.assertTrue(reloaded.setup_credential_forgotten)

    def test_resume_decision_logic(self):
        journal = InstallationJournal(self.root_path, "inst-resume-logic", "modal")
        journal.load_or_initialize("f" * 64, "f" * 64, "mc-app-resume")

        self.assertTrue(journal.can_resume())
        self.assertEqual(journal.next_action(), "seed")

        journal.transition_to(STAGE_SEEDING)
        journal.record_resource("vol-1", "volume", "mc-modal-vol-inst-resume-logic", STAGE_SEEDING)
        journal.transition_to(STAGE_SEEDED)
        self.assertEqual(journal.next_action(), "deploy")

        journal.record_resource("app-1", "app", "mc-app-resume", STAGE_DEPLOYING)
        journal.transition_to(STAGE_DEPLOYED)
        self.assertEqual(journal.next_action(), "discover_endpoint")

        journal.transition_to(STAGE_DISCOVERED)
        self.assertEqual(journal.next_action(), "create_runtime_credential")

        journal.transition_to(STAGE_CREDENTIAL_CREATED)
        self.assertEqual(journal.next_action(), "validate_compatibility")

        journal.transition_to(STAGE_VALIDATED)
        self.assertEqual(journal.next_action(), "complete")

        journal.transition_to(STAGE_COMPLETED)
        self.assertFalse(journal.can_resume())
        self.assertEqual(journal.next_action(), "ready")

    def test_oversized_journal_rejected(self):
        journal = InstallationJournal(self.root_path, "inst-huge", "modal")
        journal.load_or_initialize("0" * 64, "0" * 64, "mc-app-huge")

        # Write oversized file to journal path
        with open(journal.journal_file, "wb") as f:
            f.seek(MAX_JOURNAL_BYTES + 2048)
            f.write(b"\0")

        with self.assertRaises(ProtocolError) as ctx:
            journal.load()
        self.assertEqual(ctx.exception.code, ERR_SECURITY_VIOLATION)

    def test_reject_unsafe_installation_id(self):
        for bad_id in ["../escape", "null\0byte", "slash/id", ""]:
            with self.assertRaises((ProtocolError, Exception)):
                InstallationJournal(self.root_path, bad_id, "modal")

    def test_symlink_hazards_fail_closed_on_save(self):
        """Proves InstallationJournal.save fails closed on symlinked installation_dir or journal_file."""
        import os
        # 1. Symlinked installation dir
        target_dir = self.root_path / "real_dir"
        target_dir.mkdir(parents=True, exist_ok=True)
        inst_dir = self.root_path / "installations" / "inst-symdir"
        inst_dir.parent.mkdir(parents=True, exist_ok=True)
        os.symlink(str(target_dir), str(inst_dir))

        j1 = InstallationJournal(self.root_path, "inst-symdir", "modal")
        with self.assertRaises(ProtocolError) as ctx:
            j1.save()
        self.assertEqual(ctx.exception.code, ERR_SECURITY_VIOLATION)

        # 2. Symlinked journal file
        inst_dir2 = self.root_path / "installations" / "inst-symfile"
        inst_dir2.mkdir(parents=True, exist_ok=True)
        target_file = self.root_path / "dummy.txt"
        target_file.write_text("content", encoding="utf-8")
        os.symlink(str(target_file), str(inst_dir2 / "journal.json"))

        j2 = InstallationJournal(self.root_path, "inst-symfile", "modal")
        with self.assertRaises(ProtocolError) as ctx:
            j2.save()
        self.assertEqual(ctx.exception.code, ERR_SECURITY_VIOLATION)


if __name__ == "__main__":
    unittest.main()
