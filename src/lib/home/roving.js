/**
 * Arrow-key movement for the two collections on Home: the project grid and
 * the chapter table. Both are single-tab-stop widgets - one roving tabindex,
 * arrows to move, Enter to open - so a library of forty projects costs one
 * Tab, not forty.
 *
 * Movement clamps rather than wraps. In a grid, wrapping off the end of a row
 * lands somewhere the eye is not, and there is no reading order that makes it
 * predictable in both directions.
 */

/**
 * @param {string} key - `KeyboardEvent.key`
 * @param {{index: number, count: number, columns?: number, orientation?: 'grid'|'list'}} context
 * @returns {number|null} the index to move to, or null when the key is not ours
 */
export function nextIndex(key, { index, count, columns = 1, orientation = 'grid' }) {
  if (count === 0) return null
  const step = orientation === 'grid' ? Math.max(1, columns) : 1
  let next = null

  switch (key) {
    case 'ArrowRight':
      if (orientation === 'list') return null
      next = index + 1
      break
    case 'ArrowLeft':
      if (orientation === 'list') return null
      next = index - 1
      break
    case 'ArrowDown':
      next = orientation === 'grid' ? index + step : index + 1
      break
    case 'ArrowUp':
      next = orientation === 'grid' ? index - step : index - 1
      break
    case 'Home':
      next = 0
      break
    case 'End':
      next = count - 1
      break
    default:
      return null
  }

  const clamped = Math.min(count - 1, Math.max(0, next))
  return clamped === index ? null : clamped
}

/**
 * How many columns the grid is actually rendering. Read from the resolved
 * `grid-template-columns`, so it follows `auto-fill` at whatever width the
 * window happens to be, with no resize observer and no guessing.
 *
 * @param {HTMLElement|undefined} element
 * @returns {number}
 */
export function columnCount(element) {
  if (!element) return 1
  const template = getComputedStyle(element).gridTemplateColumns
  if (!template || template === 'none') return 1
  return Math.max(1, template.split(' ').filter(Boolean).length)
}

/**
 * Move focus to the item at `index`. Items mark themselves with
 * `data-roving-item`, so no ref plumbing crosses a component boundary.
 *
 * @param {HTMLElement|undefined} container
 * @param {number} index
 */
export function focusItem(container, index) {
  const items = container?.querySelectorAll('[data-roving-item]')
  const target = /** @type {HTMLElement|undefined} */ (items?.[index])
  target?.focus()
}
