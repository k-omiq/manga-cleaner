"""Offline feasibility unit tests for P1 preparation.

Tests actual packaging failure modes, candidate metadata, and AST introspection:
1. Pinned candidate metadata, beta9 hash verification, and unverified platform labeling.
2. Trusted template syntax and unauthenticated health stub check.
3. Actual packaging failure modes:
   - Missing required provider template (enforces both Modal and Beam).
   - Pre-read oversized file rejection (aborts before reading bytes).
   - Nested module import detection (AST walk catches imports inside functions/blocks).
   - Prohibited weight/binary artifact rejection.
   - Syntax error detection.
4. AST static analysis limitations disclosure.
5. Real UTC timestamp in probe logs.
6. CPU-only staging plan invariants, P1 prerequisites, and unobserved cleanup disclosure.
"""

from datetime import datetime
import json
import os
import sys
import tempfile
import unittest
from pathlib import Path

# Ensure repo root is on sys.path for direct script execution
REPO_ROOT = Path(__file__).resolve().parent.parent
if str(REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(REPO_ROOT))

from provisioner.matrix import (
    BEAM_CANDIDATE,
    BETA9_CANDIDATE,
    MODAL_CANDIDATE,
    SDK_MATRIX,
)
from provisioner.plans import (
    BEAM_STAGING_PLAN,
    MODAL_STAGING_PLAN,
    STAGING_PLANS,
)
from provisioner.probe import (
    AST_STATIC_ANALYSIS_DISCLOSURE,
    FORBIDDEN_MODULES,
    MAX_TEMPLATE_FILE_SIZE_BYTES,
    REQUIRED_TEMPLATES,
    inspect_template_source,
    run_probe_and_log,
    verify_template_staging,
)


class TestCandidateMatrix(unittest.TestCase):
    """Tests for pinned SDK candidate matrix metadata and platform reality labeling."""

    def test_modal_candidate_metadata(self):
        self.assertEqual(MODAL_CANDIDATE.provider_id, "modal")
        self.assertEqual(MODAL_CANDIDATE.pinned_version, "1.5.5")
        self.assertEqual(MODAL_CANDIDATE.release_date, "2026-08-28T19:51:32Z")
        self.assertEqual(MODAL_CANDIDATE.requires_python, "<3.15,>=3.10")

        wheel = next(a for a in MODAL_CANDIDATE.artifacts if a.filename.endswith(".whl"))
        self.assertEqual(
            wheel.sha256,
            "8d10d3ee09818aaba1973b73ce2521ab8961b63a29b5b52e3ff0d25e7a74808e",
        )

    def test_beam_candidate_metadata(self):
        self.assertEqual(BEAM_CANDIDATE.provider_id, "beam")
        self.assertEqual(BEAM_CANDIDATE.pinned_version, "0.2.211")
        self.assertEqual(BEAM_CANDIDATE.release_date, "2026-09-15T18:10:38Z")
        self.assertEqual(BEAM_CANDIDATE.requires_python, "<4.0,>=3.8")

        wheel = next(a for a in BEAM_CANDIDATE.artifacts if a.filename.endswith(".whl"))
        self.assertEqual(
            wheel.sha256,
            "ffa8c9bb086d85ad1bff07bf8ebde65d5e122a4df1469d1aec56598ddf44c561",
        )

    def test_beta9_candidate_metadata(self):
        """Verifies beta9 pin and verified wheel hash."""
        self.assertEqual(BETA9_CANDIDATE.provider_id, "beta9")
        self.assertEqual(BETA9_CANDIDATE.pinned_version, "0.1.268")

        wheel = next(a for a in BETA9_CANDIDATE.artifacts if a.filename.endswith(".whl"))
        self.assertEqual(
            wheel.sha256,
            "eb21e48b9ae0871449b2434b2e735024a59298ffce64953a066d9d36b8358a75",
        )

    def test_python_compatibility_gate(self):
        """Proves awareness that macOS system Python 3.9.6 is incompatible with modal 1.5.5."""
        self.assertIn("<3.15,>=3.10", MODAL_CANDIDATE.requires_python)
        self.assertIn("3.9.6", MODAL_STAGING_PLAN.explicit_blockers[0])

    def test_platform_status_labeled_unverified(self):
        """Ensures platforms are labeled source-compatible/unverified, not blindly supported."""
        for candidate in (MODAL_CANDIDATE, BEAM_CANDIDATE, BETA9_CANDIDATE):
            for platform_name, status_str in candidate.platform_support.items():
                self.assertIn(
                    "unverified",
                    status_str.lower(),
                    f"{candidate.package_name} on {platform_name} was claimed supported without test evidence",
                )

    def test_cli_wrappers_labeled_as_commands(self):
        """Ensures Click CLI wrappers are distinguished from callable SDK runtime methods."""
        for cmd in BEAM_CANDIDATE.source_cli_commands:
            self.assertTrue(
                "Click Command" in cmd or "wrapper" in cmd,
                f"CLI command {cmd} was not labeled as Click wrapper",
            )


class TestPackagingAndIntrospectionFailureModes(unittest.TestCase):
    """Tests for AST inspection, failure modes, and packaging boundary enforcement."""

    def setUp(self):
        self.templates_dir = Path(__file__).parent / "templates"

    def test_trusted_templates_syntax_and_health(self):
        """Verifies trusted template fixtures have valid syntax and minimal health stub."""
        for tmpl_rel in REQUIRED_TEMPLATES:
            path = self.templates_dir / tmpl_rel
            self.assertTrue(path.exists(), f"Required template missing: {tmpl_rel}")

            probe = inspect_template_source(path)
            self.assertTrue(probe.valid_syntax)
            self.assertTrue(probe.has_health_endpoint)
            self.assertEqual(probe.prohibited_imports_found, [])
            self.assertEqual(probe.errors, [])

    def test_sys_modules_unpolluted(self):
        """Invariant: Probe execution must never import GPU modules into sys.modules."""
        for mod in FORBIDDEN_MODULES:
            self.assertNotIn(
                mod,
                sys.modules,
                f"Forbidden module {mod} was imported into current python process!",
            )

    def test_staging_directory_verification(self):
        """Verifies normal staging directory with required templates passes."""
        result = verify_template_staging(self.templates_dir)
        self.assertTrue(result.all_templates_valid)
        self.assertEqual(result.missing_required_templates, [])
        self.assertEqual(len(result.prohibited_files_found), 0)
        self.assertEqual(result.errors, [])

    def test_failure_mode_missing_required_template(self):
        """Proves verify_template_staging fails when a required provider template is missing."""
        with tempfile.TemporaryDirectory() as tmpdir:
            tmp_path = Path(tmpdir)
            # Create only modal, omit beam
            (tmp_path / "modal").mkdir()
            (tmp_path / "modal" / "app.py").write_text("def handle_health(): pass\n")

            result = verify_template_staging(tmp_path)
            self.assertFalse(result.all_templates_valid)
            self.assertIn("beam/app.py", result.missing_required_templates)
            self.assertTrue(any("Missing required provider templates" in e for e in result.errors))

    def test_failure_mode_pre_read_oversized_file(self):
        """Proves inspect_template_source rejects oversized files BEFORE reading content."""
        with tempfile.TemporaryDirectory() as tmpdir:
            large_file = Path(tmpdir) / "large_app.py"
            # Write bytes exceeding MAX_TEMPLATE_FILE_SIZE_BYTES (100 KiB)
            large_size = MAX_TEMPLATE_FILE_SIZE_BYTES + 2048
            with open(large_file, "wb") as f:
                f.seek(large_size - 1)
                f.write(b"\0")

            probe = inspect_template_source(large_file)
            self.assertFalse(probe.valid_syntax)
            self.assertEqual(probe.size_bytes, large_size)
            self.assertTrue(any("exceeds maximum threshold" in e for e in probe.errors))

    def test_failure_mode_nested_gpu_import(self):
        """Proves AST walk catches prohibited GPU imports nested inside functions or blocks."""
        with tempfile.TemporaryDirectory() as tmpdir:
            nested_file = Path(tmpdir) / "nested_import.py"
            nested_file.write_text(
                "def helper():\n"
                "    if True:\n"
                "        import torch\n"
                "        from diffusers import StableDiffusionPipeline\n"
                "def handle_health(): pass\n"
            )

            probe = inspect_template_source(nested_file)
            self.assertFalse(probe.valid_syntax)
            self.assertIn("torch", probe.prohibited_imports_found)
            self.assertIn("diffusers", probe.prohibited_imports_found)
            self.assertTrue(any("Forbidden GPU/heavy modules imported" in e for e in probe.errors))

    def test_failure_mode_prohibited_weight_artifact(self):
        """Proves staging verification flags weight/binary artifacts in staging tree."""
        with tempfile.TemporaryDirectory() as tmpdir:
            tmp_path = Path(tmpdir)
            # Create required templates
            (tmp_path / "modal").mkdir()
            (tmp_path / "beam").mkdir()
            (tmp_path / "modal" / "app.py").write_text("def handle_health(): pass\n")
            (tmp_path / "beam" / "app.py").write_text("def handle_health(): pass\n")

            # Add weight artifact
            weight_file = tmp_path / "weights.safetensors"
            weight_file.write_bytes(b"dummy weight data")

            result = verify_template_staging(tmp_path)
            self.assertFalse(result.all_templates_valid)
            self.assertIn("weights.safetensors", result.prohibited_files_found)
            self.assertTrue(any("Prohibited weight/binary artifact" in e for e in result.errors))

    def test_failure_mode_syntax_error(self):
        """Proves inspect_template_source catches and reports Python syntax errors."""
        with tempfile.TemporaryDirectory() as tmpdir:
            bad_syntax = Path(tmpdir) / "syntax_err.py"
            bad_syntax.write_text("def invalid_syntax(\n")

            probe = inspect_template_source(bad_syntax)
            self.assertFalse(probe.valid_syntax)
            self.assertTrue(any("Syntax error in template" in e for e in probe.errors))

    def test_real_utc_timestamp_generated(self):
        """Verifies probe logger generates parseable real UTC ISO 8601 timestamp."""
        with tempfile.TemporaryDirectory() as tmpdir:
            log_file = Path(tmpdir) / "test_probe.log"
            run_probe_and_log(template_dir=self.templates_dir, log_file=log_file)

            self.assertTrue(log_file.exists())
            data = json.loads(log_file.read_text(encoding="utf-8"))
            ts = data.get("timestamp_utc")
            self.assertIsNotNone(ts)
            # Verify parseable as UTC ISO 8601
            dt = datetime.fromisoformat(ts)
            self.assertIsNotNone(dt.tzinfo)

    def test_ast_limitations_disclosed(self):
        """Verifies AST static analysis limitations are explicitly disclosed."""
        result = verify_template_staging(self.templates_dir)
        self.assertIn("Static AST analysis", result.limitations_noted)
        self.assertIn("does NOT prove zero runtime side effects", result.limitations_noted)


class TestStagingPlansInvariants(unittest.TestCase):
    """Tests for staging resource, P1 prerequisites, and unobserved cleanup guarantees."""

    def test_cpu_only_no_gpu_or_retention(self):
        for provider_id, plan in STAGING_PLANS.items():
            self.assertIsNone(
                plan.resource.gpu,
                f"Plan for {provider_id} violated CPU-only invariant by specifying GPU",
            )
            self.assertFalse(
                plan.resource.model_weights_required,
                f"Plan for {provider_id} requested model weights",
            )
            # Ensure no GPU retention parameters exist in resource endpoints
            self.assertNotIn("retention", str(plan.resource))

    def test_global_config_write_prohibited(self):
        for provider_id, plan in STAGING_PLANS.items():
            self.assertFalse(
                plan.auth.global_file_write_permitted,
                f"Plan for {provider_id} permitted writing to global CLI profile",
            )

    def test_p1_prerequisites_and_unobserved_cleanup(self):
        for provider_id, plan in STAGING_PLANS.items():
            self.assertTrue(len(plan.p1_prerequisite_gates) >= 3)
            self.assertIn("Unobserved", plan.cleanup_guarantee)
            for step in plan.cleanup_steps:
                self.assertFalse(
                    step.observed_in_production,
                    f"Cleanup step {step.action} claimed observed without test run",
                )


def run_all_tests():
    test_log_file = Path("/tmp/manga-cloud-p1-fixed-test.log")
    probe_log_file = Path("/tmp/manga-cloud-p1-fixed-probe.log")

    # Run probe and generate probe log
    run_probe_and_log(log_file=probe_log_file)

    suite = unittest.defaultTestLoader.loadTestsFromModule(sys.modules[__name__])
    runner = unittest.TextTestRunner(verbosity=2)
    result = runner.run(suite)

    log_entry = {
        "event": "p1_fixed_tests_completed",
        "tests_run": result.testsRun,
        "failures": len(result.failures),
        "errors": len(result.errors),
        "success": result.wasSuccessful(),
    }
    with open(test_log_file, "w", encoding="utf-8") as f:
        f.write(json.dumps(log_entry, indent=2))

    return result.wasSuccessful()


if __name__ == "__main__":
    success = run_all_tests()
    sys.exit(0 if success else 1)
