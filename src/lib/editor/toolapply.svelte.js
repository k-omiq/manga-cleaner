/**
 * The seam between the canvas and the tools: what happens when a region on the
 * page is clicked, or when a drawn shape turns out to be *about* a region that
 * already exists.
 *
 * The canvas owns *which* region was hit and nothing about what a tool does;
 * this module owns the one call that turns a hit into an edit - the adapter
 * call, the cloud gate in front of it, the region swap and the undo record.
 * `drawing.svelte.js` owns the gestures that make regions rather than edit
 * them, and comes back here whenever a gesture lands on an existing one.
 *
 * `maskactions.svelte.js` is the pattern, followed exactly - a pair of region
 * snapshots and one `restoreRegion` call in each direction, so redo cannot
 * drift from undo, and so a mask made here is as findable by the backend after
 * an undo as one made by the automatic pass.
 */

import { getBackend } from '../api/backend.js'
import {
  adoptRun,
  editor,
  pageStatusOf,
  recordRegionEdit,
  replaceRegion,
  select,
} from '../state/editor.svelte.js'
import { applyWithConfirmations, cloudRefused } from './cloudflow.svelte.js'
// The reporter lives beside the actions it was written for, and this module
// already follows that one for everything else it does; a second copy of the
// same three lines is a second sentence a failed edit could start saying.
import { reportRegionEditFailure } from './maskactions.svelte.js'
import { paintParamsOf } from './paint.js'

/**
 * @typedef {Object} RegionState
 * @property {import('../api/backend.js').ApiRegion} region
 * @property {string|null} pageStatus
 */

/**
 * @param {import('../api/backend.js').ApiRegion} region
 * @returns {RegionState}
 */
function snapshot(region) {
  return {
    region: /** @type {any} */ ($state.snapshot(region)),
    pageStatus: pageStatusOf(region.id),
  }
}

/**
 * Apply the active tool to one region - the canvas's whole share of what a
 * click does, and the landing point for a gesture that turned out to be about
 * a region that already exists.
 *
 * @param {string} regionId
 * @param {Record<string, unknown>} [extraParams] - merged over the tool's own,
 *   for a gesture that carries geometry (`bbox`) the parameters cannot
 * @returns {Promise<boolean>} whether anything changed
 */
export async function applyActiveToolToRegion(regionId, extraParams) {
  const chapter = editor.chapter
  if (!chapter) return false

  // Before the call, not after it. The undo record is a pair of region
  // snapshots, so an id the open chapter does not hold cannot be recorded -
  // and asking the adapter first would let it mutate its own state for an edit
  // this side can neither swap in nor reverse. Unreachable while every id
  // comes from a rendered region; the ordering is what keeps it unreachable.
  const region = findRegion(regionId)
  if (!region) return false

  const tool = editor.tool
  const mergedParams = { ...$state.snapshot(editor.toolParams[tool] ?? {}), ...(extraParams ?? {}) }
  const points = extraParams?.points ?? (/** @type {any} */ (extraParams?.stroke)?.points)
  const paint = extraParams?.paint ?? (points ? paintParamsOf(tool, mergedParams, points) : null)
  const params = {
    ...mergedParams,
    ...(paint ? { paint } : {}),
  }

  // Refused in the interface, before anything is sent. The adapter's own block
  // is the second line of defence, not the first.
  if (cloudRefused(tool, params)) return false

  const before = snapshot(region)
  select(regionId)

  // Every caller of this function fires it and walks away - a region click does
  // not await it, and neither does the gesture that lands on an existing
  // region - so a rejection here reached nothing at all: no handler, no notice,
  // and no `unhandledrejection` listener in the application to catch it last.
  // A tool that faulted was a click that did nothing. It is reported through
  // the same reporter the Layers row's own controls use, and the answer is
  // `false`, which is what this function already says for every other way of
  // changing nothing.
  /** @type {any} */
  let result
  try {
    result = await applyWithConfirmations(
      (next) =>
        getBackend().applyTool({
          tool,
          params: next,
          chapterId: chapter.id,
          pageIndex: editor.pageIndex,
          regionId,
        }),
      params,
    )
  } catch (error) {
    return reportRegionEditFailure(error)
  }

  switch (result.status) {
    case 'applied': {
      if (!result.region) return false
      // The page's status comes back with the region, as it does from every
      // other region-level edit: `api/tools.js#applyToolToRegion` moves an
      // unclean page to cleaned, and a region put back without its page's
      // status is how a full track ends up under a "not cleaned" mark. This
      // was a recorded gap in `ApplyResult`; closing it is one optional field.
      replaceRegion(result.region, result.pageStatus)
      recordApply(regionId, before, {
        region: result.region,
        pageStatus: result.pageStatus ?? before.pageStatus,
      })
      return true
    }
    // Auto clean is not a per-region tool: clicking a region with it selected
    // starts the page's run, exactly as the tool window's own button does. The
    // result arrives on the event channel, never from this promise.
    case 'run-started':
      return adoptRun(result, 'page') !== null
    // `'blocked'` can only be reached now by a path the gate above did not
    // recognise, and the adapter has already said so on the notice channel.
    // `'cancelled'` is the user abandoning a confirmation dialog: nothing was
    // sent and nothing changed.
    default:
      return false
  }
}

/**
 * @param {string} regionId
 * @param {RegionState} before
 * @param {RegionState} after
 */
function recordApply(regionId, before, after) {
  recordRegionEdit('canvas.command.applyTool', regionId, before, after)
}

/**
 * @param {string} regionId
 * @returns {import('../api/backend.js').ApiRegion|null}
 */
export function findRegion(regionId) {
  for (const page of editor.chapter?.pages ?? []) {
    const region = page.regions.find((candidate) => candidate.id === regionId)
    if (region) return region
  }
  return null
}
