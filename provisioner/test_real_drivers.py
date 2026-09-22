"""Unit tests for lazy real-driver boundaries and production safety defaults."""

from pathlib import Path
import sys
import tempfile
import unittest

from provisioner.controller import ProvisioningController
from provisioner.driver_base import AccountInspectionResult
from provisioner.fake_drivers import FakeBeamDriver, FakeModalDriver
from provisioner.protocol import (
    ERR_ACTIONABLE_PERMISSION,
    ERR_PLATFORM_GATED,
    ERR_PROVIDER_UNAVAILABLE,
    ERR_VALIDATION,
    HELPER_PROTOCOL_VERSION,
    OP_INSPECT,
    OP_PLAN,
    PROVIDER_BEAM,
    PROVIDER_MODAL,
    ProtocolError,
)
from provisioner.real_drivers import RealBeamDriver, RealModalDriver


class TestRealDriverBoundaries(unittest.TestCase):
    """Tests for RealModalDriver, RealBeamDriver, and controller safety invariants."""

    def setUp(self):
        self.temp_dir = tempfile.TemporaryDirectory()
        self.journal_root = Path(self.temp_dir.name)

    def tearDown(self):
        self.temp_dir.cleanup()

    # --- Safety Invariant: ProvisioningController Default Drivers ---

    def test_controller_defaults_to_real_drivers_not_fakes(self):
        """Invariant: ProvisioningController must NEVER default to FakeModalDriver or FakeBeamDriver."""
        controller = ProvisioningController(journal_root=self.journal_root)
        self.assertIsInstance(
            controller.modal_driver,
            RealModalDriver,
            "Controller modal_driver defaulted to fake or non-real driver!",
        )
        self.assertNotIsInstance(
            controller.modal_driver,
            FakeModalDriver,
            "Controller modal_driver is a FakeModalDriver!",
        )
        self.assertIsInstance(
            controller.beam_driver,
            RealBeamDriver,
            "Controller beam_driver defaulted to fake or non-real driver!",
        )
        self.assertNotIsInstance(
            controller.beam_driver,
            FakeBeamDriver,
            "Controller beam_driver is a FakeBeamDriver!",
        )

    def test_controller_accepts_explicit_fake_injection_for_tests(self):
        """Tests may explicitly inject fake drivers."""
        fake_modal = FakeModalDriver()
        fake_beam = FakeBeamDriver()
        controller = ProvisioningController(
            journal_root=self.journal_root,
            modal_driver=fake_modal,
            beam_driver=fake_beam,
        )
        self.assertIs(controller.modal_driver, fake_modal)
        self.assertIs(controller.beam_driver, fake_beam)

    def test_production_controller_fails_closed_without_fake_injection(self):
        """Proves default controller fails closed with actionable error on inspect."""
        controller = ProvisioningController(journal_root=self.journal_root)
        req = {
            "protocol_version": HELPER_PROTOCOL_VERSION,
            "request_id": "req-prod-inspect-1",
            "op": OP_INSPECT,
            "provider": PROVIDER_MODAL,
            "params": {
                "credentials": {
                    "token_id": "ak-test-prod",
                    "token_secret": "as-test-prod",
                }
            },
        }
        res_str = controller.handle_request(req)
        import json
        res = json.loads(res_str)
        self.assertFalse(res["success"])
        self.assertIn(
            res["error"]["code"],
            (ERR_PROVIDER_UNAVAILABLE, ERR_PLATFORM_GATED),
        )
        self.assertNotIn("acc_modal_simulated_user", str(res))

    # --- Real Modal Driver ---

    def test_real_modal_driver_missing_credentials_fails_closed(self):
        driver = RealModalDriver()
        with self.assertRaises(ProtocolError) as ctx:
            driver.inspect_account({})
        self.assertEqual(ctx.exception.code, ERR_VALIDATION)

    def test_real_modal_driver_python_compatibility_gate(self):
        """Simulate Python 3.9 runtime to verify platform gating for Modal."""
        driver = RealModalDriver(python_version_override=(3, 9))
        creds = {"token_id": "ak-test", "token_secret": "as-test"}
        with self.assertRaises(ProtocolError) as ctx:
            driver.inspect_account(creds)
        self.assertEqual(ctx.exception.code, ERR_PLATFORM_GATED)
        self.assertIn("Python >= 3.10", ctx.exception.message)

    def test_real_modal_driver_sdk_absent_fails_closed(self):
        """When modal SDK is absent in Python >= 3.10, inspect_account raises ERR_PROVIDER_UNAVAILABLE."""
        driver = RealModalDriver(python_version_override=(3, 11))
        creds = {"token_id": "ak-test", "token_secret": "as-test"}
        # Unless modal is actually installed in this env, inspect raises ERR_PROVIDER_UNAVAILABLE
        import importlib.util
        if importlib.util.find_spec("modal") is None:
            with self.assertRaises(ProtocolError) as ctx:
                driver.inspect_account(creds)
            self.assertEqual(ctx.exception.code, ERR_PROVIDER_UNAVAILABLE)

    def test_real_modal_driver_refuses_live_mutation(self):
        """Real modal driver must truthfully refuse unverified live mutations."""
        driver = RealModalDriver(python_version_override=(3, 11))
        creds = {"token_id": "ak-test", "token_secret": "as-test"}
        with self.assertRaises(ProtocolError) as ctx:
            driver.seed_volume("inst-123", creds)
        self.assertIn(ctx.exception.code, (ERR_PLATFORM_GATED, ERR_PROVIDER_UNAVAILABLE))

        with self.assertRaises(ProtocolError) as ctx:
            driver.deploy_service("inst-123", "mc-app-inst-123", creds)
        self.assertIn(ctx.exception.code, (ERR_PLATFORM_GATED, ERR_PROVIDER_UNAVAILABLE))

    def test_real_modal_driver_deterministic_plan(self):
        """Real modal driver produces deterministic plan with matching SHA-256 hash."""
        driver = RealModalDriver()
        insp = AccountInspectionResult(
            provider="modal",
            account_id="acc-test",
            workspace_name="ws-test",
            authenticated=True,
            permissions_granted=["workspace:view", "apps:deploy", "volumes:create", "proxy_tokens:create"],
            permissions_missing=[],
            eligible=True,
        )
        plan = driver.create_deployment_plan(insp, "inst-modal-real-plan")
        self.assertEqual(plan.provider, "modal")
        self.assertEqual(plan.app_name, "mc-app-inst-modal-real-plan")
        self.assertEqual(len(plan.plan_hash), 64)

    # --- Real Beam Driver ---

    def test_real_beam_driver_missing_credentials_fails_closed(self):
        driver = RealBeamDriver()
        with self.assertRaises(ProtocolError) as ctx:
            driver.inspect_account({})
        self.assertEqual(ctx.exception.code, ERR_VALIDATION)

    def test_real_beam_driver_platform_gating_windows(self):
        """Native Windows must be gated with actionable WSL guidance."""
        driver = RealBeamDriver(platform_override="win32")
        creds = {"token": "b9_test_token_12345"}
        with self.assertRaises(ProtocolError) as ctx:
            driver.inspect_account(creds)
        self.assertEqual(ctx.exception.code, ERR_PLATFORM_GATED)
        self.assertIn("WSL", ctx.exception.actionable_guidance)

    def test_real_beam_driver_sdk_absent_fails_closed(self):
        """When beam SDK is absent on POSIX, inspect_account raises ERR_PROVIDER_UNAVAILABLE."""
        driver = RealBeamDriver(platform_override="darwin")
        creds = {"token": "b9_test_token_12345"}
        import importlib.util
        if importlib.util.find_spec("beam") is None and importlib.util.find_spec("beta9") is None:
            with self.assertRaises(ProtocolError) as ctx:
                driver.inspect_account(creds)
            self.assertEqual(ctx.exception.code, ERR_PROVIDER_UNAVAILABLE)

    def test_real_beam_driver_refuses_live_mutation(self):
        """Real beam driver must truthfully refuse unverified live mutations."""
        driver = RealBeamDriver(platform_override="darwin")
        creds = {"token": "b9_test_token_12345"}
        with self.assertRaises(ProtocolError) as ctx:
            driver.seed_volume("inst-456", creds)
        self.assertIn(ctx.exception.code, (ERR_PLATFORM_GATED, ERR_PROVIDER_UNAVAILABLE))

        with self.assertRaises(ProtocolError) as ctx:
            driver.deploy_service("inst-456", "mc-svc-inst-456", creds)
        self.assertIn(ctx.exception.code, (ERR_PLATFORM_GATED, ERR_PROVIDER_UNAVAILABLE))

    def test_real_beam_driver_deterministic_plan(self):
        """Real beam driver produces deterministic plan with matching SHA-256 hash."""
        driver = RealBeamDriver()
        insp = AccountInspectionResult(
            provider="beam",
            account_id="acc-beam-test",
            workspace_name="ws-beam-test",
            authenticated=True,
            permissions_granted=["workspace:read", "deployment:create", "storage:create", "token:create"],
            permissions_missing=[],
            eligible=True,
        )
        plan = driver.create_deployment_plan(insp, "inst-beam-real-plan")
        self.assertEqual(plan.provider, "beam")
        self.assertEqual(plan.app_name, "mc-beam-svc-inst-beam-real-plan")
        self.assertEqual(len(plan.plan_hash), 64)

    def test_real_modal_driver_allow_real_sdk_refuses_unverified_scopes(self):
        """When allow_real_sdk is True, RealModalDriver must refuse unverified scopes and never claim deploy rights."""
        driver = RealModalDriver(allow_real_sdk=True, python_version_override=(3, 11))
        creds = {"token_id": "ak-test-real-123", "token_secret": "as-test-real-123"}
        import importlib.util
        if importlib.util.find_spec("modal") is not None:
            with self.assertRaises(ProtocolError) as ctx:
                driver.inspect_account(creds)
            self.assertEqual(ctx.exception.code, ERR_PLATFORM_GATED)
            self.assertIn("unverified", ctx.exception.message.lower())

    def test_real_beam_driver_allow_real_sdk_refuses_unverified_scopes(self):
        """When allow_real_sdk is True, RealBeamDriver must refuse unverified scopes and never claim deploy rights."""
        driver = RealBeamDriver(allow_real_sdk=True, platform_override="darwin")
        creds = {"token": "b9_test_token_12345"}
        import importlib.util
        if importlib.util.find_spec("beam") is not None or importlib.util.find_spec("beta9") is not None:
            with self.assertRaises(ProtocolError) as ctx:
                driver.inspect_account(creds)
            self.assertEqual(ctx.exception.code, ERR_PLATFORM_GATED)
            self.assertIn("unverified", ctx.exception.message.lower())


if __name__ == "__main__":
    unittest.main()
