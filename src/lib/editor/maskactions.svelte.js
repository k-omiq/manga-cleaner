/**
 * What the Layers panel's controls actually do: delete a mask, re-run it,
 * clean a gate-skipped region anyway, show one on the page - and undo any of
 * it.
 *
 * Every one of these goes through the backend seam and then puts the region
 * the adapter returned back into the open chapter. None of them edits a
 * region in place: a mask is the backend's to make and unmake, and the
 * interface holding a divergent copy is how "why does the panel say something
 * different from the file" starts.
 *
 * **Undo.** Each edit is recorded as a pair of `restoreRegion` calls - the
 * region as it was, and the region as it became. That is why the seam grew
 * `restoreRegion`: a mask deleted and then undone has to be findable again by
 * the *backend*, or the restored row's own actions would quietly do nothing.
 * Deleting restores the original text under the mask, and
 * undoing the delete puts the mask back.
 *
 * **A region edit can move its page.** Cleaning a gate-skipped region makes an
 * unclean page a cleaned one, and deleting the last mask on a page takes that
 * back; the Pages row's mark comes from the page and its track from the
 * regions, so the two disagree the moment one is applied without the other.
 * Every call here therefore carries the page's status as well - forward from
 * the adapter, and backward in each half of the snapshot pair, so undo is
 * symmetric with the edit it reverses.
 */

import { getBackend } from '../api/backend.js'
import {
  applyRegionDelta,
  applyRegionState,
  editor,
  recordRegionEdit,
  replaceRegion,
  pageStatusOf,
  select,
  setTool,
} from '../state/editor.svelte.js'

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
 * Records one region-level edit as an undoable command. Both directions are
 * the same call with a different snapshot, so redo cannot drift from undo.
 *
 * @param {string} label - i18n key for the undo/redo tooltip
 * @param {string} regionId
 * @param {RegionState} before
 * @param {RegionState} after
 */
function recordEdit(label, regionId, before, after) {
  recordRegionEdit(label, regionId, before, after)
}

/**
 * Delete a region's mask. The original text under it comes back, **and the row
 * goes with it**.
 *
 * Not "the region with its mask taken off". A region with no mask is a row the
 * panel lists as *unexamined* and a box the canvas draws, so leaving one behind
 * answered a delete with an empty placeholder in the same place - the thing the
 * user was asking to be rid of. The backend keeps the record invisible on disk
 * so the delete can be undone; nothing lists it, and neither does this.
 *
 * Which makes the undo pair the removal pair every other disappearance uses:
 * the region as it was against nothing at all, so undo restores it through
 * `restoreRegion` and redo takes it away again.
 *
 * @param {import('../api/backend.js').ApiRegion} region
 * @returns {Promise<boolean>} whether anything was deleted
 */
export async function deleteMask(region) {
  if (!region.mask) return false
  const before = snapshot(region)
  const result = await getBackend().deleteMask({ maskId: region.mask.id })
  if (!result) return false
  const after = { region: null, pageStatus: result.pageStatus ?? before.pageStatus }
  // The selection and the hover go with it - `applyRegionState` drops both,
  // for every route into a removal rather than for this one. A selection
  // pointing at a region that is no longer on the page highlights nothing and
  // steps to nothing; the row it belonged to has gone.
  applyRegionState(region.id, null, after.pageStatus ?? undefined)
  recordRegionEdit('masks.command.deleteMask', region.id, before, after)
  return true
}

/**
 * Remove a region that has no mask - a warning the user has read and decided
 * to leave alone.
 *
 * Not the same call as `deleteMask`, because there is no mask to delete - only
 * the region, and the seam already has the method for that. Both ends up in the
 * same place: a row the user deleted is a row that is gone.
 * `restoreRegion` with a null region is how a hand-drawn region's *creation*
 * is undone (`drawing.svelte.js`), and removing a region is the same act
 * whichever end of its life it happens at - so it is the same call, and the
 * undo pair is the ordinary one.
 *
 * @param {import('../api/backend.js').ApiRegion} region
 * @returns {Promise<boolean>}
 */
export async function deleteRegion(region) {
  if (region.mask) return deleteMask(region)
  const before = snapshot(region)
  const gone = { region: null, pageStatus: before.pageStatus }
  await restoreRegionThroughSeam(region.id, gone)
  recordRegionEdit('masks.command.deleteRegion', region.id, before, gone)
  return true
}

/**
 * Delete whatever a row stands for: its mask if it has one, the region itself
 * if it does not. The one call every delete control in the panel makes, so a
 * warning row and a mask row cannot end up with two different meanings of the
 * same trash icon.
 *
 * @param {import('../api/backend.js').ApiRegion} region
 * @returns {Promise<boolean>}
 */
export async function deleteRow(region) {
  return region.mask ? deleteMask(region) : deleteRegion(region)
}

/**
 * One direction of a removal: put the region - and its page's status - into
 * the state the snapshot describes, on the backend and then in the open
 * chapter. `{region: null}` means "it was not there".
 *
 * @param {string} regionId
 * @param {{region: import('../api/backend.js').ApiRegion|null, pageStatus: string|null}} state
 * @returns {Promise<void>}
 */
async function restoreRegionThroughSeam(regionId, state) {
  // One applier, in `state/editor.svelte.js`, shared with every replay the
  // journal drives: a failed restore must not delete what it was asked to bring
  // back, and that reading should exist once rather than at each call site.
  await applyRegionDelta(regionId, {
    present: !!state.region,
    pageStatus: state.pageStatus ?? null,
    region: state.region ?? null,
  })
}

/**
 * Re-run a mask: at a named engine, at the one it already used, one rung
 * stronger or simpler, or with the next fill mode.
 *
 * `reopenInTool` is the one kind that is *not* an edit - it hands the region
 * back to the tool that made it with the mask intact, so there is nothing to
 * undo and nothing to record.
 *
 * @param {import('../api/backend.js').ApiRegion} region
 * @param {'stronger'|'simpler'|'cycleFill'|'reopenInTool'|'retry'|'engine'} kind
 * @param {string} [engine] - the rung `kind: 'engine'` should run at
 * @returns {Promise<boolean>}
 */
export async function rerunMask(region, kind, engine) {
  if (!region.mask) return false
  const before = snapshot(region)
  const result = await getBackend().rerunMask({ maskId: region.mask.id, kind, engine })
  if (!result) return false
  replaceRegion(result.region, result.pageStatus)

  if (kind === 'reopenInTool') {
    select(region.id)
    const reopenTool =
      result.reopenTool ??
      region.tool ??
      region.mask?.provenance?.params_snapshot?.tool ??
      'contentAwareFill'
    setTool(reopenTool)
    return true
  }
  recordEdit('masks.command.rerunMask', region.id, before, {
    region: result.region,
    pageStatus: result.pageStatus,
  })
  return true
}

/**
 * Clean a region the script gate skipped, on the user's say-so.
 *
 * **The engine is the user's own pick for this kind of text.** The automatic
 * pass never cleans an out-of-balloon region - it refuses to
 * burn SFX and artwork on a guess, and that ruling stands - so the tool
 * window's *Text outside bubbles* row had no automatic effect at all.
 * This is where it bites: the row says where a
 * region of that kind *starts* when a person asks for it to be cleaned, and the
 * bubble row answers the gate's other two causes, which are regions inside a
 * balloon held back for a different reason.
 *
 * A starting rung and never a ceiling, exactly as it is for a run: the ladder
 * still escalates when the quality metric declines a patch.
 *
 * @param {import('../api/backend.js').ApiRegion} region
 * @returns {Promise<boolean>}
 */
export async function cleanAnyway(region) {
  const before = snapshot(region)
  const params = /** @type {any} */ (editor.toolParams.autoClean ?? {})
  const engine =
    region.gateSkipCause === 'outside-bubble'
      ? (params.outsideEngine ?? 'lama')
      : (params.bubbleEngine ?? 'fill')
  const result = await getBackend().cleanAnyway({ regionId: region.id, engine })
  if (!result) return false
  replaceRegion(result.region, result.pageStatus)
  recordEdit('masks.command.cleanAnyway', region.id, before, {
    region: result.region,
    pageStatus: result.pageStatus,
  })
  return true
}

/**
 * Point at a region on the canvas. A declined region has no mask and nothing
 * visibly happened to it, so this is the only way to find it.
 *
 * Selection only. `hoverId` is transient and is cleared by whoever set it (the
 * highlight contract in `state/editor.svelte.js`); this is a click, so there
 * is no matching "and now the pointer left" to clear it again, and a hover
 * left switched on would outlive the row it came from. Selection is sticky by
 * design and lights the canvas through `isHighlighted` on its own.
 *
 * @param {import('../api/backend.js').ApiRegion} region
 */
export function showOnPage(region) {
  select(region.id)
}

/**
 * Run one of the entries the region context menu offers, by the id
 * `model/masks.js#regionMenuSections` gave it.
 *
 * Nothing new happens here: the menu is a second route to the row's own three
 * controls, and it takes exactly the same two calls they do. A menu that grew
 * its own idea of what Delete means is the defect `deleteRow` exists to
 * prevent.
 *
 * @param {string} id
 * @param {import('../api/backend.js').ApiRegion} region
 * @returns {Promise<boolean>}
 */
export async function runRegionMenuItem(id, region) {
  if (!region) return false
  if (id === 'retry') return rerunMask(region, 'retry')
  if (id === 'delete') return deleteRow(region)
  if (id.startsWith('engine:')) return rerunMask(region, 'engine', id.slice('engine:'.length))
  return false
}

/**
 * Run one of the actions a row offers, by the id `model/masks.js` gave it.
 *
 * @param {string} id
 * @param {import('../api/backend.js').ApiRegion} region
 * @returns {Promise<boolean>}
 */
export async function runMaskAction(id, region) {
  switch (id) {
    case 'stronger':
    case 'simpler':
    case 'cycleFill':
    case 'reopenInTool':
      return rerunMask(region, id)
    case 'cleanAnyway':
      return cleanAnyway(region)
    case 'showOnPage':
      showOnPage(region)
      return true
    default:
      return false
  }
}
