/**
 * The mounted drawing surface across a long-strip page seam.
 *
 * Geometry unit tests pin the arithmetic, while this test pins the event path:
 * browser pointer coordinates → DrawLayer draft → drawing commit → backend
 * payload and reactive page invalidation.
 */

import { afterEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render, waitFor } from '@testing-library/svelte'
import { setBackend } from '../api/backend.js'
import { createHistory } from '../model/history.js'
import { editor, undo } from '../state/editor.svelte.js'
import { resetDraftState } from './draft.svelte.js'
import DrawLayer from './DrawLayer.svelte'

function page(id, index) {
  return {
    id,
    index,
    number: index + 1,
    status: 'unclean',
    width: 800,
    height: 1000,
    regionCount: 0,
    regions: [],
    tileRevision: 0,
  }
}

afterEach(() => {
  cleanup()
  setBackend(null)
  editor.chapter = null
  editor.tool = 'autoClean'
  resetDraftState()
  vi.clearAllMocks()
})

describe('a mounted long-strip drawing surface', () => {
  it('commits a rectangle across the seam without clamping it to the anchor page', async () => {
    const first = page('c1-p001', 0)
    const second = page('c1-p002', 1)
    editor.chapter = { id: 'c1', review: [], pages: [first, second] }
    editor.history = createHistory()
    editor.pageIndex = 0
    editor.tool = 'shapes'
    editor.toolParams = {
      shapes: { shape: 'rect', mode: 'solid', feather: 0, color: '#ffffff', opacity: 100 },
    }
    resetDraftState()

    const createRegion = vi.fn(async ({ bbox }) => ({
      region: {
        id: 'c1-p001-h1',
        pageId: first.id,
        bbox,
        source: 'hand',
        outcome: 'cleaned',
      },
      pageStatus: 'cleaned',
    }))
    let journalEntry
    const restoreRegion = vi.fn().mockResolvedValue(null)
    setBackend(/** @type {any} */ ({
      createRegion,
      historyPush: vi.fn().mockImplementation(async ({ entry }) => {
        journalEntry = entry
        return { cursor: 1, entries: [{ seq: 1, label: entry.label }] }
      }),
      historyMove: vi.fn().mockImplementation(async () => ({ entry: journalEntry })),
      restoreRegion,
    }))

    const view = render(DrawLayer, {
      props: { page: first, strip: true, stripMinY: 0, stripMaxY: 200 },
    })
    const surface = view.getByRole('button')
    surface.getBoundingClientRect = () => /** @type {DOMRect} */ ({
      x: 0,
      y: 0,
      left: 0,
      top: 0,
      right: 100,
      bottom: 100,
      width: 100,
      height: 100,
      toJSON() {},
    })
    surface.setPointerCapture = vi.fn()
    surface.releasePointerCapture = vi.fn()

    await fireEvent.pointerDown(surface, { pointerId: 7, button: 0, clientX: 20, clientY: 95 })
    await fireEvent.pointerMove(surface, { pointerId: 7, buttons: 1, clientX: 60, clientY: 112 })
    await fireEvent.pointerUp(surface, { pointerId: 7, button: 0, clientX: 60, clientY: 112 })

    await waitFor(() => expect(createRegion).toHaveBeenCalledTimes(1))
    const request = createRegion.mock.calls[0][0]
    expect(request.pageIndex).toBe(0)
    expect(request.bbox).toMatchObject({ x: 20, y: 95, w: 40 })
    expect(request.bbox.h).toBeCloseTo(17)
    expect(request.bbox.y + request.bbox.h).toBeCloseTo(112)
    expect(request.params.painted).toEqual({
      kind: 'rect',
      points: [
        { x: 20, y: 95 },
        { x: 60, y: 95 },
        { x: 60, y: 112 },
        { x: 20, y: 112 },
      ],
      feather: 0,
    })

    // A spanning region invalidates both visible tiles when it is committed.
    expect(editor.chapter.pages[0].tileRevision).toBe(1)
    expect(editor.chapter.pages[1].tileRevision).toBe(1)

    await editor.history.running
    undo()
    await waitFor(() => expect(restoreRegion).toHaveBeenCalledTimes(1))
    await waitFor(() => expect(editor.chapter.pages[1].tileRevision).toBe(2))
    expect(editor.chapter.pages[0].tileRevision).toBe(2)
  })
})
