/**
 * The first-launch offer's arithmetic.
 *
 * No DOM and no mount: what is being decided is *what is missing, what is
 * ticked and how many bytes that is*, and every one of those is a function of
 * one `listModels` answer. The dialog around it is a sequence of calls and an
 * event subscription, and `FirstLaunchDialog.dom.test.js` mounts it for those.
 *
 * The fixture is the real catalogue - the five ids, the real `requiredBy` sets
 * and the real sizes from `src-tauri/src/weights.rs` - because a plan built
 * over invented rows would prove the arithmetic and not the grouping, and the
 * grouping is the half that reads `requiredBy` rather than a list of its own.
 * It is five rather than six because the preview engine's row left the
 * catalogue with the engine it belonged to, which leaves `inpainter` as the
 * whole of the optional group.
 */

import { describe, expect, it } from 'vitest'

import {
  RUNTIME_ID,
  downloadQueue,
  firstLaunchPlan,
  initialSelection,
  labelKeyFor,
  plannedBytes,
} from './firstlaunch.js'

const LAMA_BYTES = 207_482_644
const RUNTIME_BYTES = 32_396_562
const BALLOON_BYTES = 11_120_765
/** The four Auto clean artefacts, summed. */
const AUTO_CLEAN_BYTES = 94_669_756 + 3_722_314 + 1_163 + BALLOON_BYTES

/**
 * A `ModelsView` with everything missing unless named in `installed`.
 *
 * @param {{installed?: string[], runtime?: Object|null}} [options]
 */
function view({ installed = [], runtime = {} } = {}) {
  const has = (id) => installed.includes(id)
  return {
    models: [
      { id: 'textDetector', kindKey: 'models.kind.textDetector', bytes: 94_669_756, requiredBy: ['autoClean'], installed: has('textDetector') },
      { id: 'inpainter', kindKey: 'models.kind.inpainter', bytes: LAMA_BYTES, requiredBy: ['lama'], installed: has('inpainter') },
      { id: 'scriptGate', kindKey: 'models.kind.scriptGate', bytes: 3_722_314, requiredBy: ['autoClean'], installed: has('scriptGate') },
      { id: 'scriptGateLabels', kindKey: 'models.kind.scriptGateLabels', bytes: 1_163, requiredBy: ['autoClean'], installed: has('scriptGateLabels') },
      { id: 'balloonDetector', kindKey: 'models.kind.balloonDetector', bytes: BALLOON_BYTES, requiredBy: ['autoClean'], installed: has('balloonDetector') },
    ],
    runtime:
      runtime === null
        ? undefined
        : { installed: has(RUNTIME_ID), bytes: RUNTIME_BYTES, available: true, ...runtime },
  }
}

/** Everything the catalogue names, for the "nothing to offer" case. */
const EVERYTHING = [
  'textDetector',
  'inpainter',
  'scriptGate',
  'scriptGateLabels',
  'balloonDetector',
  RUNTIME_ID,
]

describe('what a first launch has to offer', () => {
  it('groups the Auto clean set and the runtime as the required one', () => {
    const plan = firstLaunchPlan(view())
    expect(plan.required.map((row) => row.id)).toEqual([
      'textDetector',
      'scriptGate',
      'scriptGateLabels',
      'balloonDetector',
      // Last, because it is the one row that is not a weight.
      RUNTIME_ID,
    ])
    // The redraw engine is a choice and is in neither of the other's group.
    expect(plan.optional.map((row) => row.id)).toEqual(['inpainter'])
  })

  it('sums the required group from the view rather than from a constant', () => {
    expect(firstLaunchPlan(view()).requiredBytes).toBe(AUTO_CLEAN_BYTES + RUNTIME_BYTES)
    // An artefact already here is not charged for again.
    expect(firstLaunchPlan(view({ installed: ['balloonDetector'] })).requiredBytes).toBe(
      AUTO_CLEAN_BYTES + RUNTIME_BYTES - BALLOON_BYTES,
    )
  })

  it('ticks the redraw engine and nothing the catalogue did not name', () => {
    const plan = firstLaunchPlan(view())
    const selection = initialSelection(plan)
    // LaMa is the rung an ordinary first run actually reaches, so it arrives
    // ticked rather than as an offer the user has to notice.
    expect(selection.inpainter).toBe(true)
    // Everything missing from the required group is selected and there is no
    // control to unselect it: it is what Auto clean is made of.
    for (const row of plan.required) expect(selection[row.id]).toBe(true)
    // And nothing else is in it. With one optional row left there is no second
    // engine to be silently ticked, so what this pins is that the selection is
    // the *plan's* rows: an id from neither group is an id the press cannot
    // fetch and the button must not price.
    expect(Object.keys(selection).sort()).toEqual(
      [...plan.required, ...plan.optional].map((row) => row.id).sort(),
    )
  })

  it('offers nothing once the required group is here', () => {
    expect(firstLaunchPlan(view({ installed: EVERYTHING })).offer).toBe(false)
    // A missing *optional* engine is not a reason to open a modal over
    // somebody's library: everything that cleans a page is here, and the
    // redraw engine has a row in Settings to be asked for from. This was
    // MI-GAN's case and it is LaMa's now that LaMa is the whole choice.
    const partial = EVERYTHING.filter((id) => id !== 'inpainter')
    expect(firstLaunchPlan(view({ installed: partial })).offer).toBe(false)
    // One artefact short of the set is the whole reason the offer exists.
    expect(firstLaunchPlan(view({ installed: partial.filter((id) => id !== 'scriptGate') })).offer).toBe(true)
    // A runtime alone is enough: nothing runs without one.
    expect(firstLaunchPlan(view({ installed: partial.filter((id) => id !== RUNTIME_ID) })).offer).toBe(true)
  })

  it('leaves an installed optional engine out of the choice entirely', () => {
    // A ticked checkbox over something already on disk is a control whose only
    // honest answer is the one it already has. With MI-GAN gone there is one
    // optional engine, so dropping it empties the group rather than
    // shortening it - and the offer is still made, because the required set is
    // what it is made of. `FirstLaunchDialog.dom.test.js` asserts the other
    // half of this: an empty group is drawn as no section at all, not as a
    // heading over nothing.
    const plan = firstLaunchPlan(view({ installed: ['inpainter'] }))
    expect(plan.optional).toEqual([])
    expect(plan.offer).toBe(true)
    // And it is out of the selection too, so the press cannot re-fetch it.
    expect(initialSelection(plan)).not.toHaveProperty('inpainter')
    expect(downloadQueue(plan, initialSelection(plan))).not.toContain('inpainter')
  })

  it('leaves out a runtime this platform has no build for', () => {
    // An Intel Mac. A row with no download behind it would make
    // the button promise bytes that can never arrive.
    const plan = firstLaunchPlan(view({ runtime: { available: false } }))
    expect(plan.required.map((row) => row.id)).not.toContain(RUNTIME_ID)
    expect(plan.requiredBytes).toBe(AUTO_CLEAN_BYTES)
    // The weights are still worth fetching: a runtime installed later finds
    // them, and they are what the offline install needs beside a
    // library placed by hand. The dialog says so, which is what this flag is
    // for - an offer with no runtime in it and nothing to explain that would
    // read as a promise it cannot keep.
    expect(plan.offer).toBe(true)
    expect(plan.runtimeUnavailable).toBe(true)
  })

  it('says nothing about the runtime on a platform that has one', () => {
    expect(firstLaunchPlan(view()).runtimeUnavailable).toBe(false)
    expect(firstLaunchPlan(view({ installed: EVERYTHING })).runtimeUnavailable).toBe(false)
    // A view with no runtime at all is an adapter older than the row, not a
    // platform without a build, and has nothing to say either.
    expect(firstLaunchPlan(view({ runtime: null })).runtimeUnavailable).toBe(false)
  })

  it('charges nothing for a size the view cannot state', () => {
    // `runtime.bytes` is nullable on the wire, and an invented figure is a
    // promise the button cannot keep.
    const plan = firstLaunchPlan(view({ runtime: { bytes: null } }))
    expect(plan.requiredBytes).toBe(AUTO_CLEAN_BYTES)
    expect(plan.required.at(-1)).toMatchObject({ id: RUNTIME_ID, bytes: 0 })
  })

  it('survives a view that answered nothing at all', () => {
    for (const empty of [null, undefined, {}, { models: 'not a list' }]) {
      const plan = firstLaunchPlan(/** @type {any} */ (empty))
      expect(plan.offer).toBe(false)
      expect(plan.required).toEqual([])
      expect(plan.optional).toEqual([])
      expect(plan.requiredBytes).toBe(0)
    }
  })
})

describe('what the press will cost and fetch', () => {
  it('counts the ticked rows and only the ticked rows', () => {
    const plan = firstLaunchPlan(view())
    const selection = initialSelection(plan)
    expect(plannedBytes(plan, selection)).toBe(AUTO_CLEAN_BYTES + RUNTIME_BYTES + LAMA_BYTES)
    // The one tick the dialog offers, taken off. This used to be read the
    // other way round - MI-GAN ticked *on* - and unticking is the half that
    // survives the removal, which is also the only half a user can now reach.
    expect(plannedBytes(plan, { ...selection, inpainter: false })).toBe(
      AUTO_CLEAN_BYTES + RUNTIME_BYTES,
    )
    // Ticked and already here is charged for nothing: a row is counted once,
    // and only while it is *missing*. An artefact that arrived while the
    // dialog was open stops being priced even with its tick still on.
    const partly = firstLaunchPlan(view({ installed: ['balloonDetector'] }))
    const rest = AUTO_CLEAN_BYTES - BALLOON_BYTES + RUNTIME_BYTES + LAMA_BYTES
    expect(plannedBytes(partly, initialSelection(partly))).toBe(rest)
    expect(plannedBytes(partly, { ...initialSelection(partly), balloonDetector: true })).toBe(rest)
    // An id from neither group is not a row, so it is not a price.
    expect(plannedBytes(plan, { ...selection, somethingNewer: true })).toBe(
      AUTO_CLEAN_BYTES + RUNTIME_BYTES + LAMA_BYTES,
    )
    expect(plannedBytes(plan, {})).toBe(0)
  })

  it('fetches the runtime first and then the weights in catalogue order', () => {
    const plan = firstLaunchPlan(view())
    expect(downloadQueue(plan, initialSelection(plan))).toEqual([
      // Everything else needs it to be *used*: a machine that loses its
      // connection after four weights can run none of them.
      RUNTIME_ID,
      'textDetector',
      'scriptGate',
      'scriptGateLabels',
      'balloonDetector',
      'inpainter',
    ])
  })

  it('queues nothing that is already installed', () => {
    const plan = firstLaunchPlan(view({ installed: [RUNTIME_ID, 'textDetector'] }))
    const queue = downloadQueue(plan, initialSelection(plan))
    expect(queue).not.toContain(RUNTIME_ID)
    expect(queue).not.toContain('textDetector')
    expect(queue[0]).toBe('scriptGate')
  })

  it('names a row so a failure can say which one it was', () => {
    const plan = firstLaunchPlan(view())
    expect(labelKeyFor(plan, 'scriptGate')).toBe('models.kind.scriptGate')
    expect(labelKeyFor(plan, 'inpainter')).toBe('models.kind.inpainter')
    // The runtime is an archive rather than a weight and borrows the name
    // Settings gives it.
    expect(labelKeyFor(plan, RUNTIME_ID)).toBe('settings.models.runtime.label')
    expect(labelKeyFor(plan, 'somethingNewer')).toBe(null)
  })
})
