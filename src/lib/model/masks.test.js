/**
 * The Masks model's own decisions. `maskrows.test.js` covers the rows built on
 * top of these; this is the layer underneath - the fill-mode vocabulary, the
 * engines a machine may be asked for, and what the region context menu offers
 * for one region.
 */

import { describe, expect, it } from 'vitest'
import {
  FILL_MODES,
  ROW_ENGINES,
  nextFillMode,
  reRunnable,
  regionMenuSections,
  rowEngines,
} from './masks.js'

/**
 * @param {string} engine
 * @returns {any} a region carrying a mask from that engine
 */
function masked(engine) {
  return {
    id: 'c1-p001-r0',
    mask: { id: 'c1-p001-r0-m1', fillMode: 'match-surround', provenance: { engine } },
  }
}

/** @param {any} sections @returns {string[]} every item id, in order */
function ids(sections) {
  return sections.flatMap((section) => section.items.map((item) => item.id))
}

describe('the fill-mode cycle', () => {
  it('comes back to where it started', () => {
    let mode = FILL_MODES[0]
    for (const _ of FILL_MODES) mode = nextFillMode(mode)
    expect(mode).toBe(FILL_MODES[0])
  })
})

describe('rowEngines', () => {
  it('offers rung 3a only where the sidecar is installed', () => {
    expect(rowEngines({ flux: true })).toEqual([...ROW_ENGINES])
    expect(rowEngines({ flux: false })).not.toContain('flux')
    expect(rowEngines({ flux: false })).toContain('lama')
  })

  /**
   * The weights are downloaded after install, so a
   * fresh machine has no inpainter - and until this, the picker offered one.
   */
  it('drops a rung whose weights are not on this machine', () => {
    const offered = rowEngines({ flux: false, lama: false })
    expect(offered).toEqual(['fill', 'denoise'])
  })

  it('offers a rung the map says nothing about', () => {
    // The map records what is *missing*. A rung nobody has taught it about
    // must not vanish from every picker in the app.
    expect(rowEngines({ flux: true })).toContain('lama')
  })

  it('withholds only rung 3a before anything has asked the machine', () => {
    expect(rowEngines()).toEqual(['fill', 'denoise', 'lama'])
  })

  it('never offers the cloud, whatever the machine has', () => {
    expect(rowEngines({ flux: true })).not.toContain('cloud')
  })
})

describe('regionMenuSections', () => {
  it('offers Try again, the engines and Delete for an ordinary mask', () => {
    const sections = regionMenuSections(masked('lama'), { engines: { flux: false } })
    expect(ids(sections)).toEqual([
      'retry',
      'engine:fill',
      'engine:denoise',
      'engine:lama',
      'delete',
    ])
  })

  it('checks the engine the mask actually used, and only that one', () => {
    const sections = regionMenuSections(masked('lama'), { engines: { flux: false } })
    const engines = sections.find((section) => section.id === 'engine')
    expect(engines.items.filter((item) => item.selected).map((item) => item.id)).toEqual([
      'engine:lama',
    ])
  })

  it('heads the engines with the label the row picker uses', () => {
    const sections = regionMenuSections(masked('lama'))
    expect(sections.find((section) => section.id === 'engine').labelKey).toBe('masks.action.engine')
    // The two action sections carry no heading: one entry each, and a heading
    // over a single item is a label for nothing.
    expect(sections.find((section) => section.id === 'rerun').labelKey).toBe(null)
    expect(sections.find((section) => section.id === 'remove').labelKey).toBe(null)
  })

  it('offers rung 3a only where the sidecar is installed', () => {
    expect(ids(regionMenuSections(masked('lama'), { engines: { flux: true } }))).toContain('engine:flux')
    expect(ids(regionMenuSections(masked('lama'), { engines: { flux: false } }))).not.toContain('engine:flux')
    // The default is the safe one: a machine that has not answered yet is not
    // offered a rung that cannot run.
    expect(ids(regionMenuSections(masked('lama')))).not.toContain('engine:flux')
  })

  it('leaves out a rung whose weights are gone, and still names the one in use', () => {
    // A mask cleaned with LaMa on a machine that has since deleted LaMa's
    // weights: the entry has to lead the list so the menu is not claiming the
    // region has no engine, and it must not be offered to anything else.
    const sections = regionMenuSections(masked('lama'), {
      engines: { flux: false, lama: false },
    })
    expect(ids(sections)).toEqual([
      'retry',
      'engine:lama',
      'engine:fill',
      'engine:denoise',
      'delete',
    ])
  })

  it('leads with the rung in use even when the picker does not offer it', () => {
    // Not reachable for `cloud` - that mask is not re-runnable at all - but a
    // build that retires a rung must still name what a mask already ran on.
    const sections = regionMenuSections(masked('retired'), { engines: { flux: false } })
    const engines = sections.find((section) => section.id === 'engine')
    expect(engines.items[0]).toMatchObject({ id: 'engine:retired', selected: true })
  })

  it('offers Delete alone for a region with no mask, and names the region', () => {
    const sections = regionMenuSections({ id: 'c1-p001-r1', mask: null })
    expect(ids(sections)).toEqual(['delete'])
    expect(sections[0].items[0].labelKey).toBe('masks.action.deleteRegion')
  })

  it('offers Delete alone for a cloud mask - a re-run is another billable request', () => {
    expect(reRunnable(masked('cloud').mask)).toBe(false)
    const sections = regionMenuSections(masked('cloud'))
    expect(ids(sections)).toEqual(['delete'])
    expect(sections[0].items[0].labelKey).toBe('masks.action.delete')
  })

  it('gives every item a key the catalogue can answer, and a unique id', () => {
    const sections = regionMenuSections(masked('lama'), { engines: { flux: true } })
    const all = ids(sections)
    expect(new Set(all).size).toBe(all.length)
    for (const section of sections) {
      for (const item of section.items) {
        expect(item.labelKey, item.id).toMatch(/^masks\.[a-zA-Z]+\.[a-zA-Z]+$/)
      }
    }
  })
})
