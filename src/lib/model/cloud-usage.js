export function monthStartMs(now = new Date()) {
  return new Date(now.getFullYear(), now.getMonth(), 1).getTime()
}
/** Unknown costs never read as a zero-dollar total. */
export function formatUsageCost(period, unreported = 'Not reported', incomplete = false) {
  if (incomplete && period.reportedUsd === 0) return unreported
  if (period.unpricedAttempts > 0 && period.attempts === period.unpricedAttempts) return unreported
  const dollars = new Intl.NumberFormat('en-US', { style: 'currency', currency: 'USD', minimumFractionDigits: 2, maximumFractionDigits: 4 }).format(period.reportedUsd)
  return incomplete || period.unpricedAttempts > 0 ? `${dollars} + ?` : dollars
}
