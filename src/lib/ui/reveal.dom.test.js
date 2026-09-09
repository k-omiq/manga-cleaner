import { afterEach, describe, expect, it } from 'vitest'
import { focusAndReveal, reveal, scrollerOf } from './reveal.js'

/**
 * The point of these helpers is what they do *not* touch: a clipped ancestor  - 
 * the editor shell, in the app - must keep its `scrollTop` at 0 while the list
 * inside a floating window scrolls. `scrollIntoView` moved both, which is the
 * jump this avoids.
 *
 * jsdom lays nothing out, so the geometry is stubbed: `getBoundingClientRect`
 * on the row and the list, and the scroll metrics that decide whether a box
 * counts as a scroller.
 */

/**
 * @param {HTMLElement} el
 * @param {{ scrollHeight?: number, clientHeight?: number, clientTop?: number }} metrics
 */
function measure(el, metrics) {
  for (const [key, value] of Object.entries(metrics)) {
    Object.defineProperty(el, key, { value, configurable: true })
  }
}

/** @param {HTMLElement} el @param {number} top @param {number} height */
function rect(el, top, height) {
  el.getBoundingClientRect = () =>
    /** @type {DOMRect} */ (/** @type {unknown} */ ({ top, bottom: top + height, height }))
}

function build() {
  document.body.innerHTML = `
    <div class="clipped" style="overflow: hidden">
      <div class="window" style="overflow: hidden">
        <div class="body" style="overflow-y: auto">
          <button class="row">row</button>
        </div>
      </div>
    </div>`
  const clipped = /** @type {HTMLElement} */ (document.querySelector('.clipped'))
  const win = /** @type {HTMLElement} */ (document.querySelector('.window'))
  const list = /** @type {HTMLElement} */ (document.querySelector('.body'))
  const row = /** @type {HTMLElement} */ (document.querySelector('.row'))
  // Both boxes above the list have content to scroll, so `scrollIntoView`
  // would have moved them; only their overflow keeps them out of it.
  measure(clipped, { scrollHeight: 1200, clientHeight: 600, clientTop: 0 })
  measure(win, { scrollHeight: 900, clientHeight: 300, clientTop: 0 })
  measure(list, { scrollHeight: 900, clientHeight: 200, clientTop: 0 })
  rect(list, 100, 200)
  return { clipped, win, list, row }
}

afterEach(() => {
  document.body.innerHTML = ''
})

describe('scrollerOf', () => {
  it('picks the nearest ancestor that actually scrolls', () => {
    const { list, row } = build()
    expect(scrollerOf(row)).toBe(list)
  })

  it('skips an ancestor whose content fits', () => {
    const { list, win, row } = build()
    measure(list, { scrollHeight: 200, clientHeight: 200 })
    // The window is `overflow: hidden`, so there is no scroller above it.
    expect(scrollerOf(row)).toBe(null)
    expect(win.scrollTop).toBe(0)
  })
})

describe('reveal', () => {
  it('scrolls the list, and leaves the clipped ancestors at rest', () => {
    const { clipped, win, list, row } = build()
    rect(row, 360, 40) // 100px below the list's bottom edge
    reveal(row)
    expect(list.scrollTop).toBe(100)
    expect(clipped.scrollTop).toBe(0)
    expect(win.scrollTop).toBe(0)
  })

  it('does nothing for a row that is already visible', () => {
    const { list, row } = build()
    rect(row, 140, 40)
    list.scrollTop = 25
    reveal(row)
    expect(list.scrollTop).toBe(25)
  })

  it('takes the scroller the caller names', () => {
    const { list, row } = build()
    rect(row, 20, 40) // above the list
    reveal(row, list)
    expect(list.scrollTop).toBe(-80)
  })
})

describe('focusAndReveal', () => {
  it('focuses without the browser scrolling to the target', () => {
    const { clipped, list, row } = build()
    rect(row, 360, 40)
    /** @type {ScrollIntoViewOptions|undefined} */
    let asked
    row.focus = (/** @type {FocusOptions} */ options) => {
      asked = options
    }
    focusAndReveal(row)
    expect(asked).toEqual({ preventScroll: true })
    expect(list.scrollTop).toBe(100)
    expect(clipped.scrollTop).toBe(0)
  })

  it('can focus without revealing', () => {
    const { list, row } = build()
    rect(row, 360, 40)
    row.focus = () => {}
    focusAndReveal(row, false)
    expect(list.scrollTop).toBe(0)
  })
})
