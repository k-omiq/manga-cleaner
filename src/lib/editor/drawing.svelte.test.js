/**
 * The AI mask brush's seam: a stroke on the page, and the one adapter call it
 * turns into.
 *
 * **It is always `createRegion`.** The tool used to land on either method - it
 * asked a snapping helper which existing region the stroke was "about" and
 * edited that one through `applyTool` where it found one - and that is how a
 * stroke aimed at a leftover *beside* a layer box re-ran the layer box.
 * The painted shape is the mask now, so the
 * stroke makes its own region and never lands on a neighbour's.
 *
 * What must hold either way is that **the engine the user picked travels with
 * the stroke**. It is chosen in the tool window, held in `editor.toolParams`,
 * and read from `params.engine` by `src-tauri/src/region.rs#named_rung`; a
 * gesture that dropped it would clean with whatever the fill mode implied and
 * give no sign of having done so.
 */

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { editor, setToolParam } from '../state/editor.svelte.js'
import { app } from '../state/app.svelte.js'
import { setBackend } from '../api/backend.js'
import { createHistory } from '../model/history.js'
import { draft, beginDraft, resetDraftState } from './draft.svelte.js'
import { commitDraft } from './drawing.svelte.js'
import { applyActiveToolToRegion } from './toolapply.svelte.js'

/** A page holding whatever regions a case needs. */
function chapterWith(regions) {
  return {
    id: 'c1',
    review: [],
    pages: [
      {
        id: 'c1-p001',
        index: 0,
        number: 1,
        status: 'unclean',
        width: 1600,
        height: 2400,
        regionCount: regions.length,
        regions,
      },
    ],
  }
}

/** The stroke: a draft that has already been dragged, ready to commit. */
function stroke(bbox) {
  beginDraft({
    tool: 'aiMaskBrush',
    kind: 'stroke',
    pageId: 'c1-p001',
    points: [{ x: bbox.x, y: bbox.y }],
    bbox,
    mode: 'add',
    keyboard: false,
    moved: true,
  })
}

/** An adapter that answers both region methods and swallows the history push. */
function backend(overrides = {}) {
  return {
    historyPush: vi.fn().mockResolvedValue({ cursor: 1, entries: [] }),
    createRegion: vi.fn(async ({ bbox }) => ({
      region: { id: 'c1-p001-h1', pageId: 'c1-p001', bbox, source: 'hand', outcome: 'cleaned' },
      pageStatus: 'cleaned',
    })),
    applyTool: vi.fn(async () => ({
      status: 'applied',
      region: { id: 'r1', pageId: 'c1-p001', source: 'hand', outcome: 'cleaned' },
      pageStatus: 'cleaned',
    })),
    ...overrides,
  }
}

describe('the AI mask brush stroke', () => {
  beforeEach(() => {
    app.notices.length = 0
    editor.history = createHistory()
    editor.tool = 'aiMaskBrush'
    editor.pageIndex = 0
    resetDraftState()
  })

  afterEach(() => {
    setBackend(null)
    editor.chapter = null
    editor.tool = 'autoClean'
    resetDraftState()
  })

  it('creates a region with the engine the tool window picked, where nothing was found', async () => {
    editor.chapter = chapterWith([])
    const adapter = backend()
    setBackend(/** @type {any} */ (adapter))
    setToolParam('aiMaskBrush', 'engine', 'lama')

    stroke({ x: 20, y: 30, w: 12, h: 4 })
    expect(await commitDraft()).toBe(true)

    expect(adapter.applyTool).not.toHaveBeenCalled()
    expect(adapter.createRegion).toHaveBeenCalledTimes(1)
    const spec = adapter.createRegion.mock.calls[0][0]
    expect(spec.tool).toBe('aiMaskBrush')
    expect(spec.chapterId).toBe('c1')
    expect(spec.pageIndex).toBe(0)
    expect(spec.params.engine).toBe('lama')
    // The stroke's own box, unchanged: nothing was there to snap to.
    expect(spec.bbox).toEqual({ x: 20, y: 30, w: 12, h: 4 })
  })

  // The stroke overlaps a neighbouring region - a layer box
  // right beside the leftover the user is aiming at - and it must still be its
  // own mask. Retargeting sent it to `applyTool` on that region, which re-ran
  // the neighbour and replaced its patch with the stroke.
  it('never retargets onto a region it merely touches', async () => {
    editor.chapter = chapterWith([
      {
        id: 'r1',
        pageId: 'c1-p001',
        bbox: { x: 22, y: 30, w: 10, h: 4 },
        detected: true,
        outcome: 'cleaned',
      },
    ])
    const adapter = backend()
    setBackend(/** @type {any} */ (adapter))
    setToolParam('aiMaskBrush', 'engine', 'lama')

    stroke({ x: 20, y: 30, w: 12, h: 4 })
    expect(await commitDraft()).toBe(true)

    expect(adapter.applyTool).not.toHaveBeenCalled()
    expect(adapter.createRegion).toHaveBeenCalledTimes(1)
    const spec = adapter.createRegion.mock.calls[0][0]
    expect(spec.params.engine).toBe('lama')
    // The stroke's own box, not the union with the region it grazed.
    expect(spec.bbox).toEqual({ x: 20, y: 30, w: 12, h: 4 })
  })

  // A stroke lying wholly inside an existing region is the same answer: a
  // leftover inside a cleaned box is new paint over it, not a re-run of it.
  it('makes its own mask even for a stroke wholly inside a region', async () => {
    editor.chapter = chapterWith([
      {
        id: 'r1',
        pageId: 'c1-p001',
        bbox: { x: 10, y: 10, w: 40, h: 40 },
        detected: true,
        outcome: 'cleaned',
      },
    ])
    const adapter = backend()
    setBackend(/** @type {any} */ (adapter))

    stroke({ x: 20, y: 20, w: 6, h: 4 })
    expect(await commitDraft()).toBe(true)

    expect(adapter.applyTool).not.toHaveBeenCalled()
    expect(adapter.createRegion.mock.calls[0][0].bbox).toEqual({ x: 20, y: 20, w: 6, h: 4 })
  })

  // The gesture is one undo step whichever method it landed on, and a creation's
  // "before" side is the absence of the region - `restoreRegion` reads that as
  // "take it away again".
  it('records one undoable edit for the stroke', async () => {
    editor.chapter = chapterWith([])
    const adapter = backend()
    setBackend(/** @type {any} */ (adapter))

    stroke({ x: 20, y: 30, w: 12, h: 4 })
    await commitDraft()

    expect(adapter.historyPush).toHaveBeenCalledTimes(1)
    const { entry } = adapter.historyPush.mock.calls[0][0]
    expect(entry.op).toBe('region-state')
    expect(entry.regionId).toBe('c1-p001-h1')
    expect(entry.before.region).toBe(null)
    expect(entry.after.region).toMatchObject({ id: 'c1-p001-h1' })
  })
})

/**
 * A stroke with a real path: an L, whose bounding box is a square the hand
 * never painted.
 *
 * @param {string} tool
 * @param {Array<{x: number, y: number}>} points
 */
function painted(tool, points) {
  const xs = points.map((point) => point.x)
  const ys = points.map((point) => point.y)
  beginDraft({
    tool,
    kind: 'stroke',
    pageId: 'c1-p001',
    points,
    bbox: {
      x: Math.min(...xs),
      y: Math.min(...ys),
      w: Math.max(...xs) - Math.min(...xs),
      h: Math.max(...ys) - Math.min(...ys),
    },
    mode: 'add',
    keyboard: false,
    moved: true,
  })
}

/** The L: down the left, then along the bottom. Its box is a square. */
const L = [
  { x: 20, y: 20 },
  { x: 20, y: 60 },
  { x: 60, y: 60 },
]

/**
 * **The stroke's own shape crosses the seam.** Every assertion here is about the
 * difference between the path and the rectangle around it - an L and its
 * bounding square have identical bounds, so a bbox assertion cannot tell them
 * apart and this is the only place the defect was ever visible from.
 */
describe('a painted stroke', () => {
  beforeEach(() => {
    app.notices.length = 0
    editor.history = createHistory()
    editor.pageIndex = 0
    editor.chapter = chapterWith([])
    resetDraftState()
  })

  afterEach(() => {
    setBackend(null)
    editor.chapter = null
    editor.tool = 'autoClean'
    resetDraftState()
  })

  it('sends the path and the brush radius, not only the box around them', async () => {
    editor.tool = 'brush'
    const adapter = backend()
    setBackend(/** @type {any} */ (adapter))
    setToolParam('brush', 'size', 40)

    painted('brush', L)
    expect(await commitDraft()).toBe(true)

    const { params, bbox } = adapter.createRegion.mock.calls[0][0]
    // The box is still sent - a region is a bbox - and it is still the square.
    expect(bbox).toEqual({ x: 20, y: 20, w: 40, h: 40 })
    // And the shape that is not a square goes with it.
    expect(params.stroke.points).toEqual(L)
    expect(params.stroke.radius).toBe(20)
    // The corner of the bounding box the hand never went near is not on the
    // path - which is the whole of what "not a rectangle" means here.
    expect(params.stroke.points).not.toContainEqual({ x: 60, y: 20 })
  })

  it('carries the AI mask brush’s own shape, over a region or not', async () => {
    editor.tool = 'aiMaskBrush'
    editor.chapter = chapterWith([
      {
        id: 'r1',
        pageId: 'c1-p001',
        bbox: { x: 22, y: 22, w: 30, h: 30 },
        detected: true,
        outcome: 'cleaned',
      },
    ])
    const adapter = backend()
    setBackend(/** @type {any} */ (adapter))

    painted('aiMaskBrush', L)
    expect(await commitDraft()).toBe(true)

    const { params } = adapter.createRegion.mock.calls[0][0]
    expect(params.stroke.points).toEqual(L)
    // The default width the tool ships, as a radius.
    expect(params.stroke.radius).toBe(18)
    // Not the square around the L: the shape is what gets cleaned, and it is
    // the whole reason the backend no longer asks a detector what to clean.
    expect(params.stroke.points).not.toContainEqual({ x: 60, y: 20 })
  })

  it('sends no stroke for a gesture that is an area rather than a path', async () => {
    editor.tool = 'shapes'
    const adapter = backend()
    setBackend(/** @type {any} */ (adapter))

    shape('rect', [{ x: 20, y: 20 }], { x: 20, y: 20, w: 40, h: 40 })
    expect(await commitDraft()).toBe(true)

    // A shape has no radius to sweep. It sends `painted` instead, which the
    // block below is about, and the two are never both present.
    const { params } = adapter.createRegion.mock.calls[0][0]
    expect(params.stroke).toBeUndefined()
    expect(params.painted).toBeDefined()
  })
})

/**
 * A Shapes gesture, ready to commit.
 *
 * @param {'rect'|'ellipse'|'lasso'|'polygon'} kind
 * @param {Array<{x: number, y: number}>} points
 * @param {{x: number, y: number, w: number, h: number}} bbox
 */
function shape(kind, points, bbox) {
  beginDraft({
    tool: 'shapes',
    kind,
    pageId: 'c1-p001',
    points,
    bbox,
    mode: 'add',
    keyboard: false,
    moved: true,
  })
}

/** An L drawn freehand: its bounding box is a square the hand never filled. */
const LASSO = [
  { x: 20, y: 20 },
  { x: 30, y: 20 },
  { x: 30, y: 50 },
  { x: 60, y: 50 },
  { x: 60, y: 60 },
  { x: 20, y: 60 },
]

/**
 * **What a drawn shape sends.** Two things had to change together: the shape
 * itself has to cross the seam - an ellipse used to be committed as its
 * bounding rectangle and a lasso as the box its curve fitted inside, in
 * the last tool it was open in - and the
 * tool's new `mode` row has to reach the backend as the one field that decides
 * what happens to it: `params.engine` for a clean, `params.paint` for a colour.
 */
describe('a drawn shape', () => {
  beforeEach(() => {
    app.notices.length = 0
    editor.history = createHistory()
    editor.tool = 'shapes'
    editor.pageIndex = 0
    editor.chapter = chapterWith([])
    editor.toolParams.shapes = {
      shape: 'rect',
      mode: 'fill',
      color: '#000000',
      opacity: 100,
      feather: 2,
    }
    resetDraftState()
  })

  afterEach(() => {
    setBackend(null)
    editor.chapter = null
    editor.tool = 'autoClean'
    resetDraftState()
  })

  it('sends the rectangle as its four corners, with the feather', async () => {
    const adapter = backend()
    setBackend(/** @type {any} */ (adapter))

    shape('rect', [{ x: 20, y: 20 }], { x: 20, y: 20, w: 40, h: 40 })
    expect(await commitDraft()).toBe(true)

    const { params, bbox } = adapter.createRegion.mock.calls[0][0]
    expect(bbox).toEqual({ x: 20, y: 20, w: 40, h: 40 })
    expect(params.painted).toEqual({
      kind: 'rect',
      points: [
        { x: 20, y: 20 },
        { x: 60, y: 20 },
        { x: 60, y: 60 },
        { x: 20, y: 60 },
      ],
      feather: 2,
    })
  })

  it('sends an ellipse as an ellipse, not as the rectangle it fits in', async () => {
    const adapter = backend()
    setBackend(/** @type {any} */ (adapter))
    setToolParam('shapes', 'shape', 'ellipse')

    shape('ellipse', [{ x: 10, y: 10 }], { x: 10, y: 10, w: 20, h: 30 })
    expect(await commitDraft()).toBe(true)

    const { params } = adapter.createRegion.mock.calls[0][0]
    expect(params.painted.kind).toBe('ellipse')
    // The corners of the box it is inscribed in, which is what the drag said.
    expect(params.painted.points).toHaveLength(4)
  })

  it('sends a lasso and a polygon as their own vertices', async () => {
    for (const kind of /** @type {const} */ (['lasso', 'polygon'])) {
      const adapter = backend()
      setBackend(/** @type {any} */ (adapter))

      shape(kind, LASSO, { x: 20, y: 20, w: 40, h: 40 })
      expect(await commitDraft()).toBe(true)

      const { params } = adapter.createRegion.mock.calls[0][0]
      // One kind for both, because a lasso *is* a polygon drawn freehand.
      expect(params.painted.kind).toBe('polygon')
      expect(params.painted.points).toEqual(LASSO)
      // The corner of the bounding box the hand never went near is not a
      // vertex - which is the whole of what "not a rectangle" means here.
      expect(params.painted.points).not.toContainEqual({ x: 60, y: 20 })
    }
  })

  it('refuses to call two clicks an area', async () => {
    const adapter = backend()
    setBackend(/** @type {any} */ (adapter))

    shape('polygon', LASSO.slice(0, 2), { x: 20, y: 20, w: 10, h: 0.5 })
    expect(await commitDraft()).toBe(true)

    // No shape crossed, so the backend falls back to the box it also sent
    // rather than being handed an outline that encloses nothing.
    expect(adapter.createRegion.mock.calls[0][0].params.painted).toBeUndefined()
  })

  it('names the rung the mode row picked, for every engine on it', async () => {
    for (const engine of ['fill', 'denoise', 'lama', 'flux']) {
      const adapter = backend()
      setBackend(/** @type {any} */ (adapter))
      setToolParam('shapes', 'mode', engine)

      shape('rect', [{ x: 20, y: 20 }], { x: 20, y: 20, w: 40, h: 40 })
      expect(await commitDraft()).toBe(true)

      const { params } = adapter.createRegion.mock.calls[0][0]
      // `region.rs#named_rung` reads this word and runs that rung.
      expect(params.engine).toBe(engine)
      // A clean is not a paint: the backend's paint branch must not be taken.
      expect(params.paint).toBeUndefined()
    }
  })

  it('sends a solid fill as paint, with the shape as the coverage', async () => {
    const adapter = backend()
    setBackend(/** @type {any} */ (adapter))
    setToolParam('shapes', 'mode', 'solid')
    setToolParam('shapes', 'color', '#ff8800')
    setToolParam('shapes', 'opacity', 60)

    shape('ellipse', [{ x: 10, y: 10 }], { x: 10, y: 10, w: 20, h: 30 })
    expect(await commitDraft()).toBe(true)

    const { params } = adapter.createRegion.mock.calls[0][0]
    expect(params.paint).toMatchObject({
      color: '#ff8800',
      opacity: 60,
      // A shape has no soft rim and no build-up: its edge is the feather,
      // which is geometry and travels with the shape.
      hardness: 100,
      flow: 100,
    })
    expect(params.paint.shape).toEqual(params.painted)
    // And it names no engine, because a colour somebody chose is not a rung.
    expect(params.engine).toBeUndefined()
  })

  it('is one undoable edit, whichever mode it was in', async () => {
    const adapter = backend()
    setBackend(/** @type {any} */ (adapter))
    setToolParam('shapes', 'mode', 'solid')

    shape('rect', [{ x: 20, y: 20 }], { x: 20, y: 20, w: 40, h: 40 })
    await commitDraft()

    expect(adapter.historyPush).toHaveBeenCalledTimes(1)
    const { entry } = adapter.historyPush.mock.calls[0][0]
    expect(entry.op).toBe('region-state')
    expect(entry.before.region).toBe(null)
    expect(entry.after.region).toMatchObject({ id: 'c1-p001-h1' })
  })
})

describe('content-aware fill on region click', () => {
  beforeEach(() => {
    app.notices.length = 0
    editor.history = createHistory()
    editor.tool = 'contentAwareFill'
    editor.pageIndex = 0
    resetDraftState()
  })

  afterEach(() => {
    setBackend(null)
    editor.chapter = null
    editor.tool = 'autoClean'
    resetDraftState()
  })

  it('applies content-aware fill to an existing region on click', async () => {
    editor.chapter = chapterWith([
      {
        id: 'r1',
        pageId: 'c1-p001',
        bbox: { x: 10, y: 10, w: 20, h: 20 },
        detected: true,
        outcome: 'unclean',
      },
    ])
    const adapter = backend()
    setBackend(/** @type {any} */ (adapter))

    const applied = await applyActiveToolToRegion('r1')
    expect(applied).toBe(true)
    expect(adapter.applyTool).toHaveBeenCalledTimes(1)
    const spec = adapter.applyTool.mock.calls[0][0]
    expect(spec.tool).toBe('contentAwareFill')
    expect(spec.regionId).toBe('r1')
    expect(editor.selectionId).toBe('r1')
  })
})

describe('paint integration payload contract', () => {
  beforeEach(() => {
    app.notices.length = 0
    editor.history = createHistory()
    editor.chapter = chapterWith([])
    editor.pageIndex = 0
    resetDraftState()
  })

  afterEach(() => {
    setBackend(null)
    editor.chapter = null
    editor.tool = 'autoClean'
    resetDraftState()
  })

  it('builds params.paint for brush in paint mode', async () => {
    editor.tool = 'brush'
    editor.toolParams.brush = {
      size: 30,
      hardness: 80,
      spacing: 15,
      mode: 'paint',
      color: '#ff0000',
      opacity: 90,
      flow: 85,
    }
    const adapter = backend()
    setBackend(/** @type {any} */ (adapter))

    const strokePoints = [
      { x: 10, y: 10, p: 0.8 },
      { x: 20, y: 30, p: 0.6 },
    ]
    painted('brush', strokePoints, 30)
    expect(await commitDraft()).toBe(true)

    const { params, tool } = adapter.createRegion.mock.calls[0][0]
    expect(tool).toBe('brush')
    expect(params.mode).toBe('paint')
    expect(params.color).toBe('#ff0000')
    expect(params.opacity).toBe(90)
    expect(params.flow).toBe(85)
    expect(params.stroke).toEqual({
      points: [{ x: 10, y: 10 }, { x: 20, y: 30 }],
      radius: 15,
    })
    expect(params.paint).toMatchObject({
      points: [
        { x: 10, y: 10, p: 0.8 },
        { x: 20, y: 30, p: 0.6 },
      ],
      color: '#ff0000',
      opacity: 90,
      flow: 85,
      hardness: 80,
      spacing: 15,
      pressureSize: true,
      pressureOpacity: false,
    })
    expect(typeof params.paint.seed).toBe('number')
    expect(params.paint.seed).toBeGreaterThanOrEqual(0)
  })

  it('does not build params.paint for brush in add mode', async () => {
    editor.tool = 'brush'
    editor.toolParams.brush = {
      size: 28,
      hardness: 70,
      spacing: 12,
      mode: 'add',
      color: '#000000',
      opacity: 100,
      flow: 100,
    }
    const adapter = backend()
    setBackend(/** @type {any} */ (adapter))

    painted('brush', [{ x: 10, y: 10 }, { x: 20, y: 20 }], 28)
    expect(await commitDraft()).toBe(true)

    const { params } = adapter.createRegion.mock.calls[0][0]
    expect(params.mode).toBe('add')
    expect(params.paint).toBeUndefined()
  })

  it('builds params.paint with seed and points for cloneHeal', async () => {
    editor.tool = 'cloneHeal'
    editor.toolParams.cloneHeal = {
      size: 32,
      hardness: 60,
      alignment: 'aligned',
      mode: 'heal',
      opacity: 75,
      flow: 80,
    }
    const adapter = backend()
    setBackend(/** @type {any} */ (adapter))

    // Set clone source
    draft.cloneSource = { pageId: 'c1-p001', x: 50, y: 50 }

    const strokePoints = [
      { x: 10, y: 10, p: 0.7 },
      { x: 15, y: 15, p: 0.9 },
    ]
    painted('cloneHeal', strokePoints, 32)
    expect(await commitDraft()).toBe(true)

    const { params, tool } = adapter.createRegion.mock.calls[0][0]
    expect(tool).toBe('cloneHeal')
    expect(params.opacity).toBe(75)
    expect(params.flow).toBe(80)
    expect(params.mode).toBe('heal')
    expect(params.alignment).toBe('aligned')
    expect(params.stroke).toBeDefined()
    expect(params.cloneSource).toEqual({ x: 50, y: 50 })
    expect(params.paint).toMatchObject({
      points: [
        { x: 10, y: 10, p: 0.7 },
        { x: 15, y: 15, p: 0.9 },
      ],
    })
    expect(typeof params.paint.seed).toBe('number')
  })
})

