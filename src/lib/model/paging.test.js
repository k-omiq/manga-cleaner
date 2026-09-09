import { describe, expect, it } from 'vitest'
import { pageNavControls } from './paging.js'

describe('pageNavControls', () => {
  it('RTL maps ‹ to the next page and › back to the previous', () => {
    const controls = pageNavControls('rtl')
    expect(controls.left).toEqual({ action: 'next', tooltipKey: 'paging.action.next' })
    expect(controls.right).toEqual({ action: 'prev', tooltipKey: 'paging.action.prev' })
  })

  it('LTR maps ‹ to the previous page and › forward to the next', () => {
    const controls = pageNavControls('ltr')
    expect(controls.left).toEqual({ action: 'prev', tooltipKey: 'paging.action.prev' })
    expect(controls.right).toEqual({ action: 'next', tooltipKey: 'paging.action.next' })
  })
})
