"""Read-only Modal workspace billing; no inference or resource creation."""
import math
from provisioner.protocol import ERR_PROVIDER_UNAVAILABLE, ERR_VALIDATION, ProtocolError


def modal_billing(session, cycle):
    if not isinstance(cycle, str) or len(cycle) != 7:
        raise ProtocolError(ERR_VALIDATION, "Billing cycle must be YYYY-MM")
    from datetime import datetime
    try:
        datetime.strptime(cycle, "%Y-%m")
    except ValueError:
        raise ProtocolError(ERR_VALIDATION, "Billing cycle must be YYYY-MM") from None
    billing = getattr(session.workspace, "billing", None)
    if not callable(getattr(billing, "summary", None)):
        raise ProtocolError(ERR_PROVIDER_UNAVAILABLE, "This Modal SDK does not support billing summaries")
    summary = billing.summary(cycle=cycle)
    result = {"workspace": session.workspace_name, "cycle": cycle}
    for name in ("metered_cost", "billed_cost"):
        raw = getattr(summary, name, None)
        try:
            value = float(raw)
        except (TypeError, ValueError, OverflowError):
            raise ProtocolError(ERR_PROVIDER_UNAVAILABLE, "Modal returned an invalid billing summary") from None
        if not math.isfinite(value) or value < 0:
            raise ProtocolError(ERR_PROVIDER_UNAVAILABLE, "Modal returned an invalid billing summary")
        result[name] = value
    return result
