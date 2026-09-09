import { describe, expect, it } from 'vitest'
import { segmentedTabStop } from '../editor/tools.js'

describe('Segmented roving tab stop', () => {
  it('places the tab stop on the selected option when enabled', () => {
    const items = [
      { value: 'page', label: 'Page', disabled: false },
      { value: 'project', label: 'Project', disabled: false },
    ]
    expect(segmentedTabStop(items, 0)).toBe(0)
    expect(segmentedTabStop(items, 1)).toBe(1)
  })

  it('moves the tab stop to the first enabled option when the selected option is disabled', () => {
    const items = [
      { value: 'local', label: 'Local', disabled: true },
      { value: 'cloud', label: 'Cloud', disabled: false },
    ]
    // Option 0 is selected but disabled; roving tab stop must fall back to option 1
    expect(segmentedTabStop(items, 0)).toBe(1)
  })

  it('places the tab stop on the first enabled option when unselected', () => {
    const items = [
      { value: 'a', label: 'A', disabled: true },
      { value: 'b', label: 'B', disabled: false },
      { value: 'c', label: 'C', disabled: false },
    ]
    expect(segmentedTabStop(items, -1)).toBe(1)
  })

  it('yields -1 when no option is enabled', () => {
    const items = [
      { value: 'a', label: 'A', disabled: true },
      { value: 'b', label: 'B', disabled: true },
    ]
    expect(segmentedTabStop(items, 0)).toBe(-1)
    expect(segmentedTabStop(items, -1)).toBe(-1)
  })

  it('yields -1 when the entire group is disabled', () => {
    const items = [
      { value: 'a', label: 'A', disabled: false },
      { value: 'b', label: 'B', disabled: false },
    ]
    const stopFor = (items, selected, disabled) =>
      disabled
        ? -1
        : selected >= 0 && !items[selected]?.disabled
          ? selected
          : items.findIndex((o) => !o.disabled)

    expect(stopFor(items, 0, true)).toBe(-1)
    expect(stopFor(items, 1, true)).toBe(-1)
    expect(stopFor(items, 0, false)).toBe(0)
  })
})
