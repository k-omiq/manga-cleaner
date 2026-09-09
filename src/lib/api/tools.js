/**
 * What each tool, re-run and cloud request does to a region.
 *
 * Every function here mutates the region in place and returns the notices the
 * adapter should put on the event channel. Notices are i18n keys plus
 * parameters - never English text.
 *
 * A mask made by hand and a mask made by the automatic pass are identical in
 * kind, so all of these produce the same `Mask` shape.
 */

import { RUNGS, simpler, stronger } from '../model/ladder.js'
import { fillModeLabel, nextFillMode, ROW_ENGINES } from '../model/masks.js'
import {
  capRung,
  CLOUD_REJECTION_CAUSES,
  commitMask,
  ENGINE_INFO,
} from './provenance.js'
import { hashString } from './rng.js'

/** @typedef {{ key: string, params: Object, tone: 'info'|'warn' }} NoticeSpec */

/**
 * A `CommitContext` (provenance.js) whose timestamps come from the mock's
 * injected clock, plus the settings that gate the cloud rung. Sharing the
 * shape with `pagebuilder.js`'s `BuildContext` is what lets both commit a
 * mask through one function.
 *
 * @typedef {import('./provenance.js').CommitContext & { settings: Object }} ToolContext
 */

/** Tool id to the engine it commits with, and the tool's i18n label key. */
export const TOOLS = Object.freeze({
  autoClean: { labelKey: 'tools.name.autoClean', engine: null, fillMode: null },
  brush: { labelKey: 'tools.name.brush', engine: 'fill', fillMode: 'match-surround' },
  shapes: { labelKey: 'tools.name.shapes', engine: 'fill', fillMode: 'match-surround' },
  aiMaskBrush: { labelKey: 'tools.name.aiMaskBrush', engine: 'fill', fillMode: 'match-surround' },
  contentAwareFill: {
    labelKey: 'tools.name.contentAwareFill',
    engine: 'fill',
    fillMode: 'match-surround',
  },
  cloneHeal: { labelKey: 'tools.name.cloneHeal', engine: 'clone', fillMode: 'match-surround' },
})

/**
 * Whether a cloud request for this region comes back accepted or rejected,
 * and by which of the five causes. Deterministic in
 * the region id, so a region's cloud behaviour is stable across launches and
 * a re-run is a re-run rather than a dice roll.
 *
 * @param {string} regionId
 * @returns {import('../model/types.js').CloudOutcome}
 */
export function cloudOutcomeFor(regionId) {
  const hash = hashString(`cloud:${regionId}`)
  if (hash % 4 !== 3) return { accepted: true, rejectionCause: null }
  return { accepted: false, rejectionCause: CLOUD_REJECTION_CAUSES[(hash >>> 3) % 5] }
}

/**
 * Whether an automatic run may pick this region up. A region the detector
 * never found stays pending for ever - the auto pass would miss it again for
 * the same reason, and a hand tool is what reaches it. Without this, a later run over
 * the chapter would silently clean text the app never detected.
 *
 * @param {import('../model/types.js').Region} region
 * @returns {boolean}
 */
export function isQueueable(region) {
  return region.outcome === 'pending' && region.detected !== false
}

/**
 * A region the gate held back as "text outside a speech bubble". A run with
 * `outsideBubbles: 'clean'` queues these beside the pending ones - the
 * native run never records them as held back in the first place, and this is
 * the mock's equivalent for a fixture that already did (the outside-bubble opt-in).
 *
 * @param {import('../model/types.js').Region} region
 */
export function isOutsideHeld(region) {
  return (
    region.outcome === 'gate-skipped' &&
    region.gateSkipCause === 'outside-bubble' &&
    region.detected !== false
  )
}

/**
 * The automatic pass's result for one region: route it up the ladder, and
 * record a reconstruction (which is a review cause) whenever it landed on an
 * inpainting rung, because that is exactly what "fitting failed and a model
 * reconstructed the area" means.
 *
 * @param {import('../model/types.js').Region} region
 * @param {import('../model/types.js').Page} page
 * @param {ToolContext} ctx
 * @param {{ engineCeiling?: string, bubbleEngine?: string, outsideEngine?: string }} options
 * @returns {import('../model/types.js').Mask|null} null when the region was left alone
 */
export function cleanRegionAutomatically(region, page, ctx, options = {}) {
  // `engineCeiling` is a ceiling in both directions: a caller may lower the
  // rung a run reaches, never raise it above the setting. Taking the caller's
  // value outright let `startRun`'s local-only pin (the highest local rung)
  // reach *past* a user whose setting was lower - the same shape of defect as
  // reaching past a user who blocked the cloud.
  const setting = ctx.settings.engineCeiling ?? 'lama'
  const requested = options.engineCeiling ? capRung(options.engineCeiling, setting) : setting
  // Cloud is opt-in. When Settings block it the
  // ceiling is capped again at the highest local rung.
  const ceiling = ctx.settings.cloudEngines === 'allowed' ? requested : capRung(requested, 'lama')
  // The two rows now name rungs outright rather than the two families they
  // used to, so a pick *is* a starting rung -
  // capped by the ceiling, and still only a starting point. The two retired
  // words are still read, because a stored preference from before the change
  // is not a typo.
  const pick =
    region.kind === 'bubble'
      ? (options.bubbleEngine ?? 'fill')
      : (options.outsideEngine ?? 'lama')
  const started = pick === 'redraw' ? 'lama' : RUNGS.includes(pick) ? pick : 'fill'
  const engine = capRung(started, ceiling)
  if (engine === 'cloud') {
    const outcome = cloudOutcomeFor(region.id)
    return outcome.accepted
      ? commitMask(region, ctx, {
          engine: 'cloud',
          cloudBilled: true,
          cloudOutcome: outcome,
          fillMode: 'reconstruct',
        })
      : // The fallback to rung 2 does not set `fittingReconstructed`: that is
        // checked first in `review.js` and would hide the rejection cause,
        // which is the more actionable thing to show.
        commitMask(region, ctx, {
          engine: 'lama',
          cloudOutcome: outcome,
          fillMode: 'reconstruct',
        })
  }
  return commitMask(region, ctx, {
    engine,
    fittingReconstructed: engine === 'lama',
    fillMode: ENGINE_INFO[engine].fillMode,
  })
}

/**
 * Applies a hand tool to a region.
 *
 * @param {import('../model/types.js').Region} region
 * @param {import('../model/types.js').Page} page
 * @param {ToolContext} ctx
 * @param {{ tool: string, params?: Object }} options
 * @returns {{ mask: import('../model/types.js').Mask, notices: NoticeSpec[] }}
 */
export function applyToolToRegion(region, page, ctx, options) {
  const notices = []
  const params = options.params ?? {}
  const tool = TOOLS[options.tool] ?? TOOLS.brush

  // A hand-drawn mask is **authoritative**: the
  // fitting search does not run on it, so the geometry the gesture describes
  // replaces the detector's box outright rather than being reconciled with it.
  // The AI mask brush is no exception any more - its stroke *is* the mask, no
  // detector is asked what is under it, and so there is no `aiSnapped` /
  // `aiFallback` mechanism left to report.
  if (params.bbox) region.bbox = { ...params.bbox }

  const fillMode = params.fillMode ?? tool.fillMode ?? 'match-surround'
  const reconstructing = fillMode === 'reconstruct'
  region.source = 'hand'
  region.tool = options.tool

  // A Shapes gesture set to `solid` is paint as surely as the Brush's paint
  // mode is: it lays down a colour somebody chose rather than asking an engine
  // what belongs there, so it takes the same branch here and the same branch
  // in `region.rs#paint_plan`, and it is refused on the same colour modes.
  const solidShape = options.tool === 'shapes' && params.mode === 'solid'
  const isPaint =
    (options.tool === 'brush' && params.mode === 'paint') ||
    options.tool === 'cloneHeal' ||
    solidShape
  if (isPaint && page.colorMode && !['Gray8', 'GrayAlpha8', 'RGB8', 'RGBA8'].includes(page.colorMode)) {
    region.outcome = 'declined'
    region.declineReason = 'decline.reason.paintUnsupportedMode'
    region.mask = null
    return {
      mask: null,
      notices: [
        {
          key: 'notice.mask.rerunFailed',
          params: { reasonKey: 'decline.reason.paintUnsupportedMode' },
          tone: 'warn',
        },
      ],
    }
  }

  let engine = reconstructing ? 'lama' : (tool.engine ?? 'fill')
  if ((options.tool === 'brush' && params.mode === 'paint') || solidShape) {
    engine = 'paint'
  } else if (options.tool === 'cloneHeal') {
    engine = 'clone'
  } else if (
    (options.tool === 'aiMaskBrush' || options.tool === 'shapes') &&
    ROW_ENGINES.includes(params.engine)
  ) {
    // The AI mask brush and Shapes both name a rung outright, exactly as a
    // Layers row's picker does - `region.rs#named_rung` reads the same word
    // and runs that engine rather than starting there. The mock ignored it, so
    // a stroke the user pointed at FLUX committed a fill and said so in the
    // provenance; a Shape pointed at LaMa did the same until Shapes grew the
    // row that names one.
    engine = params.engine
  }
  const mask = commitMask(region, ctx, {
    tool: options.tool,
    source: 'hand',
    engine,
    fillMode,
    fittingReconstructed: reconstructing,
  })
  // **Deliberate, and load-bearing since `createHandRegion`.** A hand tool is
  // the app being *told* where text is, so from here on the app holds a box for
  // it and the region is on the detector's side of the fork. That means a
  // region the auto pass missed becomes queueable again if its mask is later
  // deleted (`isQueueable`), which is right: the exclusion is for text nothing
  // has ever found. Pinned by a test in `mock.test.js`.
  region.detected = true
  region.tool = options.tool
  if (page.status === 'unclean') page.status = 'cleaned'
  return { mask, notices }
}

/**
 * A region drawn by hand, where the detector found nothing.
 *
 * Brush, Shapes and the AI mask brush all **create** masks - that is their
 * whole purpose - and `applyTool`
 * cannot express that, because it needs a region id that does not exist yet.
 * The region this builds is **identical in kind** to one the detector found:
 * same shape, same row in the Layers panel, same actions,
 * same export treatment. `source: 'hand'` is the only difference, and
 * `applyToolToRegion` is what sets it, so a hand region and a hand *edit* of an
 * automatic region go through exactly one code path.
 *
 * `detected: false` on the way in says what is true of a hand region at the
 * moment it is made: nothing found this text, a person did. `applyToolToRegion`
 * flips it, for the reason written there.
 *
 * @param {import('../model/types.js').Page} page
 * @param {ToolContext} ctx
 * @param {{ id: string, bbox: Object, tool: string, params?: Object }} options
 * @returns {{ region: import('../model/types.js').Region, mask: import('../model/types.js').Mask, notices: NoticeSpec[] }}
 */
export function createHandRegion(page, ctx, options) {
  /** @type {any} */
  const region = {
    id: options.id,
    pageId: page.id,
    sourceSha: page.sourceSha,
    bbox: { ...options.bbox },
    // Stand-in image data, as on every other region: a hand-drawn mask covers
    // whatever is under it, so there is no bubble and no sample text to draw.
    kind: 'outside',
    text: '',
    detected: false,
    source: 'hand',
    outcome: 'pending',
    gateSkipCause: null,
    declineReason: null,
    unusuallyLarge: false,
    mask: null,
  }
  page.regions.push(region)
  const applied = applyToolToRegion(region, page, ctx, {
    tool: options.tool,
    params: options.params,
  })
  return { region, mask: applied.mask, notices: applied.notices }
}

/**
 * A cloud request for one region, after the user has confirmed the spend.
 * Rejected requests fall back to rung 2 and are not billed.
 *
 * @param {import('../model/types.js').Region} region
 * @param {import('../model/types.js').Page} page
 * @param {ToolContext} ctx
 * @returns {{ mask: import('../model/types.js').Mask, outcome: import('../model/types.js').CloudOutcome, notices: NoticeSpec[] }}
 */
export function applyCloudToRegion(region, page, ctx) {
  const outcome = cloudOutcomeFor(region.id)
  const notices = []
  let mask
  if (outcome.accepted) {
    mask = commitMask(region, ctx, {
      engine: 'cloud',
      fillMode: 'reconstruct',
      cloudBilled: true,
      cloudOutcome: outcome,
    })
    notices.push({
      key: 'notice.cloud.returned',
      params: { seconds: Math.round(mask.elapsedMs / 100) / 10, cost: mask.provenance.cloud.cost },
      tone: 'info',
    })
  } else {
    mask = commitMask(region, ctx, {
      engine: 'lama',
      fillMode: 'reconstruct',
      cloudOutcome: outcome,
    })
    notices.push({
      key: 'notice.cloud.rejected',
      params: { causeKey: `review.reason.cloudRejected${causeSuffix(outcome.rejectionCause)}` },
      tone: 'warn',
    })
  }
  region.source = 'hand'
  region.tool = 'contentAwareFill'
  if (page.status === 'unclean') page.status = 'cleaned'
  return { mask, outcome, notices }
}

/**
 * @param {string} cause
 * @returns {string} the cause in PascalCase, matching `review.js`'s key names
 */
function causeSuffix(cause) {
  return cause
    .split('-')
    .map((part) => part[0].toUpperCase() + part.slice(1))
    .join('')
}

/**
 * Deletes a region's mask, restoring the original text under it.
 *
 * The mask only. Taking the region off the page is the caller's half - see
 * `mock.js#deleteMask`, which splices the row out once this has emptied it,
 * because a maskless region is a row the Layers panel would list as unexamined
 * and the canvas would draw a box for: an empty placeholder exactly where the
 * user asked for one to stop being.
 *
 * @param {import('../model/types.js').Region} region
 * @returns {{ notices: NoticeSpec[] }}
 */
export function deleteRegionMask(region) {
  region.mask = null
  region.outcome = 'pending'
  return { notices: [{ key: 'notice.mask.deleted', params: {}, tone: 'info' }] }
}

/**
 * Re-runs a region's mask: at a named rung, at the one it already used, one
 * rung stronger or simpler, with the next fill mode, or handed back to the tool
 * that made it with the mask intact.
 *
 * `'retry'` and `'engine'` are what the Layers panel's row controls send -
 * "run this again" and "run this with that engine". They join the ladder pair
 * at the tail below rather than getting a branch of their own, because the only
 * thing that separates the four is how the target rung is chosen; everything
 * after that - the cloud gate, the rejection fallback, the commit - has to be
 * identical or a row control would produce a mask an escalation could not.
 *
 * @param {import('../model/types.js').Region} region
 * @param {ToolContext} ctx
 * @param {{ kind: 'stronger'|'simpler'|'cycleFill'|'reopenInTool'|'retry'|'engine', engine?: string }} options
 * @returns {{ mask: import('../model/types.js').Mask, reopenTool: string|null, notices: NoticeSpec[] }}
 */
export function rerunRegionMask(region, ctx, options) {
  const current = region.mask
  const engine = current.provenance.engine

  if (options.kind === 'reopenInTool') {
    // The tool that made it, with the mask left intact.
    // Masks from the automatic pass reopen in Content-aware fill, which is
    // the tool that can act on an existing mask.
    const toolCandidate = region.tool ?? current.provenance?.params_snapshot?.tool
    const reopenTool = TOOLS[toolCandidate] ? toolCandidate : 'contentAwareFill'
    return {
      mask: current,
      reopenTool,
      notices: [
        {
          key: 'notice.mask.reopened',
          params: { toolKey: TOOLS[reopenTool].labelKey },
          tone: 'info',
        },
      ],
    }
  }

  if (options.kind === 'cycleFill') {
    const next = nextFillMode(current.fillMode)
    const mask = commitMask(region, ctx, {
      engine,
      fillMode: next,
      fittingReconstructed: current.fittingReconstructed,
      cloudOutcome: current.cloudOutcome,
      cloudBilled: !!current.provenance.cloud,
    })
    return {
      mask,
      reopenTool: null,
      notices: [
        { key: 'notice.mask.fillMode', params: { fillModeKey: fillModeLabel(next) }, tone: 'info' },
      ],
    }
  }

  // An unknown rung on `'engine'` falls back to the one the mask already used
  // rather than to a guess: the request was "run it with that", and if the app
  // does not have "that", running it again unchanged is the honest answer.
  const target = TARGET_RUNG[options.kind](engine, options.engine)
  if (target === 'cloud' && ctx.settings.cloudEngines !== 'allowed') {
    return {
      mask: current,
      reopenTool: null,
      notices: [{ key: 'notice.cloud.blocked', params: {}, tone: 'warn' }],
    }
  }
  const cloudOutcome = target === 'cloud' ? cloudOutcomeFor(region.id) : null
  const mask = commitMask(region, ctx, {
    engine: target === 'cloud' && !cloudOutcome.accepted ? 'lama' : target,
    fillMode: ENGINE_INFO[target].fillMode,
    fittingReconstructed: target === 'lama',
    cloudOutcome,
    cloudBilled: target === 'cloud' && cloudOutcome.accepted,
  })
  return {
    mask,
    reopenTool: null,
    notices: [
      {
        key: RERUN_NOTICE[options.kind],
        params: { rungKey: `ladder.rung.${target}` },
        tone: 'info',
      },
    ],
  }
}

/** How each re-run kind picks the rung to run at. */
const TARGET_RUNG = Object.freeze({
  stronger: (/** @type {string} */ engine) => stronger(engine),
  simpler: (/** @type {string} */ engine) => simpler(engine),
  retry: (/** @type {string} */ engine) => engine,
  // The ladder's own rungs, and the ones a Layers row may name that are not on
  // it: `flux` is rung 3a, which the ladder skips because nothing *steps* to
  // it - it is reached by being asked for, and this is where it is asked for.
  engine: (/** @type {string} */ current, /** @type {string|undefined} */ wanted) =>
    RUNGS.includes(wanted ?? '') || ROW_ENGINES.includes(wanted ?? '')
      ? /** @type {string} */ (wanted)
      : current,
})

/** What each of them says when it lands. */
const RERUN_NOTICE = Object.freeze({
  stronger: 'notice.mask.rerunStronger',
  simpler: 'notice.mask.rerunSimpler',
  retry: 'notice.mask.rerunAgain',
  engine: 'notice.mask.rerunEngine',
})

/**
 * Cleans a region the script gate skipped, on the user's say-so.
 *
 * @param {import('../model/types.js').Region} region
 * @param {import('../model/types.js').Page} page
 * @param {ToolContext} ctx
 * @param {string} [engine]
 * @returns {{ mask: import('../model/types.js').Mask, notices: NoticeSpec[] }}
 */
export function cleanRegionAnyway(region, page, ctx, engine = 'fill') {
  const mask = commitMask(region, ctx, { engine, fillMode: 'match-surround' })
  if (page.status === 'unclean') page.status = 'cleaned'
  return { mask, notices: [{ key: 'notice.gate.cleanedAnyway', params: {}, tone: 'info' }] }
}
