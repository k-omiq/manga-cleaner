from decimal import Decimal
from types import SimpleNamespace
import unittest
from provisioner.billing import modal_billing
from provisioner.protocol import ProtocolError

class BillingTests(unittest.TestCase):
    def test_summary_preserves_metered_and_invoiced_cost(self):
        cycles = []
        def summary(*, cycle):
            cycles.append(cycle)
            return SimpleNamespace(metered_cost=Decimal('2.1234'), billed_cost=Decimal('0'))
        session = SimpleNamespace(workspace=SimpleNamespace(billing=SimpleNamespace(summary=summary)), workspace_name='studio')
        self.assertEqual(modal_billing(session, '2026-09'), {'workspace':'studio', 'cycle':'2026-09', 'metered_cost':2.1234, 'billed_cost':0.0})
        self.assertEqual(cycles, ['2026-09'])
    def test_missing_api_is_unavailable_not_free(self):
        with self.assertRaises(ProtocolError): modal_billing(SimpleNamespace(workspace=object()), '2026-09')
    def test_invalid_dates_and_prices_rejected(self):
        for cycle in ('2026-13', 'secret', None):
            with self.assertRaises(ProtocolError): modal_billing(None, cycle)
        for cost in (None, float('nan'), -1):
            session = SimpleNamespace(workspace=SimpleNamespace(billing=SimpleNamespace(summary=lambda **kw: SimpleNamespace(metered_cost=cost, billed_cost=0))), workspace_name='studio')
            with self.assertRaises(ProtocolError): modal_billing(session, '2026-09')
