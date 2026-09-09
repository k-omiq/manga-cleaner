/**
 * The run scheduler, and only the run scheduler - the one piece of this layer
 * with logic rather than data. Fixture contents are not tested (they are
 * data), and neither are the typedefs.
 *
 * Timings are injected so fake timers drive the whole run: a page costs
 * `region` ms per pending region plus `pageTail` ms.
 */

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { createMockBackend } from './mock.js'
import { maskRows } from '../editor/maskrows.js'

const TIMING = { method: 0, openChapter: 0, export: 0, region: 100, pageTail: 100, noticeStagger: 0 }

/** A chapter with 20 pages, none of them cleaned. */
const CHAPTER = 'tsuki-to-hane-ch107'
/** A chapter of 48 pages with no text detected on any of them. */
const EMPTY_CHAPTER = 'yoake-photobook-ch1'

function makeBackend() {
  const backend = createMockBackend({ timing: TIMING })
  /** @type {Object[]} */
  const events = []
  backend.subscribe((event) => events.push(event))
  return { backend, events }
}

/** Resolves a backend promise, flushing every timer it waits on. */
async function settle(promise) {
  await vi.runAllTimersAsync()
  return promise
}

/** Resolves a backend promise without running the run it may have started. */
async function begin(promise) {
  await vi.advanceTimersByTimeAsync(0)
  return promise
}

beforeEach(() => vi.useFakeTimers())
afterEach(() => vi.useRealTimers())

/**
 * Every page of a chapter, paged in.
 *
 * A chapter crosses the seam as page **headers** -
 * the regions arrive only for the window somebody is looking at - so a test
 * that wants to reason about a chapter's regions has to ask for them, exactly
 * as the editor does.
 */
async function pagedIn(backend, chapter, settle) {
  return settle(
    backend.loadPages({ chapterId: chapter.id, indices: chapter.pages.map((p) => p.index) }),
  )
}

describe('run scheduler', () => {
  it('emits every page in order and finishes', async () => {
    const { backend, events } = makeBackend()

    const handle = await settle(backend.runClean({ scope: 'chapter', chapterId: CHAPTER }))

    expect(handle.pages).toHaveLength(20)

    const started = events.filter((e) => e.type === 'page-started').map((e) => e.pageIndex)
    const done = events.filter((e) => e.type === 'page-done').map((e) => e.pageIndex)
    const queued = handle.pages.map((page) => page.pageIndex)
    expect(started).toEqual(queued)
    expect(done).toEqual(queued)

    // Every region of a page is emitted between that page's start and its end.
    for (const pageIndex of queued) {
      const openedAt = events.findIndex((e) => e.type === 'page-started' && e.pageIndex === pageIndex)
      const closedAt = events.findIndex((e) => e.type === 'page-done' && e.pageIndex === pageIndex)
      const regions = events.filter((e) => e.type === 'region-done' && e.pageId === events[closedAt].pageId)
      expect(regions.length).toBeGreaterThan(0)
      for (const region of regions) {
        const at = events.indexOf(region)
        expect(at).toBeGreaterThan(openedAt)
        expect(at).toBeLessThan(closedAt)
      }
    }

    const finished = events.filter((e) => e.type === 'run-finished')
    expect(finished).toHaveLength(1)
    expect(finished[0]).toMatchObject({
      runId: handle.runId,
      reason: 'completed',
      pagesQueued: 20,
      pagesCleaned: 20,
      nextPageIndex: null,
    })
    expect(finished[0].regionsCleaned).toBe(
      events.filter((e) => e.type === 'region-done').length,
    )
    expect(events.at(-1)).toMatchObject({ type: 'notice', key: 'notice.run.finished' })
  })

  it('keeps completed regions and stops emitting when cancelled mid-run', async () => {
    const { backend, events } = makeBackend()

    const handle = await begin(backend.runClean({ scope: 'chapter', chapterId: CHAPTER }))
    expect(handle.runId).not.toBeNull()

    // Long enough for a couple of pages, far short of all twenty.
    await vi.advanceTimersByTimeAsync(900)
    const cleanedBefore = events.filter((e) => e.type === 'region-done').length
    expect(cleanedBefore).toBeGreaterThan(0)
    expect(events.some((e) => e.type === 'run-finished')).toBe(false)

    const cancelled = await begin(backend.cancelRun())
    expect(cancelled).toBe(handle.runId)

    const finished = events.filter((e) => e.type === 'run-finished')
    expect(finished).toHaveLength(1)
    expect(finished[0].reason).toBe('cancelled')
    expect(finished[0].pagesCleaned).toBeLessThan(20)
    expect(finished[0].nextPageIndex).not.toBeNull()

    const eventCount = events.length
    await vi.advanceTimersByTimeAsync(60_000)
    expect(events).toHaveLength(eventCount)

    // The regions that finished are still cleaned; the job is incomplete.
    const projects = await settle(backend.listProjects())
    const chapter = projects
      .flatMap((project) => project.chapters)
      .find((c) => c.id === CHAPTER)
    const cleanedRegions = (await pagedIn(backend, chapter, settle))
      .flatMap((page) => page.regions)
      .filter((region) => region.outcome === 'cleaned')
    expect(cleanedRegions).toHaveLength(cleanedBefore)
    expect(chapter.pages.some((page) => page.status === 'unclean')).toBe(true)
    expect(chapter.pages.every((page) => page.status !== 'cleaning')).toBe(true)
  })

  it('restores a region so a deleted mask is addressable again', async () => {
    const { backend } = makeBackend()

    const opened = await settle(
      backend.openChapter({ projectId: 'wandering-moon', chapterId: 'wandering-moon-ch12' }),
    )
    const region = (await pagedIn(backend, opened.chapter, settle))
      .flatMap((page) => page.regions)
      .find((candidate) => candidate.mask)
    const maskId = region.mask.id

    const deleted = await settle(backend.deleteMask({ maskId }))
    // Deleting a mask deletes the row: no region comes back, and none is left
    // on the page for the panel to list as an empty one.
    expect(deleted.region).toBeNull()
    expect(deleted.pageStatus).toBeTypeOf('string')
    const afterDelete = (await pagedIn(backend, opened.chapter, settle)).flatMap((p) => p.regions)
    expect(afterDelete.some((candidate) => candidate.id === region.id)).toBe(false)
    expect(maskRows(afterDelete).some((row) => row.id === region.id)).toBe(false)
    // The undo route: without it the mask would be gone from the backend and
    // every action on the restored row would find nothing.
    expect(await settle(backend.rerunMask({ maskId, kind: 'stronger' }))).toBeNull()

    const restored = await settle(backend.restoreRegion({ regionId: region.id, region }))
    expect(restored.mask.id).toBe(maskId)
    expect(restored.outcome).toBe('cleaned')
    expect(await settle(backend.rerunMask({ maskId, kind: 'stronger' }))).not.toBeNull()
  })

  it('re-runs a mask at the rung it already used, and at a rung it is given', async () => {
    const { backend, events } = makeBackend()

    const opened = await settle(
      backend.openChapter({ projectId: 'wandering-moon', chapterId: 'wandering-moon-ch12' }),
    )
    const region = (await pagedIn(backend, opened.chapter, settle))
      .flatMap((page) => page.regions)
      .find((candidate) => candidate.mask)
    const maskId = region.mask.id
    const engine = region.mask.provenance.engine

    // Try again: the same rung, a new revision. The Layers row's own control,
    // and the one thing the four ladder actions could not express.
    const again = await settle(backend.rerunMask({ maskId, kind: 'retry' }))
    expect(again.mask.provenance.engine).toBe(engine)
    expect(again.mask.sequence).toBeGreaterThan(region.mask.sequence)
    expect(events.some((e) => e.key === 'notice.mask.rerunAgain')).toBe(true)

    // A named rung, whichever direction it lies in.
    const named = await settle(
      backend.rerunMask({ maskId: again.mask.id, kind: 'engine', engine: 'denoise' }),
    )
    expect(named.mask.provenance.engine).toBe('denoise')
    expect(events.some((e) => e.key === 'notice.mask.rerunEngine')).toBe(true)

    // A rung this build does not have is not a guess: it runs again unchanged.
    const unknown = await settle(
      backend.rerunMask({ maskId: named.mask.id, kind: 'engine', engine: 'diffusion-9000' }),
    )
    expect(unknown.mask.provenance.engine).toBe('denoise')
  })

  it('creates a hand-drawn region, and removes it again on undo', async () => {
    const { backend, events } = makeBackend()

    const opened = await settle(
      backend.openChapter({ projectId: 'wandering-moon', chapterId: 'wandering-moon-ch12' }),
    )
    const page = (await pagedIn(backend, opened.chapter, settle))[0]
    const before = page.regions.length
    const bbox = { x: 42, y: 42, w: 12, h: 9 }

    const created = await settle(
      backend.createRegion({
        chapterId: opened.chapter.id,
        pageIndex: page.index,
        bbox,
        tool: 'brush',
        params: { mode: 'add' },
      }),
    )

    // A hand mask is identical in kind to an automatic one; `source` is the
    // only difference, and the geometry is the caller's own.
    expect(created.region.source).toBe('hand')
    expect(created.region.tool).toBe('brush')
    expect(created.region.outcome).toBe('cleaned')
    expect(created.region.bbox).toEqual(bbox)
    expect(created.region.mask.provenance.engine).toBe('fill')
    expect(created.pageStatus).toBeTypeOf('string')

    // A listing carries headers, so the count comes off the header and the
    // regions come from a window - which is the whole of §1b at the seam.
    const headerFor = (projects) =>
      projects
        .flatMap((project) => project.chapters)
        .find((chapter) => chapter.id === opened.chapter.id)
        .pages.find((candidate) => candidate.id === page.id)
    expect(headerFor(await settle(backend.listProjects())).regionCount).toBe(before + 1)

    // And the header carries the other two counts as well, because the Pages
    // list draws a ratio and a ✓ mark for a page it is not holding.
    const header = headerFor(await settle(backend.listProjects()))
    expect(header.resident).toBe(false)
    expect(header.doneCount).toBeGreaterThan(0)
    expect(header.doneCount + header.reviewCount).toBeLessThanOrEqual(header.regionCount)

    // Undo: before the gesture the region was not there, and that is what the
    // snapshot says. Redo puts the same region back, mask and all.
    await settle(backend.restoreRegion({ regionId: created.region.id, region: null }))
    expect(headerFor(await settle(backend.listProjects())).regionCount).toBe(before)

    const redone = await settle(
      backend.restoreRegion({ regionId: created.region.id, region: created.region }),
    )
    expect(redone.mask.id).toBe(created.region.mask.id)
    expect(headerFor(await settle(backend.listProjects())).regionCount).toBe(before + 1)

    // A re-run finds the restored mask, which is the whole point of routing
    // undo back through the seam.
    expect(
      await settle(backend.rerunMask({ maskId: created.region.mask.id, kind: 'simpler' })),
    ).not.toBeNull()

  })

  // **No notice, and no snapping mechanism to report one about.** The tool used
  // to say `aiSnapped` or `aiFallback` - which of the detector's answers made
  // the mask - and there is no detector on this path any more: the stroke is
  // the mask. What the stroke *does* carry is
  // the engine, named outright, and the committed mask has to record it.
  it('cleans an AI mask brush stroke with the engine it named, and says nothing', async () => {
    const { backend, events } = makeBackend()

    const opened = await settle(
      backend.openChapter({ projectId: 'wandering-moon', chapterId: 'wandering-moon-ch12' }),
    )
    const region = (await pagedIn(backend, opened.chapter, settle))
      .flatMap((page) => page.regions)
      .find((candidate) => candidate.detected !== false)

    const applied = await settle(
      backend.applyTool({
        tool: 'aiMaskBrush',
        regionId: region.id,
        params: { engine: 'lama' },
      }),
    )

    expect(applied.mask.provenance.engine).toBe('lama')
    const keys = events.filter((e) => e.type === 'notice').map((e) => e.key)
    expect(keys).not.toContain('notice.tool.aiSnapped')
    expect(keys).not.toContain('notice.tool.aiFallback')
  })

  it('keeps a region a hand tool has touched on the detected side of the fork', async () => {
    const { backend, events } = makeBackend()

    const opened = await settle(
      backend.openChapter({ projectId: 'wandering-moon', chapterId: 'wandering-moon-ch12' }),
    )
    const missed = (await pagedIn(backend, opened.chapter, settle))
      .flatMap((page) => page.regions)
      .find((region) => region.detected === false)

    // A hand tool marks the region detected, deliberately: after it the app
    // holds a box for text the auto pass never found, which is what puts it
    // back inside the automatic queue if the mask is later deleted.
    await settle(backend.applyTool({ tool: 'aiMaskBrush', regionId: missed.id, params: {} }))
    events.length = 0
    const again = await settle(
      backend.applyTool({ tool: 'aiMaskBrush', regionId: missed.id, params: {} }),
    )

    expect(again.region.detected).toBe(true)
    const keys = events.filter((e) => e.type === 'notice').map((e) => e.key)
    expect(keys).not.toContain('notice.tool.aiSnapped')
  })

  it('adopts a hand-drawn geometry and reports the page it moved', async () => {
    const { backend } = makeBackend()

    const opened = await settle(
      backend.openChapter({ projectId: 'tsuki-to-hane', chapterId: CHAPTER }),
    )
    const page = (await pagedIn(backend, opened.chapter, settle)).find(
      (candidate) => candidate.status === 'unclean',
    )
    const region = page.regions[0]
    const bbox = { x: 11, y: 12, w: 13, h: 14 }

    const result = await settle(
      backend.applyTool({
        tool: 'shapes',
        regionId: region.id,
        chapterId: opened.chapter.id,
        pageIndex: page.index,
        params: { bbox },
      }),
    )

    // A hand-drawn mask is authoritative: the geometry replaces the
    // detector's box rather than being reconciled with it.
    expect(result.status).toBe('applied')
    expect(result.region.bbox).toEqual(bbox)
    expect(result.region.source).toBe('hand')
    // And the page's status comes back with it, as it does from every other
    // region-level edit.
    expect(result.pageStatus).toBe('cleaned')
  })

  // `engineCeiling` is a ceiling in both directions. `startRun` pins every run
  // to the highest *local* rung so a batch can never reach the cloud, and that
  // pin must not become a way of reaching past a user whose setting is lower:
  // the stored ceiling here is `lama`, one rung below the pin.
  it('never routes past the stored ceiling, whatever the caller asks for', async () => {
    const { backend, events } = makeBackend()
    await settle(backend.runClean({ scope: 'chapter', chapterId: CHAPTER, engineCeiling: 'cloud' }))

    const engines = new Set(
      events
        .filter((event) => event.type === 'region-done' && event.region.mask)
        .map((event) => event.region.mask.provenance.engine),
    )
    expect(engines.size).toBeGreaterThan(0)
    expect(engines.has('cloud')).toBe(false)
  })

  it('honors bubbleEngine and outsideEngine picks during auto clean', async () => {
    const { backend: fillBackend, events: fillEvents } = makeBackend()
    await settle(
      fillBackend.runClean({
        scope: 'chapter',
        chapterId: CHAPTER,
        bubbleEngine: 'fill',
        outsideEngine: 'redraw',
      }),
    )
    const bubbleFill = fillEvents
      .filter((e) => e.type === 'region-done' && e.region.kind === 'bubble')
      .map((e) => e.region.mask.provenance.engine)
    const outsideRedraw = fillEvents
      .filter((e) => e.type === 'region-done' && e.region.kind !== 'bubble')
      .map((e) => e.region.mask.provenance.engine)
    expect(bubbleFill.length).toBeGreaterThan(0)
    expect(outsideRedraw.length).toBeGreaterThan(0)
    expect(bubbleFill.every((eng) => eng === 'fill')).toBe(true)
    expect(outsideRedraw.every((eng) => eng === 'lama')).toBe(true)

    const { backend: redrawBackend, events: redrawEvents } = makeBackend()
    await settle(
      redrawBackend.runClean({
        scope: 'chapter',
        chapterId: CHAPTER,
        bubbleEngine: 'redraw',
        outsideEngine: 'fill',
      }),
    )
    const bubbleRedraw = redrawEvents
      .filter((e) => e.type === 'region-done' && e.region.kind === 'bubble')
      .map((e) => e.region.mask.provenance.engine)
    const outsideFill = redrawEvents
      .filter((e) => e.type === 'region-done' && e.region.kind !== 'bubble')
      .map((e) => e.region.mask.provenance.engine)
    expect(bubbleRedraw.length).toBeGreaterThan(0)
    expect(outsideFill.length).toBeGreaterThan(0)
    expect(bubbleRedraw.every((eng) => eng === 'lama')).toBe(true)
    expect(outsideFill.every((eng) => eng === 'fill')).toBe(true)
  })

  it('cleans gate-skipped outside-bubble regions only when the run opts in', async () => {
    // Wandering Moon Ch. 12 carries the fixture's one "text outside a speech
    // bubble" region, on a page the auto pass has otherwise finished.
    const WANDERING = 'wandering-moon-ch12'
    const outsideHeld = (region) =>
      region.outcome === 'gate-skipped' && region.gateSkipCause === 'outside-bubble'
    const regionsOf = async (backend) => {
      const projects = await settle(backend.listProjects())
      const chapter = projects.flatMap((p) => p.chapters).find((c) => c.id === WANDERING)
      return (await pagedIn(backend, chapter, settle)).flatMap((page) => page.regions)
    }

    // Default: the region stays held, and no region-done event names it.
    const { backend: held, events: heldEvents } = makeBackend()
    await settle(held.runClean({ scope: 'chapter', chapterId: WANDERING, outsideEngine: 'lama' }))
    expect(
      heldEvents.filter((e) => e.type === 'region-done' && e.region.gateSkipCause === 'outside-bubble'),
    ).toEqual([])
    const stillHeld = (await regionsOf(held)).filter(outsideHeld)
    expect(stillHeld.length).toBeGreaterThan(0)

    // Opted in: the same region is cleaned, on the outside-text engine, and
    // is no longer a gate skip.
    const { backend: opted, events: optedEvents } = makeBackend()
    await settle(
      opted.runClean({
        scope: 'chapter',
        chapterId: WANDERING,
        outsideEngine: 'lama',
        outsideBubbles: 'clean',
      }),
    )
    const after = await regionsOf(opted)
    expect(after.filter(outsideHeld)).toEqual([])
    const cleanedOutside = optedEvents
      .filter((e) => e.type === 'region-done' && e.region.kind === 'outside')
      .map((e) => e.region.mask.provenance.engine)
    expect(cleanedOutside.length).toBeGreaterThanOrEqual(stillHeld.length)
    expect(cleanedOutside.every((engine) => engine === 'lama')).toBe(true)
  })

  it('reports the empty result when a chapter has no regions at all', async () => {
    const { backend, events } = makeBackend()

    await settle(backend.runClean({ scope: 'chapter', chapterId: EMPTY_CHAPTER }))

    expect(events.filter((e) => e.type === 'region-done')).toHaveLength(0)
    expect(events.filter((e) => e.type === 'page-done')).toHaveLength(48)

    const notices = events.filter((e) => e.type === 'notice')
    expect(notices).toHaveLength(1)
    expect(notices[0]).toMatchObject({
      key: 'notice.chapter.emptyResult',
      params: { regions: 0, pages: 48 },
      tone: 'warn',
    })
    expect(notices.some((e) => e.key === 'notice.run.finished')).toBe(false)
  })
})

/**
 * `exportChapter` refuses nine different things here and used to refuse one.
 * The mock is the only backend outside a Tauri window, so a refusal it does
 * not produce is a refusal the interface is never seen handling.
 * `src-tauri/src/exporting.rs` is the side that decides them; these assert the
 * mock decides the same ones, in the same shape, and names each one's own
 * reason rather than the overwrite refusal it grew out of. The two it cannot
 * decide - a PSD of a page PSD cannot carry - need per-source mode and depth
 * the mock's pages do not have.
 */
describe('export refusals', () => {
  /** A paginated chapter, and a longstrip one, both from the fixture library. */
  const LONGSTRIP = 'neon-alley-ch4'

  /** The refusal, and the notice it announced, from one call. */
  async function exportWith(spec) {
    const { backend, events } = makeBackend()
    const result = await settle(backend.exportChapter({ chapterId: CHAPTER, ...spec }))
    const notices = events.filter((e) => e.type === 'notice')
    return { result, notices }
  }

  it('names the format it cannot write rather than substituting one', async () => {
    const cases = [
      ['JPEG', 'notice.export.refusedLossyFormat'],
      ['jpg', 'notice.export.refusedLossyFormat'],
      ['WebP', 'notice.export.refusedLossyFormat'],
      ['PSB', 'notice.export.refusedLayeredFormat'],
      ['AVIF', 'notice.export.refusedUnknownFormat'],
    ]
    for (const [format, reasonKey] of cases) {
      const { result, notices } = await exportWith({ format })
      expect(result, format).toMatchObject({ status: 'refused', reasonKey })
      expect(result.fileCount, format).toBeUndefined()
      // The defect this replaced: a JPEG that came back as a PNG, reported as
      // a success. Nothing about this answer says the export happened.
      expect(notices.map((e) => e.key), format).toEqual([reasonKey])
      expect(notices[0].params.format, format).toBe(format)
    }
  })

  it('writes PNG, TIFF, PSD and CBZ, and puts the archive in a file of its own', async () => {
    for (const format of ['PNG', 'TIFF', 'PSD']) {
      const { result } = await exportWith({ format })
      expect(result, format).toMatchObject({ status: 'exported' })
      expect(result.path.endsWith('.cbz'), format).toBe(false)
    }
    const { result } = await exportWith({ format: 'CBZ' })
    expect(result.status).toBe('exported')
    expect(result.path.endsWith('.cbz')).toBe(true)
    expect(result.fileCount).toBeGreaterThan(0)
  })

  it('writes separate masks for a raster page or a PSD, and refuses them inside an archive', async () => {
    for (const format of ['PNG', 'TIFF', 'PSD']) {
      const { result } = await exportWith({ format, masks: 'separate-layer' })
      expect(result, format).toMatchObject({ status: 'exported' })
    }
    const { result, notices } = await exportWith({ format: 'CBZ', masks: 'separate-layer' })
    expect(result).toMatchObject({
      status: 'refused',
      reasonKey: 'notice.export.refusedMaskLayers',
    })
    expect(notices.map((e) => e.key)).toEqual(['notice.export.refusedMaskLayers'])
  })

  it('refuses to stitch a chapter into one PSD', async () => {
    const { backend } = makeBackend()
    const result = await settle(
      backend.exportChapter({ chapterId: LONGSTRIP, format: 'PSD', layout: 'stitched' }),
    )
    expect(result).toMatchObject({
      status: 'refused',
      reasonKey: 'notice.export.refusedStitchedLayered',
    })
  })

  it('takes an absolute destination and refuses a relative one', async () => {
    const chosen = await exportWith({ destination: '/Volumes/scratch/ch107' })
    expect(chosen.result.status).toBe('exported')
    expect(chosen.result.path).toBe('/Volumes/scratch/ch107')

    const relative = await exportWith({ destination: '../up/one' })
    expect(relative.result).toMatchObject({
      status: 'refused',
      reasonKey: 'notice.export.refusedDestination',
    })
  })

  it('still refuses the source folder, and says which folder', async () => {
    const { result, notices } = await exportWith({ destination: 'source-folder' })
    expect(result).toMatchObject({
      status: 'refused',
      reasonKey: 'notice.export.refusedOverwrite',
    })
    expect(notices[0].params.path).toBeTruthy()
  })

  it('stitches a longstrip chapter into one file and counts its gutter', async () => {
    const { backend, events } = makeBackend()
    const result = await settle(
      backend.exportChapter({ chapterId: LONGSTRIP, format: 'PNG', layout: 'stitched' }),
    )
    expect(result.status).toBe('exported')
    expect(result.fileCount).toBe(1)
    expect(result.path.endsWith('.png')).toBe(true)
    // The count is reported, and zero is a real answer - every
    // fixture page is the full width of the strip.
    expect(result.gutterPixels).toBe(0)
    const notices = events.filter((e) => e.type === 'notice')
    expect(notices.map((e) => e.key)).toEqual(['notice.export.stitched'])
    expect(notices[0].params.count).toBe(0)
  })

  it('refuses to stitch a paginated chapter, or to stitch into an archive', async () => {
    const paginated = await exportWith({ layout: 'stitched' })
    expect(paginated.result).toMatchObject({
      status: 'refused',
      reasonKey: 'notice.export.refusedStitchPaginated',
    })

    const { backend } = makeBackend()
    const archive = await settle(
      backend.exportChapter({ chapterId: LONGSTRIP, format: 'CBZ', layout: 'stitched' }),
    )
    expect(archive).toMatchObject({
      status: 'refused',
      reasonKey: 'notice.export.refusedStitchedArchive',
    })
  })
})

describe('cleanAnyway', () => {
  it('cleans a skipped region with the specified engine and defaults to fill', async () => {
    const { backend } = makeBackend()
    const opened = await settle(
      backend.openChapter({ projectId: 'tsuki-to-hane', chapterId: CHAPTER }),
    )
    const loaded = await pagedIn(backend, opened.chapter, settle)
    const page = loaded[0]
    const region = page.regions[0]

    const resultWithEngine = await settle(
      backend.cleanAnyway({ regionId: region.id, engine: 'lama' }),
    )
    expect(resultWithEngine).not.toBeNull()
    expect(resultWithEngine.mask.provenance.engine).toBe('lama')
    expect(resultWithEngine.region.outcome).toBe('cleaned')

    const region2 = page.regions[1]
    const resultDefault = await settle(backend.cleanAnyway({ regionId: region2.id }))
    expect(resultDefault).not.toBeNull()
    expect(resultDefault.mask.provenance.engine).toBe('fill')
    expect(resultDefault.region.outcome).toBe('cleaned')
  })
})

describe('paint and clone engine integration in mock backend', () => {
  it('creates a region with engine paint when brush mode is paint', async () => {
    const { backend } = makeBackend()
    const opened = await settle(
      backend.openChapter({ projectId: 'tsuki-to-hane', chapterId: CHAPTER }),
    )
    const result = await settle(
      backend.createRegion({
        chapterId: opened.chapter.id,
        pageIndex: 0,
        bbox: { x: 10, y: 10, w: 20, h: 20 },
        tool: 'brush',
        params: {
          mode: 'paint',
          color: '#123456',
          opacity: 100,
          flow: 100,
          stroke: { points: [{ x: 10, y: 10 }], radius: 10 },
          paint: {
            points: [{ x: 10, y: 10, p: 0.5 }],
            color: '#123456',
            opacity: 100,
            flow: 100,
            hardness: 70,
            spacing: 12,
            pressureSize: true,
            pressureOpacity: false,
            seed: 42,
          },
        },
      }),
    )
    expect(result).not.toBeNull()
    expect(result.region.tool).toBe('brush')
    expect(result.region.mask?.provenance.engine).toBe('paint')
  })

  it('creates a region with engine clone for cloneHeal tool', async () => {
    const { backend } = makeBackend()
    const opened = await settle(
      backend.openChapter({ projectId: 'tsuki-to-hane', chapterId: CHAPTER }),
    )
    const result = await settle(
      backend.createRegion({
        chapterId: opened.chapter.id,
        pageIndex: 0,
        bbox: { x: 10, y: 10, w: 20, h: 20 },
        tool: 'cloneHeal',
        params: {
          mode: 'heal',
          alignment: 'aligned',
          opacity: 90,
          flow: 80,
          stroke: { points: [{ x: 10, y: 10 }], radius: 10 },
          cloneSource: { x: 50, y: 50 },
          cloneOffset: { x: 40, y: 40 },
          paint: {
            points: [{ x: 10, y: 10, p: 0.5 }],
            seed: 99,
          },
        },
      }),
    )
    expect(result).not.toBeNull()
    expect(result.region.tool).toBe('cloneHeal')
    expect(result.region.mask?.provenance.engine).toBe('clone')
  })
})

/**
 * The model catalogue. What is asserted is the
 * *shape* the interface reads and the one rule the mock exists to make
 * visible in a browser - one artefact missing, so the engine gating is
 * something a person can see rather than something only a Tauri window with a
 * half-empty models directory ever shows.
 */
describe('the model catalogue', () => {
  it('reports every artefact, with exactly one of them not installed', async () => {
    const { backend } = makeBackend()
    const view = await settle(backend.listModels())

    expect(view.models).toHaveLength(8)
    for (const model of view.models) {
      expect(model.kindKey).toMatch(/^models\.kind\./)
      expect(model.bytes).toBeGreaterThan(0)
      expect(Array.isArray(model.requiredBy)).toBe(true)
    }
    // The redraw model, which is what makes the engine gating visible, and the
    // rescue reader's three parts, which gate nothing and are absent because
    // that is what a machine that has not chosen to fetch 460 MB looks like.
    const missing = view.models.filter((model) => !model.installed).map((model) => model.id)
    expect(missing).toEqual(['inpainter', 'ocrEncoder', 'ocrDecoder', 'ocrVocab'])
    expect(view.runtime.installed).toBe(true)
    expect(view.modelsDir).toBeTruthy()
    // The token never comes back across the seam - only whether one is stored
    // and where it is kept.
    expect(view).not.toHaveProperty('hfToken')
    expect(view.hasToken).toBe(false)
    expect(view.tokenStore).toBe('fileNoStore')
  })

  it('reports a download on the event channel and ends with exactly one done', async () => {
    const { backend, events } = makeBackend()
    await settle(backend.downloadModel({ id: 'inpainter' }))

    const progress = events.filter((event) => event.type === 'model-progress')
    expect(progress.length).toBeGreaterThan(2)
    expect(progress[0]).toMatchObject({ id: 'inpainter', downloaded: 0, done: false })
    expect(progress.filter((event) => event.done)).toHaveLength(1)
    expect(progress.at(-1)).toMatchObject({ done: true, error: null })

    // The one row that was missing and gates something is now installed. The
    // reader's three rows are still absent - they were not what was downloaded
    // - which is exactly the state a real machine sits in.
    const view = await settle(backend.listModels())
    expect(view.models.find((model) => model.id === 'inpainter')?.installed).toBe(true)
    const missing = view.models.filter((model) => !model.installed).map((model) => model.id)
    expect(missing).toEqual(['ocrEncoder', 'ocrDecoder', 'ocrVocab'])
  })

  it('ends a cancelled download the same way a failed one ends', async () => {
    const { backend, events } = makeBackend()
    await begin(backend.downloadModel({ id: 'inpainter' }))
    await begin(backend.cancelDownload({ id: 'inpainter' }))

    const done = events.filter((event) => event.type === 'model-progress' && event.done)
    expect(done).toHaveLength(1)
    expect(done[0].error).toBe('cancelled')
    // And the row is where it started, not half-installed.
    const view = await settle(backend.listModels())
    expect(view.models.find((model) => model.id === 'inpainter').installed).toBe(false)
  })

  /**
   * What a stopped download leaves, said out loud. The bytes
   * are kept so the next press can resume from them, which makes them worth
   * reporting and worth being able to give back.
   */
  it('reports what a cancelled download left, and discards it on request', async () => {
    const { backend } = makeBackend()
    await begin(backend.downloadModel({ id: 'inpainter' }))
    await begin(backend.cancelDownload({ id: 'inpainter' }))

    const stopped = await settle(backend.listModels())
    const row = stopped.models.find((model) => model.id === 'inpainter')
    expect(row.installed).toBe(false)
    expect(row.partialBytes).toBeGreaterThan(0)
    expect(row.partialBytes).toBeLessThan(row.bytes)
    // Every other row has nothing beside it, which is the ordinary state.
    expect(stopped.models.filter((model) => model.partialBytes !== null)).toHaveLength(1)

    expect(await settle(backend.discardPartial({ id: 'inpainter' }))).toBe(true)
    const after = await settle(backend.listModels())
    expect(after.models.find((model) => model.id === 'inpainter').partialBytes).toBe(null)
    // And a second press has nothing to throw away, which is not a failure.
    expect(await settle(backend.discardPartial({ id: 'inpainter' }))).toBe(false)
  })

  it('resumes from the bytes it kept rather than starting again', async () => {
    const { backend, events } = makeBackend()
    await begin(backend.downloadModel({ id: 'inpainter' }))
    await begin(backend.cancelDownload({ id: 'inpainter' }))
    const kept = (await settle(backend.listModels())).models.find(
      (model) => model.id === 'inpainter',
    ).partialBytes

    events.length = 0
    await begin(backend.downloadModel({ id: 'inpainter' }))
    // The first event of a resumed transfer carries the prefix, so a bar
    // picking it back up starts where it stopped.
    expect(events[0]).toMatchObject({ type: 'model-progress', downloaded: kept, done: false })
    await begin(backend.cancelDownload({ id: 'inpainter' }))
  })

  it('will not discard the bytes a running download is writing into', async () => {
    const { backend } = makeBackend()
    await begin(backend.downloadModel({ id: 'inpainter' }))
    // Another window's press against a row drawn before this transfer started.
    // Deleting the `.part` under a writer is the one thing the press must not
    // do, so it does nothing and says so.
    expect(await begin(backend.discardPartial({ id: 'inpainter' }))).toBe(false)
    await begin(backend.cancelDownload({ id: 'inpainter' }))
    expect(await begin(backend.discardPartial({ id: 'inpainter' }))).toBe(true)
  })

  it('refuses to delete the runtime while it is downloading', async () => {
    const { backend } = makeBackend()
    await begin(backend.downloadRuntime())
    // The native command removes the whole runtimes tree, and a transfer's
    // `.part` is inside it.
    expect(await settle(backend.deleteRuntime())).toBe('busy')

    await begin(backend.cancelDownload({ id: 'runtime' }))
    expect(await settle(backend.deleteRuntime())).toBe('deleted')
  })

  it('takes an installed weight away again, and says which of the three things happened', async () => {
    const { backend } = makeBackend()
    expect(await settle(backend.deleteModel({ id: 'textDetector' }))).toBe('deleted')
    // Twice is not an error and it is not a silence either: the second press is
    // a stale row, and the answer says so.
    expect(await settle(backend.deleteModel({ id: 'textDetector' }))).toBe('notFound')
    const view = await settle(backend.listModels())
    expect(view.models.find((model) => model.id === 'textDetector').installed).toBe(false)
  })

  /**
   * The two refusals which are what a second window's stale
   * row gets when it presses Download: one already running, one already here.
   */
  it('says why a download did not start', async () => {
    const { backend } = makeBackend()
    // Every row but the redraw model is installed in the mock, so this is the
    // already-installed answer without arranging anything.
    expect(await settle(backend.downloadModel({ id: 'textDetector' }))).toBe('alreadyInstalled')

    expect(await begin(backend.downloadModel({ id: 'inpainter' }))).toBe('started')
    expect(await begin(backend.downloadModel({ id: 'inpainter' }))).toBe('alreadyRunning')
    await begin(backend.cancelDownload({ id: 'inpainter' }))
  })

  /**
   * The runtime is an archive rather than a weight and its row says what its
   * download costs before anything is fetched. The mock reports
   * one flavour because macOS publishes one, which is why the picker is not
   * drawn in a browser.
   */
  it('reports the runtime download size and the builds there are to choose from', async () => {
    const { backend } = makeBackend()
    const { runtime } = await settle(backend.listModels())

    expect(runtime.bytes).toBeGreaterThan(0)
    expect(runtime.flavours).toHaveLength(1)
    expect(runtime.flavours[0]).toMatchObject({ id: 'stock', isDefault: true, userInstalled: [] })
    expect(runtime.flavour).toBe(runtime.flavours[0].id)
    expect(runtime.bytes).toBe(runtime.flavours[0].bytes)

    // What is *installed* is a second question the row now answers.
    // On this machine the two agree, which is why Settings draws no
    // "installed X, chosen Y" line in a browser.
    expect(runtime.installedFlavour).toBe(runtime.flavour)
    expect(runtime.installedVersion).toBe(runtime.version)
  })

  /**
   * An uninstalled runtime has no build to report, and `null` means *unknown*
   * rather than a claim about one - the same thing the native side answers for
   * a library it did not unpack itself.
   */
  it('says nothing about the installed build when there is no runtime', async () => {
    const { backend } = makeBackend()
    expect(await settle(backend.deleteRuntime())).toBe('deleted')

    const { runtime } = await settle(backend.listModels())
    expect(runtime.installed).toBe(false)
    expect(runtime.installedFlavour).toBe(null)
    expect(runtime.installedVersion).toBe(null)
    // The chosen build is still named: it is what the Download button fetches.
    expect(runtime.flavour).toBe('stock')
  })

  /**
   * The seam's one argument to `listModels`. It asks the native
   * side to offer the token to a credential store that refused this process
   * once, which a browser has none of - so what is asserted here is that the
   * mock takes the argument and answers the same view, rather than rejecting a
   * call the dialog makes on every open.
   */
  it('accepts the credential-store retry a dialog open asks for', async () => {
    const { backend } = makeBackend()
    const plain = await settle(backend.listModels())
    const retried = await settle(backend.listModels({ retryStore: true }))

    expect(retried).toEqual(plain)
    expect(retried.tokenStore).toBe('fileNoStore')
    // No unreachable store, so no reason to give for one.
    expect(retried.tokenStoreReason).toBe(null)
  })

  it('stores a token without ever handing it back', async () => {
    const { backend } = makeBackend()
    const written = await settle(backend.writeSettings({ hfToken: 'hf_secret' }))
    const view = await settle(backend.listModels())
    expect(view.hasToken).toBe(true)
    expect(JSON.stringify(view)).not.toContain('hf_secret')

    // Not through the settings snapshot either, which is the other direction
    // the same value could have come back in.
    expect(written).not.toHaveProperty('hfToken')
    const read = await settle(backend.readSettings())
    expect(read).not.toHaveProperty('hfToken')
    expect(JSON.stringify(read)).not.toContain('hf_secret')
  })
})
