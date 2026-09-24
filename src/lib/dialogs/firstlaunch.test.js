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
  DEFAULT_FLUX_MODEL,
  FIRST_LAUNCH_STEPS,
  RUNTIME_ID,
  defaultFluxModel,
  downloadQueue,
  firstLaunchPlan,
  groupBytes,
  groupState,
  initialSelection,
  labelKeyFor,
  planGroups,
  plannedBytes,
  runProgress,
  runtimeNotice,
  runtimeReady,
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
  it('skips Japanese OCR unless the reader is selected', () => {
    const catalogue = view()
    catalogue.models.push(
      { id: 'ocrEncoder', kindKey: 'models.kind.ocr', bytes: 343, requiredBy: [], installed: false },
      { id: 'ocrDecoder', kindKey: 'models.kind.ocrDecoder', bytes: 117, requiredBy: [], installed: false },
      { id: 'ocrVocab', kindKey: 'models.kind.ocrVocab', bytes: 1, requiredBy: [], installed: false },
    )
    const plan = firstLaunchPlan(catalogue)
    const selection = initialSelection(plan)
    expect(downloadQueue(plan, selection)).not.toContain('ocrEncoder')
    const selected = { ...selection, ocrEncoder: true, ocrDecoder: true, ocrVocab: true }
    expect(downloadQueue(plan, selected).slice(-3)).toEqual(['ocrEncoder', 'ocrDecoder', 'ocrVocab'])
    expect(plannedBytes(plan, selected) - plannedBytes(plan, selection)).toBe(461)
  })

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

/** The catalogue with the Japanese reader's three files in it as well. */
function withReader(options) {
  const catalogue = view(options)
  catalogue.models.push(
    { id: 'ocrEncoder', kindKey: 'models.kind.ocr', bytes: 343, requiredBy: [], installed: false },
    { id: 'ocrDecoder', kindKey: 'models.kind.ocrDecoder', bytes: 117, requiredBy: [], installed: false },
    { id: 'ocrVocab', kindKey: 'models.kind.ocrVocab', bytes: 1, requiredBy: [], installed: false },
  )
  return catalogue
}

/** A run that has not started. @param {Object} [overrides] */
function run(overrides = {}) {
  return { selection: {}, finished: {}, failure: null, current: null, running: false, paused: false, ...overrides }
}

describe('the setup around the offer', () => {
  it('walks six steps, one choice each, in this order', () => {
    expect(FIRST_LAUNCH_STEPS).toEqual(['welcome', 'models', 'defaults', 'cloud', 'behavior', 'done'])
    // No token step: every default download is from a public repository.
    expect(FIRST_LAUNCH_STEPS).not.toContain('token')
  })

  it('draws the download step as three choices, leaving out an empty one', () => {
    const plan = firstLaunchPlan(withReader())
    const groups = planGroups(plan)
    expect(groups.map((group) => [group.id, group.optional])).toEqual([
      ['required', false],
      ['redraw', true],
      ['japanese', true],
    ])
    expect(groups[2].rows.map((row) => row.id)).toEqual(['ocrEncoder', 'ocrDecoder', 'ocrVocab'])
    // The redraw engine is here already, and the reader is not in this view.
    expect(planGroups(firstLaunchPlan(view({ installed: ['inpainter'] }))).map((group) => group.id)).toEqual([
      'required',
    ])
    expect(planGroups(null)).toEqual([])
  })

  it('prices a group by what is still to come', () => {
    const plan = firstLaunchPlan(view({ installed: ['balloonDetector'] }))
    const [required] = planGroups(plan)
    expect(groupBytes(required, {})).toBe(AUTO_CLEAN_BYTES - BALLOON_BYTES + RUNTIME_BYTES)
    expect(groupBytes(required, { [RUNTIME_ID]: true })).toBe(AUTO_CLEAN_BYTES - BALLOON_BYTES)
  })

  it('names each group\'s part in a run, the finished and the failed first', () => {
    const plan = firstLaunchPlan(withReader())
    const [required, redraw, japanese] = planGroups(plan)
    const selection = initialSelection(plan)

    expect(groupState(required, run({ selection }))).toBe(null)
    expect(groupState(required, run({ selection, running: true, current: RUNTIME_ID }))).toBe('downloading')
    expect(groupState(redraw, run({ selection, running: true, current: RUNTIME_ID }))).toBe('waiting')
    // Not ticked, so not part of the run: the size stays.
    expect(groupState(japanese, run({ selection, running: true, current: RUNTIME_ID }))).toBe(null)
    expect(groupState(redraw, run({ selection, paused: true }))).toBe('paused')
    expect(groupState(required, run({ selection, failure: { id: 'scriptGate' } }))).toBe('failed')

    const everything = Object.fromEntries(required.rows.map((row) => [row.id, true]))
    // Complete says so even while another group's download failed.
    expect(groupState(required, run({ selection, finished: everything, failure: { id: 'inpainter' } }))).toBe(
      'installed',
    )
  })

  it('measures a run against the whole selection, so a resume never moves the bar back', () => {
    const plan = firstLaunchPlan(view())
    const selection = initialSelection(plan)
    const total = AUTO_CLEAN_BYTES + RUNTIME_BYTES + LAMA_BYTES
    expect(runProgress(plan, selection, {}, {})).toEqual({ done: 0, total })
    expect(
      runProgress(plan, selection, { [RUNTIME_ID]: true }, { textDetector: { downloaded: 1_000, total: null } }),
    ).toEqual({ done: RUNTIME_BYTES + 1_000, total })
    // A runtime package reports its archive, which can be larger than the row.
    expect(runProgress(plan, selection, {}, { [RUNTIME_ID]: { downloaded: RUNTIME_BYTES * 2, total: null } }).done).toBe(
      RUNTIME_BYTES,
    )
    expect(runProgress(null, selection, {}, {})).toEqual({ done: 0, total: 0 })
  })

  it('asks the runtime what it can run on only once it is here and not being replaced', () => {
    const plan = firstLaunchPlan(view())
    expect(runtimeReady(plan, {}, null)).toBe(false)
    expect(runtimeReady(plan, { [RUNTIME_ID]: true }, 'textDetector')).toBe(true)
    expect(runtimeReady(firstLaunchPlan(view({ installed: [RUNTIME_ID] })), {}, null)).toBe(true)
    expect(runtimeReady(firstLaunchPlan(view({ installed: [RUNTIME_ID] })), {}, RUNTIME_ID)).toBe(false)
    expect(runtimeReady(firstLaunchPlan(view({ runtime: { available: false } })), {}, null)).toBe(false)
    // An adapter older than the runtime row is answered the way capabilities answers it.
    expect(runtimeReady(firstLaunchPlan(view({ runtime: null })), {}, null)).toBe(true)
    expect(runtimeReady(null, {}, null)).toBe(false)
  })

  it('reads the runtime\'s refusals and nothing that merely looks like a key', () => {
    expect(runtimeNotice('notice.runtime.noSpace needed=253000000 free=1200000 junk =3 bad=x')).toEqual({
      key: 'notice.runtime.noSpace',
      params: { needed: 253_000_000, free: 1_200_000 },
    })
    expect(runtimeNotice('notice.runtime.inUse')).toEqual({ key: 'notice.runtime.inUse', params: {} })
    expect(runtimeNotice('settings.models.status.failed')).toBe(null)
    expect(runtimeNotice('connection reset')).toBe(null)
    expect(runtimeNotice(undefined)).toBe(null)
  })

  it('lands the AI redraw model on Settings\' first choice', () => {
    expect(defaultFluxModel([{ id: 'other' }, { id: DEFAULT_FLUX_MODEL }])).toBe(DEFAULT_FLUX_MODEL)
    expect(defaultFluxModel([{ id: 'other' }])).toBe('other')
    expect(defaultFluxModel([])).toBe(null)
    expect(defaultFluxModel(undefined)).toBe(null)
  })
})
