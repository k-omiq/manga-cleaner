import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import {
  applyRegionState,
  editor,
  openEditorChapter,
  recordRegionEdit,
  select,
  selectedRegion,
  undo,
} from './editor.svelte.js'
import { app } from './app.svelte.js'
import { setBackend } from '../api/backend.js'
import { createHistory, push } from '../model/history.js'

describe('editor history persistence failures', () => {
  beforeEach(() => {
    vi.useFakeTimers()
    app.notices.length = 0
    editor.chapter = { id: 'test-chapter' }
    editor.history = createHistory()
    setBackend(null)
  })

  afterEach(() => {
    vi.useRealTimers()
    setBackend(null)
    editor.chapter = null
  })

  it('historyPush rejecting twice yields one notice and records the error', async () => {
    const pushMock = vi.fn().mockRejectedValue(new Error('disk write failed'))
    setBackend(/** @type {any} */ ({ historyPush: pushMock }))

    const before = { region: null, pageStatus: 'unclean' }
    const after = { region: { id: 'r1', pageId: 'p1' }, pageStatus: 'cleaned' }

    recordRegionEdit('canvas.command.applyTool', 'r1', before, after)
    expect(pushMock).toHaveBeenCalledTimes(1)
    expect(app.notices).toHaveLength(0)

    // Advance timer past retry delay (50ms) to trigger retry and allow promises to settle
    await vi.advanceTimersByTimeAsync(60)

    expect(pushMock).toHaveBeenCalledTimes(2)
    expect(app.notices).toHaveLength(1)
    expect(app.notices[0]).toMatchObject({
      key: 'notice.history.saveFailed',
      tone: 'warn',
    })
    expect(editor.history.error).toBeInstanceOf(Error)
  })

  it('historyPush rejecting once then succeeding yields no notice', async () => {
    const pushMock = vi
      .fn()
      .mockRejectedValueOnce(new Error('temporary disk lock'))
      .mockResolvedValueOnce({
        cursor: 1,
        entries: [{ seq: 1, label: 'canvas.command.applyTool' }],
      })
    setBackend(/** @type {any} */ ({ historyPush: pushMock }))

    const before = { region: null, pageStatus: 'unclean' }
    const after = { region: { id: 'r1', pageId: 'p1' }, pageStatus: 'cleaned' }

    recordRegionEdit('canvas.command.applyTool', 'r1', before, after)
    expect(pushMock).toHaveBeenCalledTimes(1)
    expect(app.notices).toHaveLength(0)

    // Advance timer past retry delay
    await vi.advanceTimersByTimeAsync(60)

    expect(pushMock).toHaveBeenCalledTimes(2)
    expect(app.notices).toHaveLength(0)
    expect(editor.history.entries).toEqual([
      { seq: 1, label: 'canvas.command.applyTool' },
    ])
  })

  it('historyMove rejecting twice during undo yields one notice and records the error', async () => {
    const moveMock = vi.fn().mockRejectedValue(new Error('disk move failed'))
    setBackend(/** @type {any} */ ({ historyMove: moveMock }))

    push(editor.history, { seq: 1, label: 'canvas.command.applyTool' })
    expect(editor.history.cursor).toBe(1)

    undo()
    await vi.advanceTimersByTimeAsync(0)
    expect(moveMock).toHaveBeenCalledTimes(1)
    expect(app.notices).toHaveLength(0)

    await vi.advanceTimersByTimeAsync(60)

    expect(moveMock).toHaveBeenCalledTimes(2)
    expect(app.notices).toHaveLength(1)
    expect(app.notices[0]).toMatchObject({
      key: 'notice.history.saveFailed',
      tone: 'warn',
    })
    expect(editor.history.error).toBeInstanceOf(Error)
  })

  it('historyMove rejecting once then succeeding during undo yields no notice', async () => {
    const moveMock = vi
      .fn()
      .mockRejectedValueOnce(new Error('temporary move lock'))
      .mockResolvedValueOnce({
        cursor: 0,
        entry: null,
      })
    setBackend(/** @type {any} */ ({ historyMove: moveMock }))

    push(editor.history, { seq: 1, label: 'canvas.command.applyTool' })
    expect(editor.history.cursor).toBe(1)

    undo()
    await vi.advanceTimersByTimeAsync(0)
    expect(moveMock).toHaveBeenCalledTimes(1)
    expect(app.notices).toHaveLength(0)

    await vi.advanceTimersByTimeAsync(60)

    expect(moveMock).toHaveBeenCalledTimes(2)
    expect(app.notices).toHaveLength(0)
  })

  it('recordRegionEdit followed immediately by undo preserves cursor 0 when historyPush resolves', async () => {
    /** @type {(value: any) => void} */
    let resolvePush
    const pushPromise = new Promise((res) => {
      resolvePush = res
    })
    const pushMock = vi.fn().mockReturnValue(pushPromise)
    const moveMock = vi.fn().mockResolvedValue({ cursor: 0, entry: null })
    setBackend(/** @type {any} */ ({ historyPush: pushMock, historyMove: moveMock }))

    const before = { region: null, pageStatus: 'unclean' }
    const after = { region: { id: 'r1', pageId: 'p1' }, pageStatus: 'cleaned' }

    recordRegionEdit('canvas.command.applyTool', 'r1', before, after)
    expect(editor.history.cursor).toBe(1)

    undo()
    expect(editor.history.cursor).toBe(0)

    // Push resolves with backend's view (which reported cursor 1 at time of push)
    resolvePush({ cursor: 1, entries: [{ seq: 1, label: 'canvas.command.applyTool' }] })
    await vi.advanceTimersByTimeAsync(0)

    expect(editor.history.cursor).toBe(0)
    expect(moveMock).toHaveBeenCalledWith(expect.objectContaining({ direction: 'undo' }))
  })

  it('delayed retry of first historyPush does not overwrite subsequent edit', async () => {
    const pushCalls = []
    const pushMock = vi.fn().mockImplementation(async ({ entry }) => {
      pushCalls.push(entry.label)
      if (entry.label === 'edit1' && pushCalls.filter((l) => l === 'edit1').length === 1) {
        throw new Error('fail 1')
      }
      if (entry.label === 'edit1') {
        return { cursor: 1, entries: [{ seq: 1, label: 'edit1' }] }
      }
      return {
        cursor: 2,
        entries: [
          { seq: 1, label: 'edit1' },
          { seq: 2, label: 'edit2' },
        ],
      }
    })
    setBackend(/** @type {any} */ ({ historyPush: pushMock }))

    const before = { region: null, pageStatus: 'unclean' }
    const after1 = { region: { id: 'r1', pageId: 'p1' }, pageStatus: 'cleaned' }
    const after2 = { region: { id: 'r2', pageId: 'p1' }, pageStatus: 'cleaned' }

    recordRegionEdit('edit1', 'r1', before, after1)
    expect(pushMock).toHaveBeenCalledTimes(1)

    // While edit 1 is waiting for retry delay, record edit 2
    recordRegionEdit('edit2', 'r2', before, after2)

    // Advance past retry delay and allow queued pushes to complete in order
    await vi.advanceTimersByTimeAsync(100)

    expect(editor.history.cursor).toBe(2)
    expect(editor.history.entries).toEqual([
      { seq: 1, label: 'edit1' },
      { seq: 2, label: 'edit2' },
    ])
  })
})

describe('applyRegionState on non-resident pages', () => {
  beforeEach(() => {
    editor.chapter = null
  })

  it('applyRegionState on an evicted page updates review index without corrupting other entries or mutating regions', () => {
    editor.chapter = {
      id: 'ch1',
      pages: [
        {
          id: 'p0',
          index: 0,
          resident: false,
          regions: [],
          regionCount: 2,
          doneCount: 0,
          reviewCount: 2,
          status: 'cleaned',
        },
      ],
      review: [
        { id: 'r1', pageId: 'p0', pageIndex: 0, reasonKey: 'review.reason.unusuallyLarge' },
        { id: 'r2', pageId: 'p0', pageIndex: 0, reasonKey: 'review.reason.declined' },
      ],
    }

    const updatedR1 = {
      id: 'r1',
      pageId: 'p0',
      bbox: { x: 0, y: 0, w: 1, h: 1 },
      source: 'auto',
      detected: true,
      outcome: 'cleaned',
      gateSkipCause: null,
      declineReason: null,
      unusuallyLarge: false,
      mask: { id: 'r1-m1', regionId: 'r1', sequence: 1, fittingReconstructed: false, cloudOutcome: null },
    }

    applyRegionState('r1', updatedR1, 'cleaned')
    const page = editor.chapter.pages[0]
    expect(page.regions).toEqual([])
    expect(editor.chapter.review).toEqual([
      { id: 'r2', pageId: 'p0', pageIndex: 0, reasonKey: 'review.reason.declined' },
    ])
    expect(page.reviewCount).toBe(1)
  })

  it('applyRegionState with null region on an evicted page removes review entry and decrements counts', () => {
    editor.chapter = {
      id: 'ch1',
      pages: [
        {
          id: 'p0',
          index: 0,
          resident: false,
          regions: [],
          regionCount: 2,
          doneCount: 0,
          reviewCount: 1,
          status: 'cleaned',
        },
      ],
      review: [
        { id: 'r1', pageId: 'p0', pageIndex: 0, reasonKey: 'review.reason.unusuallyLarge' },
      ],
    }

    applyRegionState('r1', null, 'unclean')
    const page = editor.chapter.pages[0]
    expect(editor.chapter.review).toHaveLength(0)
    expect(page.regionCount).toBe(1)
    expect(page.reviewCount).toBe(0)
    expect(page.status).toBe('unclean')
  })
})

/**
 * A run reports regions on pages the reader is nowhere near, and those pages
 * are headers. The Pages list draws a row for every one of them,
 * so the counts on the header - and the chapter-wide review index - have to
 * move without the regions ever arriving.
 */
describe('a run over pages the window is not holding', () => {
  /** @type {(event: any) => void} */
  let emit

  const header = (index) => ({
    id: `p${index}`,
    chapterId: 'ch1',
    index,
    number: index + 1,
    status: 'unclean',
    skipReason: null,
    regions: [],
    regionCount: 0,
    doneCount: 0,
    reviewCount: 0,
    resident: false,
  })

  const cleanedRegion = (id, pageId, overrides = {}) => ({
    id,
    pageId,
    bbox: { x: 0, y: 0, w: 1, h: 1 },
    source: 'auto',
    detected: true,
    outcome: 'cleaned',
    gateSkipCause: null,
    declineReason: null,
    unusuallyLarge: false,
    mask: { id: `${id}-m1`, regionId: id, sequence: 1, fittingReconstructed: false, cloudOutcome: null },
    ...overrides,
  })

  beforeEach(async () => {
    editor.chapter = null
    editor.project = null
    setBackend(
      /** @type {any} */ ({
        subscribe: (/** @type {any} */ fn) => {
          emit = fn
          return () => {}
        },
        openChapter: async () => ({
          project: { id: 'pr1', mode: 'single', readingDirection: 'rtl' },
          chapter: {
            id: 'ch1',
            projectId: 'pr1',
            pages: [header(0), header(1), header(2), header(3), header(4)],
            review: [],
          },
          pendingConversion: null,
        }),
        // The window is asked for separately; nothing comes back, so every
        // page stays a header and the counts have only one place to come from.
        loadPages: async () => [],
        historyLoad: async () => ({ cursor: 0, entries: [] }),
      }),
    )
    await openEditorChapter('pr1', 'ch1')
  })

  afterEach(() => {
    setBackend(null)
    editor.chapter = null
    editor.project = null
  })

  it('counts a finished region onto the header of a page it is not holding', () => {
    emit({ type: 'region-done', chapterId: 'ch1', pageIndex: 4, region: cleanedRegion('r1', 'p4') })
    const page = editor.chapter.pages[4]
    expect(page.resident).toBe(false)
    expect(page.regionCount).toBe(1)
    expect(page.doneCount).toBe(1)
    expect(page.reviewCount).toBe(0)
    expect(editor.chapter.review).toHaveLength(0)
  })

  it('puts a flagged region into the chapter-wide index and onto the header', () => {
    emit({
      type: 'region-done',
      chapterId: 'ch1',
      pageIndex: 3,
      region: cleanedRegion('r2', 'p3', { unusuallyLarge: true }),
    })
    const page = editor.chapter.pages[3]
    expect(page.reviewCount).toBe(1)
    expect(page.doneCount).toBe(0)
    expect(editor.chapter.review).toEqual([
      { id: 'r2', pageId: 'p3', pageIndex: 3, reasonKey: 'review.reason.unusuallyLarge' },
    ])
  })

  it('does not count a region twice when the run replaces one it already flagged', () => {
    const flagged = cleanedRegion('r3', 'p3', { unusuallyLarge: true })
    emit({ type: 'region-done', chapterId: 'ch1', pageIndex: 3, region: flagged })
    emit({ type: 'region-done', chapterId: 'ch1', pageIndex: 3, region: flagged })
    expect(editor.chapter.pages[3].reviewCount).toBe(1)
    expect(editor.chapter.review).toHaveLength(1)
  })

  it('takes the whole page-done payload as counts, then drops the regions', () => {
    emit({
      type: 'page-done',
      chapterId: 'ch1',
      page: {
        ...header(2),
        status: 'cleaned',
        resident: true,
        regions: [
          cleanedRegion('a', 'p2'),
          cleanedRegion('b', 'p2'),
          cleanedRegion('c', 'p2', { outcome: 'declined', mask: null }),
        ],
      },
    })
    const page = editor.chapter.pages[2]
    expect(page.resident).toBe(false)
    expect(page.regions).toEqual([])
    expect(page.regionCount).toBe(3)
    expect(page.doneCount).toBe(2)
    expect(page.reviewCount).toBe(1)
    expect(editor.chapter.review.map((entry) => entry.id)).toEqual(['c'])
  })
})

describe('tool parameters defaults', () => {
  it('initializes brush with color, opacity, and flow', () => {
    expect(editor.toolParams.brush).toMatchObject({
      size: 28,
      hardness: 70,
      spacing: 12,
      mode: 'paint',
      color: '#000000',
      opacity: 100,
      flow: 100,
    })
  })

  it('initializes cloneHeal with opacity and flow', () => {
    expect(editor.toolParams.cloneHeal).toMatchObject({
      size: 32,
      hardness: 60,
      opacity: 100,
      flow: 100,
      alignment: 'aligned',
      mode: 'heal',
    })
  })
})


/**
 * A removal has to reach the page even when the page is a header and the
 * region was never flagged - the review index is the only index there is, and
 * it only knows about regions that need review.
 */
describe('a removal on a page the window is not holding', () => {
  beforeEach(() => {
    editor.chapter = null
  })

  afterEach(() => {
    editor.chapter = null
    editor.selectionId = null
    editor.hoverId = null
  })

  /** @returns {any} one evicted page with three regions, one of them flagged */
  function evicted() {
    return {
      id: 'ch1',
      pages: [
        {
          id: 'ch1-p001',
          index: 0,
          resident: false,
          regions: [],
          regionCount: 3,
          doneCount: 2,
          reviewCount: 1,
          status: 'cleaned',
        },
      ],
      review: [
        { id: 'ch1-p001-r2', pageId: 'ch1-p001', pageIndex: 0, reasonKey: 'review.reason.declined' },
      ],
    }
  }

  it('finds the page from the region id when the review index has never heard of it', () => {
    editor.chapter = evicted()
    // `ch1-p001-r0` is an ordinary cleaned region: nothing flagged it, so it
    // is in no index at all. This used to return false and leave the header
    // counting a region that had gone.
    expect(applyRegionState('ch1-p001-r0', null, 'cleaned')).toBe(true)
    const page = editor.chapter.pages[0]
    expect(page.regionCount).toBe(2)
    expect(page.reviewCount).toBe(1)
    expect(page.status).toBe('cleaned')
  })

  it('keeps the header arithmetic possible - done can never exceed what is left', () => {
    editor.chapter = evicted()
    applyRegionState('ch1-p001-r0', null, 'cleaned')
    const page = editor.chapter.pages[0]
    expect(page.doneCount).toBeLessThanOrEqual(page.regionCount - page.reviewCount)
    expect(page.doneCount).toBeGreaterThanOrEqual(0)
  })

  it('still goes through the review index for a flagged region', () => {
    editor.chapter = evicted()
    expect(applyRegionState('ch1-p001-r2', null, 'cleaned')).toBe(true)
    const page = editor.chapter.pages[0]
    expect(editor.chapter.review).toHaveLength(0)
    expect(page.regionCount).toBe(2)
    expect(page.reviewCount).toBe(0)
  })

  it('takes the longest matching page id, because page ids are not prefix-free', () => {
    editor.chapter = evicted()
    editor.chapter.pages.push({
      id: 'ch1-p001-x',
      index: 1,
      resident: false,
      regions: [],
      regionCount: 4,
      doneCount: 0,
      reviewCount: 0,
      status: 'cleaned',
    })
    applyRegionState('ch1-p001-x-r0', null, 'cleaned')
    expect(editor.chapter.pages[0].regionCount).toBe(3)
    expect(editor.chapter.pages[1].regionCount).toBe(3)
  })

  it('answers false for an id that names no page at all', () => {
    editor.chapter = evicted()
    expect(applyRegionState('ch9-p001-r0', null, 'cleaned')).toBe(false)
  })
})

describe('a selection whose region has been removed', () => {
  afterEach(() => {
    editor.chapter = null
    editor.selectionId = null
    editor.hoverId = null
  })

  /** @param {boolean} resident */
  function chapterWith(resident) {
    const region = {
      id: 'ch1-p001-r0',
      pageId: 'ch1-p001',
      bbox: { x: 0, y: 0, w: 10, h: 10 },
      source: 'auto',
      outcome: 'cleaned',
      mask: { id: 'ch1-p001-r0-m1', sequence: 1, provenance: { engine: 'lama' } },
    }
    editor.chapter = {
      id: 'ch1',
      pages: [
        {
          id: 'ch1-p001',
          index: 0,
          resident,
          regions: resident ? [region] : [],
          regionCount: 1,
          doneCount: 1,
          reviewCount: 0,
          status: 'cleaned',
        },
      ],
      review: [],
    }
    return region
  }

  it('is dropped, on the page the window is holding', () => {
    chapterWith(true)
    select('ch1-p001-r0')
    editor.hoverId = 'ch1-p001-r0'
    applyRegionState('ch1-p001-r0', null, 'unclean')
    expect(editor.selectionId).toBe(null)
    expect(editor.hoverId).toBe(null)
  })

  it('is dropped on a page the window is not holding too', () => {
    chapterWith(false)
    select('ch1-p001-r0')
    applyRegionState('ch1-p001-r0', null, 'unclean')
    expect(editor.selectionId).toBe(null)
  })

  it('survives an edit that only replaces the region', () => {
    const region = chapterWith(true)
    select('ch1-p001-r0')
    applyRegionState('ch1-p001-r0', { ...region, outcome: 'declined' }, 'cleaned')
    expect(editor.selectionId).toBe('ch1-p001-r0')
  })

  it('leaves a selection that names some other region alone', () => {
    chapterWith(true)
    select('ch1-p001-r9')
    applyRegionState('ch1-p001-r0', null, 'unclean')
    expect(editor.selectionId).toBe('ch1-p001-r9')
  })
})

describe('selectedRegion', () => {
  afterEach(() => {
    editor.chapter = null
    editor.selectionId = null
  })

  it('is the region the selection names, so the editor-wide delete has one to give', () => {
    editor.chapter = {
      id: 'ch1',
      pages: [
        {
          id: 'ch1-p001',
          index: 0,
          regions: [{ id: 'ch1-p001-r0', pageId: 'ch1-p001', mask: null }],
          status: 'cleaned',
        },
      ],
      review: [],
    }
    select('ch1-p001-r0')
    expect(selectedRegion()?.id).toBe('ch1-p001-r0')
  })

  it('is null with nothing selected, and null for an id nothing holds', () => {
    editor.chapter = { id: 'ch1', pages: [], review: [] }
    select(null)
    expect(selectedRegion()).toBe(null)
    select('ch1-p001-r0')
    expect(selectedRegion()).toBe(null)
  })
})
