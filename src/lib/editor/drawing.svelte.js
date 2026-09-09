/**
 * What the five hand tools actually do once a gesture finishes.
 *
 * The geometry arrives already in the region's normalised coordinate space
 * (`gesture.js`), so everything here is about *which* seam call a gesture is:
 *
 * - **Brush, Shapes, Clone / heal** draw a mask where there was none, so they
 *   create a region - `createRegion`, the one method the seam grew for this
 *   task. The Brush paints a colour into it; the other two hand the area to an
 *   engine.
 * - **The AI mask brush** creates a region too, and for the same reason: the
 *   stroke *is* the mask. It used to ask a snapping helper which existing
 *   region the stroke was "about" and edit that one instead, which is how a
 *   stroke aimed at a leftover beside a layer box re-ran the layer box.
 *   A stroke never retargets now.
 * - **Content-aware fill** never reaches here. It fills an existing mask,
 *   which is a click on a region and therefore
 *   `toolapply.svelte.js`'s.
 *
 * **Undo.** Every command is a pair of region snapshots and one
 * `recordRegionEdit` in each direction - `maskactions.svelte.js`'s
 * pattern. A creation's "before" snapshot is `null`, because before the gesture
 * the region was not there; `restoreRegion` takes that and removes it, and the
 * "after" snapshot puts the same region back with the same mask, so redo cannot
 * drift from undo.
 */

import { getBackend } from '../api/backend.js'
import { notify } from '../state/app.svelte.js'
import {
  applyRegionState,
  editor,
  recordRegionEdit,
  select,
} from '../state/editor.svelte.js'
import { cloudRefused } from './cloudflow.svelte.js'
import { draft, clearDraft, setCloneOffset } from './draft.svelte.js'
import { AI_STROKE_PX, cloneOffset, paintedStroke } from './gesture.js'
import { paintParamsOf } from './paint.js'
import { SOLID } from './tools.js'

export { paintParamsOf }

/**
 * Commit whatever the draft describes. Clears the draft either way: a gesture
 * that finished is over, whether or not it changed anything.
 *
 * @returns {Promise<boolean>} whether the chapter changed
 */
export async function commitDraft() {
  const active = draft.active
  if (!active?.bbox) {
    clearDraft()
    return false
  }
  const spec = {
    tool: active.tool,
    kind: active.kind,
    pageId: active.pageId,
    bbox: active.bbox,
    mode: active.mode,
    // The path itself, copied out of the reactive draft before it is dropped.
    // It is what makes a stroke a stroke on the far side of the seam rather
    // than the rectangle around it.
    points: active.points.map((point) => ({
      x: point.x,
      y: point.y,
      p: typeof point.p === 'number' && point.p > 0 ? point.p : 0.5,
    })),
    start: active.points[0] ?? { x: active.bbox.x, y: active.bbox.y },
  }
  clearDraft()

  switch (spec.tool) {
    case 'brush':
      return createMask(spec)
    case 'cloneHeal':
      return cloneInto(spec)
    case 'shapes':
      return createMask(spec, shapeEngine())
    default:
      return createMask(spec)
  }
}

/**
 * What a Shapes commit adds to its parameters: the rung it was pointed at.
 *
 * Shapes' `mode` row is one list of six - a solid colour, or one of the five
 * cleaning rungs (`tools.js#SHAPE_MODES`) - and the seam has a field for
 * exactly one of those: `params.engine`, which
 * `src-tauri/src/region.rs#named_rung` reads and runs. A solid fill names no
 * engine at all, because it is not a clean: it crosses as `params.paint`, and
 * the backend's paint branch never reaches the ladder.
 *
 * @returns {Record<string, unknown>}
 */
function shapeEngine() {
  const mode = String(paramsOf('shapes').mode ?? SOLID)
  return mode === SOLID ? {} : { engine: mode }
}

/**
 * @typedef {Object} CommitSpec
 * @property {string} tool
 * @property {string} kind - the draft's shape: `stroke` is the only one that carries a path
 * @property {string} pageId
 * @property {import('./gesture.js').Bbox} bbox
 * @property {'add'|'paint'} mode
 * @property {Array<{x: number, y: number}>} points
 * @property {{x: number, y: number}} start
 */

/**
 * The **path** a gesture swept, or `null` where it swept none.
 *
 * Only a `stroke` draft has one: a brush stroke is a swept disc, and the box
 * around it is the defect this records. A Shapes
 * gesture describes an area rather than a path and sends `paintedShape`
 * below instead; the two payloads are exclusive.
 *
 * The size falls back to the same number `DrawLayer` draws the preview at, so
 * what was on screen and what crosses the seam are one width.
 *
 * @param {CommitSpec} spec
 * @returns {{points: Array<{x: number, y: number}>, radius: number}|null}
 */
function strokeOf(spec) {
  if (spec.kind !== 'stroke') return null
  const size = paramsOf(spec.tool).size ?? (spec.tool === 'aiMaskBrush' ? AI_STROKE_PX : 0)
  return paintedStroke(spec.points, Number(size))
}

/**
 * The **area** a Shapes gesture describes, in the shape the seam carries:
 * the outline and the feather, not the
 * rectangle around them.
 *
 * Until this, only the bbox crossed - so an ellipse cleaned its bounding
 * rectangle and a lasso cleaned the box its curve fitted inside, a defect
 * in the one tool it had not yet been
 * fixed in. Three kinds cover the four shapes: a lasso *is* a polygon, drawn
 * freehand rather than clicked.
 *
 * **Vector, and in the same two spaces `paintedStroke` uses.** The vertices
 * are page percent, like every other `bbox` on the seam; `feather` is page
 * pixels, because it is a distance on the image rather than on the screen and
 * the backend is the only side that knows the page's real size.
 *
 * @param {CommitSpec} spec
 * @returns {{kind: 'rect'|'ellipse'|'polygon', points: Array<{x: number, y: number}>, feather: number}|null}
 */
export function paintedShape(spec) {
  if (spec.tool !== 'shapes') return null
  const box = spec.bbox
  if (!box) return null
  const feather = Math.max(0, Number(paramsOf(spec.tool).feather ?? 0) || 0)

  // A rectangle and an ellipse are their box: the drag says two corners, and
  // the keyboard route says a rectangle outright. The corners travel anyway,
  // so one payload describes all four shapes and the backend has one parser.
  if (spec.kind === 'rect' || spec.kind === 'ellipse') {
    return {
      kind: spec.kind,
      points: corners(box).map(place),
      feather,
    }
  }

  // A lasso and a clicked polygon are the vertices themselves, closed by the
  // backend. Fewer than three is not an area - the surface refuses to commit
  // one, and this is the same rule where the payload is built.
  const points = (spec.points ?? []).filter(
    (point) => point && Number.isFinite(point.x) && Number.isFinite(point.y),
  )
  if (points.length < 3) return null
  return { kind: 'polygon', points: points.map(place), feather }
}

/**
 * @param {import('./gesture.js').Bbox} box
 * @returns {Array<{x: number, y: number}>} clockwise from the top left
 */
function corners(box) {
  return [
    { x: box.x, y: box.y },
    { x: box.x + box.w, y: box.y },
    { x: box.x + box.w, y: box.y + box.h },
    { x: box.x, y: box.y + box.h },
  ]
}

/**
 * @param {{x: number, y: number}} point
 * @returns {{x: number, y: number}} two decimal places, as `paintedStroke`
 *   rounds a path: enough for a 1600px page, and it keeps the payload short
 */
function place(point) {
  return { x: Math.round(point.x * 100) / 100, y: Math.round(point.y * 100) / 100 }
}

/**
 * @param {string} pageId
 * @returns {import('../api/backend.js').ApiPage|null}
 */
function pageOf(pageId) {
  return editor.chapter?.pages.find((page) => page.id === pageId) ?? null
}

/**
 * @param {string} tool
 * @returns {Record<string, unknown>}
 */
function paramsOf(tool) {
  return /** @type {any} */ ($state.snapshot(editor.toolParams[tool] ?? {}))
}

/* ------------------------------------------------------------------ */
/* Creating a mask                                                     */
/* ------------------------------------------------------------------ */

/**
 * A hand-drawn mask: what the gesture painted, cleaned where it was painted.
 *
 * @param {CommitSpec} spec
 * @param {Record<string, unknown>} [extraParams]
 * @returns {Promise<boolean>}
 */
async function createMask(spec, extraParams) {
  const chapter = editor.chapter
  const page = pageOf(spec.pageId)
  if (!chapter || !page) return false

  const before = page.status
  const stroke = strokeOf(spec)
  const shape = paintedShape(spec)
  const mergedParams = {
    ...paramsOf(spec.tool),
    ...(stroke ? { stroke } : {}),
    // The drawn area, for the tools whose gesture is an area rather than a
    // path. `painted` and `stroke` are never both present: a shape has no
    // radius and a stroke has no vertices.
    ...(shape ? { painted: shape } : {}),
    ...(extraParams ?? {}),
  }
  const paint = paintParamsOf(spec.tool, mergedParams, spec.points)
  const params = {
    ...mergedParams,
    ...(paint ? { paint } : {}),
  }

  // The same refusal the other two ways into the seam make, so the gate is on
  // all three rather than on the two that happen to need it today. No
  // `DRAWING_TOOLS` parameter carries `cloud: true` at the moment - the two
  // that do belong to Auto clean and Content-aware fill, neither of which
  // draws - so this cannot fire; one cloud option added to a drawing tool's
  // specs would otherwise be a silent hole.
  if (cloudRefused(spec.tool, params)) return false

  const result = await getBackend().createRegion({
    chapterId: chapter.id,
    pageIndex: page.index,
    bbox: spec.bbox,
    tool: spec.tool,
    params,
  })
  if (!result) return false

  // The redo half is a snapshot, like every other half of every other pair:
  // `applyRegionState` puts this same object into reactive state, and a
  // command must not hold a handle on state that later edits can move under it.
  const created = /** @type {any} */ ($state.snapshot(result.region))
  applyRegionState(result.region.id, result.region, result.pageStatus)
  select(created.id)
  // Before the gesture the region was not there at all, and that is what the
  // delta's `before` side says: `present: false`, which `restoreRegion` reads
  // as "take it away again".
  recordRegionEdit(
    'canvas.command.drawMask',
    created.id,
    { region: null, pageStatus: before },
    { region: created, pageStatus: result.pageStatus },
  )
  return true
}

/* ------------------------------------------------------------------ */
/* Clone / heal                                                        */
/* ------------------------------------------------------------------ */

/**
 * Paint with the stamp. The source must have been sampled first - alt-click on
 * the page, or `S` on the focused drawing surface, which is the sticky
 * equivalent the modifier needs.
 *
 * `aligned` holds the offset between source and stroke once a stroke has
 * established it, so successive strokes read on from one another; `nonAligned`
 * re-anchors to the sampled point on every stroke. The
 * arithmetic is `gesture.js#cloneOffset`; what is recorded on the mask is
 * which of the two was in force and where it read from.
 *
 * @param {CommitSpec} spec
 * @returns {Promise<boolean>}
 */
async function cloneInto(spec) {
  const params = paramsOf(spec.tool)
  const source = draft.cloneSource?.pageId === spec.pageId ? draft.cloneSource : null
  if (!source) {
    notify({ key: 'notice.tool.cloneNeedsSource', tone: 'warn' })
    return false
  }
  const resolved = cloneOffset({
    source: { x: source.x, y: source.y },
    strokeStart: spec.start,
    alignment: String(params.alignment ?? 'aligned'),
    offset: draft.cloneOffset,
  })
  if (!resolved) return false
  setCloneOffset(resolved.offset)
  return createMask(spec, {
    cloneSource: resolved.source,
    cloneOffset: resolved.offset,
  })
}
