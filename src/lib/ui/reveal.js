/**
 * Bring a row into view **without touching anything but its own list**.
 *
 * `Element.scrollIntoView({ block: 'nearest' })` is the obvious call and it is
 * the wrong one inside a floating window. It walks *every* scrollable ancestor
 * up to the document, and a box with `overflow: hidden` counts as scrollable:
 * it has no scrollbar, but it can be scrolled programmatically and nothing
 * ever scrolls it back. Measured in the editor (Layers window dragged so its
 * lower half hangs below the canvas area, then a row near its bottom pressed):
 * `.editor.scrollTop` went 0 → 137.5 and the viewport's bounding top went
 * 0 → -137.5, so the artwork and every pill slid up under the user's pointer
 * and stayed there.
 *
 * `.editor` is `overflow: clip` now, which cannot be scrolled at all, and
 * these helpers are the second half of that fix: the reveal moves the nearest
 * scrolling ancestor - the list body itself - and no other box on the page.
 *
 * Vertical only: every list this serves is a scrolling column.
 */

/** @typedef {{ top: number, bottom: number }} Span */

/**
 * How far a scroller has to move for `item` to be inside `view`, following
 * `nearest`: nothing at all when it is already in view, otherwise the smaller
 * of the two ends. An item taller than the view aligns its top, which is what
 * the browser does with `nearest` too.
 *
 * @param {Span} item - the row, in the same coordinate space as `view`
 * @param {Span} view - the scroller's visible band
 * @returns {number} pixels to add to `scrollTop`; 0 means leave it alone
 */
export function revealDelta(item, view) {
  if (item.top >= view.top && item.bottom <= view.bottom) return 0
  if (item.bottom - item.top > view.bottom - view.top) return item.top - view.top
  if (item.top < view.top) return item.top - view.top
  return item.bottom - view.bottom
}

/**
 * The nearest ancestor that actually scrolls: one whose overflow is `auto`,
 * `scroll` or `overlay` *and* which has content to scroll. A `hidden` or
 * `clip` ancestor is deliberately not a candidate - the point of this helper
 * is that those are never moved.
 *
 * @param {HTMLElement} element
 * @returns {HTMLElement|null}
 */
export function scrollerOf(element) {
  const view = element.ownerDocument?.defaultView
  if (!view) return null
  for (let node = element.parentElement; node; node = node.parentElement) {
    const style = view.getComputedStyle(node)
    const overflow = style.overflowY || style.overflow
    if (/(auto|scroll|overlay)/.test(overflow) && node.scrollHeight > node.clientHeight) {
      return node
    }
    // A clipped box is the end of the road: nothing outside it can bring the
    // element into view, and walking past it is exactly the ancestor-scrolling
    // `scrollIntoView` did to the editor shell.
    if (/clip/.test(overflow)) return null
  }
  return null
}

/**
 * Scroll `element`'s own list - and only its own list - until it is in view.
 *
 * @param {HTMLElement} element
 * @param {HTMLElement|null} [scroller] - the list, when the caller knows it
 */
export function reveal(element, scroller = scrollerOf(element)) {
  if (!scroller) return
  const item = element.getBoundingClientRect()
  const box = scroller.getBoundingClientRect()
  // The client area, not the border box: a border (or a classic scrollbar on
  // the other axis) is not somewhere a row can be visible.
  const top = box.top + scroller.clientTop
  const delta = revealDelta(item, { top, bottom: top + scroller.clientHeight })
  if (delta) scroller.scrollTop += delta
}

/**
 * Move focus without letting the browser scroll to it, then bring the target
 * into view through `reveal`. `HTMLElement.focus()` has the same
 * ancestor-walking scroll as `scrollIntoView`, so `preventScroll` is not
 * optional here.
 *
 * @param {HTMLElement} element
 * @param {boolean} [show] - whether to bring it into view afterwards
 */
export function focusAndReveal(element, show = true) {
  element.focus({ preventScroll: true })
  if (show) reveal(element)
}
