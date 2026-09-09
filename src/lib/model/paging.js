/**
 * Page navigation honouring reading direction. RTL is the default, set per project. The ‹ and
 * › controls occupy fixed visual positions; what they *do* depends on
 * direction, so this is the one place that reasons about it - no component
 * should independently decide whether ‹ goes forward or back.
 */

/**
 * @returns {'rtl'|'ltr'} the project default when none is set
 */
export function defaultReadingDirection() {
  return 'rtl'
}

/**
 * @typedef {Object} NavControl
 * @property {'next'|'prev'} action
 * @property {string} tooltipKey
 */

/**
 * @param {'rtl'|'ltr'} direction
 * @returns {{ left: NavControl, right: NavControl }} what the ‹ (left) and › (right) controls do
 */
export function pageNavControls(direction) {
  const rtl = direction === 'rtl'
  return {
    left: rtl
      ? { action: 'next', tooltipKey: 'paging.action.next' }
      : { action: 'prev', tooltipKey: 'paging.action.prev' },
    right: rtl
      ? { action: 'prev', tooltipKey: 'paging.action.prev' }
      : { action: 'next', tooltipKey: 'paging.action.next' },
  }
}
