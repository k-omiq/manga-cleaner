/**
 * The Masks model's own decisions. `maskrows.test.js` covers the rows built on
 * top of these; this is the layer underneath - the fill-mode vocabulary, the
 * engines a machine may be asked for, and what the region context menu offers
 * for one region.
 */

import { describe, expect, it } from 'vitest'
import {
  CLOUD_ENGINE,
  FILL_MODES,
  ROW_ENGINES,
  isCloudMask,
  maskEngine,
  nextFillMode,
  provenanceFacts,
  reRunnable,
  regionMenuSections,
  rowEngines,
} from './masks.js'
import { rungLabel } from './ladder.js'

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

  it('offers the cloud last, and only when an endpoint is ready', () => {
    expect(rowEngines({ flux: true })).not.toContain(CLOUD_ENGINE)
    expect(rowEngines({ flux: true }, { cloud: false })).not.toContain(CLOUD_ENGINE)
    expect(rowEngines({ flux: true }, { cloud: true })).toEqual([...ROW_ENGINES, CLOUD_ENGINE])
    expect(rowEngines(undefined, { cloud: true })).toEqual(['fill', 'denoise', 'lama', CLOUD_ENGINE])
  })

  it('keeps the cloud out of the shared rung list, which automatic runs read', () => {
    // `editor/tools.js` builds Auto clean's engine rows and the AI mask
    // brush's from ROW_ENGINES; an automatic run never reaches the cloud.
    expect(ROW_ENGINES).not.toContain(CLOUD_ENGINE)
  })
})

describe('cloud masks', () => {
  const remote = {
    id: 'c1-p001-r0',
    mask: {
      id: 'c1-p001-r0-m1',
      fillMode: 'reconstruct',
      provenance: { engine: 'flux', cloud: { provider: 'modal', request_id: 'req-123', cost: null } },
    },
  }

  it('are the legacy cloud engine, or a patch with a cloud record, and read as Cloud', () => {
    expect(isCloudMask(masked('cloud').mask)).toBe(true)
    expect(isCloudMask(remote.mask)).toBe(true)
    expect(maskEngine(remote.mask)).toBe(CLOUD_ENGINE)
    expect(isCloudMask(masked('flux').mask)).toBe(false)
    expect(maskEngine(masked('flux').mask)).toBe('flux')
    expect(maskEngine(null)).toBe(null)
  })

  it('may be tried again, which asks for consent again', () => {
    expect(reRunnable(remote.mask)).toBe(true)
    expect(reRunnable(masked('cloud').mask)).toBe(true)
    expect(reRunnable(null)).toBe(false)
  })

  it('keep Cloud checked in the menu, leading the list while the cloud is not ready', () => {
    const sections = regionMenuSections(remote, { engines: { flux: false } })
    expect(ids(sections)).toEqual(['retry', 'engine:cloud', 'engine:fill', 'engine:denoise', 'engine:lama', 'delete'])
    const engines = sections.find((section) => section.id === 'engine')
    expect(engines.items.filter((item) => item.selected).map((item) => item.id)).toEqual(['engine:cloud'])
  })

  it('take Cloud in its own place once the cloud is ready', () => {
    const sections = regionMenuSections(remote, { engines: { flux: false }, cloud: true })
    expect(ids(sections)).toEqual(['retry', 'engine:fill', 'engine:denoise', 'engine:lama', 'engine:cloud', 'delete'])
  })
})

describe('regionMenuSections', () => {
  it('offers approval and available local models for a gated review item', () => {
    const region = { id: 'c1-p001-r1', outcome: 'gate-skipped', mask: null }
    expect(ids(regionMenuSections(region, { engines: { flux: false }, cloud: true }))).toEqual([
      'approve', 'approve:fill', 'approve:denoise', 'approve:lama', 'delete',
    ])
  })

  it('offers only Delete for an approved component whose sidecar cannot be replaced safely', () => {
    const region = masked('fill')
    region.id = 'c1-p001-hreview-sam-00001-deadbeef'
    region.mask.id = `${region.id}-m1`
    expect(reRunnable(region.mask)).toBe(false)
    expect(ids(regionMenuSections(region))).toEqual(['delete'])
  })

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
    // A build that retires a rung must still name what a mask already ran on.
    const sections = regionMenuSections(masked('retired'), { engines: { flux: false } })
    const engines = sections.find((section) => section.id === 'engine')
    expect(engines.items[0]).toMatchObject({ id: 'engine:retired', selected: true })
  })

  it('offers Delete alone for a region with no mask, and names the region', () => {
    const sections = regionMenuSections({ id: 'c1-p001-r1', mask: null })
    expect(ids(sections)).toEqual(['delete'])
    expect(sections[0].items[0].labelKey).toBe('masks.action.deleteRegion')
  })

  it('offers Cloud last for a local mask when the cloud is ready, and not otherwise', () => {
    expect(ids(regionMenuSections(masked('lama'), { engines: { flux: false }, cloud: true }))).toEqual([
      'retry',
      'engine:fill',
      'engine:denoise',
      'engine:lama',
      'engine:cloud',
      'delete',
    ])
    expect(ids(regionMenuSections(masked('lama'), { engines: { flux: false }, cloud: false }))).not.toContain(
      'engine:cloud',
    )
    const engines = regionMenuSections(masked('lama'), { cloud: true }).find((section) => section.id === 'engine')
    expect(engines.items.find((item) => item.id === 'engine:cloud')).toMatchObject({
      labelKey: 'masks.engineChoice.cloud',
      selected: false,
    })
  })

  it('allows reRunnable for local FLUX masks without cloud provenance', () => {
    const localFlux = {
      id: 'c1-p001-r0',
      mask: {
        id: 'c1-p001-r0-m1',
        fillMode: 'reconstruct',
        provenance: {
          engine: 'flux',
          cloud: null,
        },
      },
    }
    expect(reRunnable(localFlux.mask)).toBe(true)
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

/**
 * A paint or clone stroke is copied pixels, not a cleaning: there is nothing
 * to run again from the layers below it (docs/repeated-inpaint-plan.md, M1
 * item 5). Try again and Clean with are refused for it everywhere they are
 * offered, and the layer keeps the name of the tool that made it rather than
 * borrowing an engine's.
 */
describe('paint and clone masks', () => {
  for (const engine of ['paint', 'clone']) {
    it(`refuse Try again and every engine for a ${engine} stroke, and keep Delete`, () => {
      const region = masked(engine)
      expect(reRunnable(region.mask)).toBe(false)
      // Whatever the machine could run and whether the cloud is ready: an
      // engine list would be an offer to re-clean something never cleaned.
      for (const options of [{}, { engines: { flux: true } }, { engines: { flux: true }, cloud: true }]) {
        const sections = regionMenuSections(region, options)
        expect(ids(sections)).toEqual(['delete'])
        expect(sections.map((section) => section.id)).toEqual(['remove'])
      }
    })

    it(`deletes a ${engine} stroke as a layer, not as a region`, () => {
      const [remove] = regionMenuSections(masked(engine))
      expect(remove.items[0]).toMatchObject({ id: 'delete', labelKey: 'masks.action.delete' })
    })

    it(`names a ${engine} stroke by the tool that made it, and never as Cloud`, () => {
      const { mask } = masked(engine)
      expect(maskEngine(mask)).toBe(engine)
      expect(isCloudMask(mask)).toBe(false)
      expect(rungLabel(/** @type {string} */ (maskEngine(mask)))).toBe(`ladder.rung.${engine}`)
    })
  }
})

describe('provenanceFacts', () => {
  it('retains cloudCost as null when cost is null so UI displays unknown dash', () => {
    const maskWithNullCost = {
      fillMode: 'reconstruct',
      elapsedMs: 2500,
      provenance: {
        engine: 'flux',
        engine_version: 'flux-sdnq-v1',
        cloud: {
          provider: 'beam',
          request_id: 'req-456',
          cost: null,
        },
      },
    }
    const facts = provenanceFacts(maskWithNullCost)
    expect(facts).toContainEqual({ key: 'masks.provenance.cloudCost', value: null })
    expect(facts).toContainEqual({ key: 'masks.provenance.cloudRequestId', value: 'req-456' })
  })

  it('includes cloudCost when cost is a valid positive number', () => {
    const maskWithCost = {
      fillMode: 'reconstruct',
      elapsedMs: 1800,
      provenance: {
        engine: 'flux',
        engine_version: 'flux-sdnq-v1',
        cloud: {
          provider: 'modal',
          request_id: 'req-789',
          cost: 0.025,
        },
      },
    }
    const facts = provenanceFacts(maskWithCost)
    expect(facts).toContainEqual({ key: 'masks.provenance.cloudCost', value: 0.025 })
    expect(facts).toContainEqual({ key: 'masks.provenance.cloudRequestId', value: 'req-789' })
  })

  it('omits cloud facts entirely for local masks', () => {
    const localMask = {
      fillMode: 'match-surround',
      elapsedMs: 30,
      provenance: {
        engine: 'fill',
        engine_version: 'planar-1.4',
        cloud: null,
      },
    }
    const facts = provenanceFacts(localMask)
    expect(facts.map((f) => f.key)).not.toContain('masks.provenance.cloudCost')
    expect(facts.map((f) => f.key)).not.toContain('masks.provenance.cloudRequestId')
  })
})
