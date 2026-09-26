/**
 * A Layers row's engine picker and the cloud: Cloud is offered last while a
 * cloud endpoint is ready and allowed, and left out otherwise; a mask the
 * cloud rendered reads as Cloud; and a pick that changes nothing, a cloud
 * consent cancelled, leaves the picker on the engine the mask has.
 *
 * Then what the row says about Try again - that it replaces the layer, where a
 * new stroke refines it; that it is off for a paint or clone stroke, and why -
 * and the three answers a layer in Needs review offers.
 *
 * Against the browser mock with every delay zero, the chapter open on a
 * region with a local mask. The consent dialog is answered through the modal
 * stack, which is what its buttons do. The editor's undo and the two mask
 * actions the review controls call are spies over the real modules, so the
 * picker's cases still run through the real re-run.
 */
import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/svelte'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

const undoState = vi.hoisted(() => ({ offered: false }))

vi.mock('../state/editor.svelte.js', async (importOriginal) => {
  const actual = /** @type {any} */ (await importOriginal())
  return {
    ...actual,
    undo: vi.fn(),
    undoAvailable: vi.fn(() => undoState.offered),
    undoLabelKey: vi.fn(() => (undoState.offered ? 'masks.command.deleteMask' : null)),
  }
})

vi.mock('./maskactions.svelte.js', async (importOriginal) => {
  const actual = /** @type {any} */ (await importOriginal())
  return {
    ...actual,
    keepDependencyResult: vi.fn(actual.keepDependencyResult),
    rerunMask: vi.fn(actual.rerunMask),
  }
})

import { setBackend } from '../api/backend.js'
import { createMockBackend } from '../api/mock.js'
import { t } from '../i18n/index.js'
import { app, clearNotices, closeAllModals, closeModal } from '../state/app.svelte.js'
import { refreshCloudReadiness, stopCloud } from '../state/cloud.svelte.js'
import { editor, undo } from '../state/editor.svelte.js'
import { setCloudAllowed } from '../state/session.svelte.js'
import MaskRow from './MaskRow.svelte'
import { keepDependencyResult, rerunMask } from './maskactions.svelte.js'
import { maskRow } from './maskrows.js'

const ZERO = { method: 0, openChapter: 0, export: 0, cloud: 0, provision: 0, region: 0, pageTail: 0, noticeStagger: 0 }
const CHAPTER = 'tsuki-to-hane-ch107'
const PROFILE = {
  id: 'mc-abc123',
  name: 'Studio GPU',
  endpointUrl: 'https://ws--mc-abc123-gateway.modal.run/mc/v1',
}

/** Local storage for the test: the node running the suite has none. */
function memoryStorage() {
  /** @type {Map<string, string>} */
  const values = new Map()
  return {
    get length() {
      return values.size
    },
    key: (/** @type {number} */ index) => [...values.keys()][index] ?? null,
    getItem: (/** @type {string} */ key) => values.get(key) ?? null,
    setItem: (/** @type {string} */ key, /** @type {string} */ value) => void values.set(key, String(value)),
    removeItem: (/** @type {string} */ key) => void values.delete(key),
    clear: () => values.clear(),
  }
}

beforeEach(() => {
  vi.stubGlobal('localStorage', memoryStorage())
})

afterEach(() => {
  cleanup()
  closeAllModals()
  stopCloud()
  setCloudAllowed(false)
  clearNotices()
  editor.chapter = null
  setBackend(null)
  vi.unstubAllGlobals()
  vi.restoreAllMocks()
})

/**
 * The chapter open on a region with a local Fill mask, on a mock whose one
 * Modal endpoint is the default, with the cloud permission as given.
 *
 * @param {{allowed?: boolean}} [options]
 */
async function opened({ allowed = true } = {}) {
  const backend = createMockBackend({ timing: ZERO })
  await backend.writeSettings({ cloudEngines: allowed ? 'allowed' : 'blocked' })
  setCloudAllowed(allowed)
  await backend.writeInferenceConfig({
    config: {
      schemaVersion: 1,
      selectedTarget: { type: 'modal', profile_id: PROFILE.id },
      beamProfiles: {},
      modalProfiles: {
        [PROFILE.id]: {
          ...PROFILE,
          canonicalOrigin: new URL(PROFILE.endpointUrl).origin,
          canonicalOriginFingerprint: '',
          createdAtMs: 1,
          updatedAtMs: 1,
        },
      },
    },
  })
  await backend.storeCloudSecret({
    provider: 'modal',
    profileId: PROFILE.id,
    role: 'runtime',
    secret: 'rt-secret',
    tokenId: 'rt-id',
  })
  setBackend(backend)
  await refreshCloudReadiness(backend)

  const indices = [0, 1, 2, 3, 4, 5]
  const first = await backend.loadPages({ chapterId: CHAPTER, indices })
  const page = /** @type {any} */ (first.find((candidate) => candidate.regions.length > 0))
  const regionId = page.regions[0].id
  await backend.applyTool({
    tool: 'contentAwareFill',
    params: { engine: 'local' },
    chapterId: CHAPTER,
    pageIndex: page.index,
    regionId,
  })
  editor.chapter = /** @type {any} */ ({
    id: CHAPTER,
    review: [],
    pages: await backend.loadPages({ chapterId: CHAPTER, indices }),
  })
  return { backend, region: current(regionId) }
}

/**
 * The region as the open chapter holds it now.
 *
 * @param {string} regionId
 * @returns {any}
 */
function current(regionId) {
  for (const page of editor.chapter?.pages ?? []) {
    const found = page.regions.find((candidate) => candidate.id === regionId)
    if (found) return found
  }
  return null
}

/** @param {any} region */
function show(region) {
  return render(MaskRow, { props: { row: maskRow(region, false), region, open: true, ontoggle: () => {} } })
}

const picker = () => /** @type {HTMLSelectElement} */ (screen.getByLabelText(t('masks.action.engine')))
const offered = () => Array.from(picker().options, (option) => option.value)

describe('the engine picker on a Layers row', () => {
  it('offers Cloud last while a cloud endpoint is ready and allowed', async () => {
    const { region } = await opened()
    show(region)
    expect(offered()).toEqual(['fill', 'denoise', 'lama', 'cloud'])
    expect(picker().value).toBe('fill')
  })

  it('leaves Cloud out while the cloud is off', async () => {
    const { region } = await opened({ allowed: false })
    show(region)
    expect(offered()).toEqual(['fill', 'denoise', 'lama'])
  })

  it('names a mask the cloud rendered Cloud, and still shows it selected while the cloud is off', async () => {
    const { region } = await opened({ allowed: false })
    const cloudPatch = {
      ...region,
      mask: {
        ...region.mask,
        provenance: {
          ...region.mask.provenance,
          engine: 'flux',
          cloud: { provider: 'modal', profile_id: PROFILE.id, request_id: 'req-1', model: 'Disty0/FLUX.2-klein-9B-SDNQ-4bit-dynamic-svd-r32', cost: null },
        },
      },
    }
    show(cloudPatch)
    expect(screen.getByText('☁ FLUX.2 Klein 9B · Cloud', { selector: '.title' })).toBeTruthy()
    expect(offered()).toEqual(['cloud', 'fill', 'denoise', 'lama'])
    expect(picker().value).toBe('cloud')
  })

  it('goes back to the engine the mask has when the cloud consent is cancelled', async () => {
    const { backend, region } = await opened()
    const rerun = vi.spyOn(backend, 'rerunMask')
    show(region)

    await fireEvent.change(picker(), { target: { value: 'cloud' } })
    // The pick shows while its consent is asked.
    expect(picker().value).toBe('cloud')
    await waitFor(() => expect(app.modals.map((modal) => modal.kind)).toEqual(['cloudConsent']))
    closeModal('cancel')

    await waitFor(() => expect(picker().value).toBe('fill'))
    expect(rerun).not.toHaveBeenCalled()
  })
})

/**
 * The region as the row would get it with its mask changed by `patch`.
 *
 * @param {any} region
 * @param {Record<string, unknown>} patch
 * @returns {any}
 */
function withMask(region, patch) {
  return { ...region, mask: { ...region.mask, ...patch } }
}

/** @param {any} region @param {string} engine @returns {any} */
function strokeBy(region, engine) {
  return withMask(region, { provenance: { ...region.mask.provenance, engine } })
}

/**
 * The row's own focusable controls, in tab order: native buttons and selects
 * that are neither disabled nor taken out of the order.
 *
 * @returns {HTMLElement[]}
 */
function tabStops() {
  const row = /** @type {HTMLElement} */ (document.querySelector('[data-mask-row]'))
  return /** @type {HTMLElement[]} */ (Array.from(row.querySelectorAll('button, select')))
    .filter((element) => !(/** @type {any} */ (element).disabled) && element.tabIndex >= 0)
}

/**
 * Activate a focused native button from the keyboard the way a browser does:
 * Enter on keydown, Space on keyup, each followed by the click the browser
 * synthesises unless a handler cancelled the key. jsdom synthesises no click,
 * so without this a test that only sends the key never reaches the button's
 * click path at all.
 *
 * @param {HTMLElement} element
 * @param {'Enter'|' '} key
 */
async function pressKey(element, key) {
  const init = { key, code: key === ' ' ? 'Space' : 'Enter', bubbles: true, cancelable: true }
  const down = await fireEvent.keyDown(element, init)
  const up = await fireEvent.keyUp(element, init)
  const activates = key === 'Enter' ? down : down && up
  if (activates) await fireEvent.click(element)
}

describe('what a row says about Try again', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    undoState.offered = false
  })

  it('says Try again replaces this layer, where a new stroke refines it', async () => {
    const { region } = await opened({ allowed: false })
    show(region)
    const retry = screen.getByRole('button', { name: t('masks.action.retry') })
    expect(retry.getAttribute('title')).toBe(t('masks.action.retryHint'))
    expect(retry.hasAttribute('aria-disabled')).toBe(false)
    expect(picker().getAttribute('title')).toBe(t('masks.action.engineHint'))
    // Visible under the picker, so it is read without a hover.
    expect(screen.getByText(t('masks.action.rerunNote'))).toBeTruthy()

    await fireEvent.click(retry)
    expect(rerunMask).toHaveBeenCalledWith(region, 'retry')

    // The keyboard reaches the same press, which is what makes the blocked
    // case below a real check rather than a key nothing listens to.
    vi.mocked(rerunMask).mockClear()
    retry.focus()
    await pressKey(retry, 'Enter')
    await pressKey(retry, ' ')
    expect(rerunMask).toHaveBeenCalledTimes(2)
    expect(rerunMask).toHaveBeenNthCalledWith(2, region, 'retry')
  })

  for (const engine of ['paint', 'clone']) {
    it(`keeps Try again in place but off for a ${engine} stroke, and says why`, async () => {
      const { backend, region } = await opened({ allowed: false })
      const rerun = vi.spyOn(backend, 'rerunMask')
      const stroke = strokeBy(region, engine)
      show(stroke)

      const retry = screen.getByRole('button', { name: t('masks.action.retry') })
      expect(retry.getAttribute('aria-disabled')).toBe('true')
      expect(retry.getAttribute('title')).toBe(t('masks.action.retryBlocked'))
      // Still in the tab order, which a native `disabled` would take it out of,
      // so the reason can be reached from the keyboard.
      expect(/** @type {HTMLButtonElement} */ (retry).disabled).toBe(false)
      expect(tabStops()).toContain(retry)
      retry.focus()
      expect(document.activeElement).toBe(retry)

      // The same reason in the open row, where the picker would be; no picker,
      // since there is no engine to swap a stroke to.
      expect(screen.getByText(t('masks.action.retryBlocked'), { selector: 'p' })).toBeTruthy()
      expect(screen.queryByLabelText(t('masks.action.engine'))).toBe(null)
      expect(screen.queryByText(t('masks.action.rerunNote'))).toBe(null)

      await fireEvent.click(retry)
      expect(rerunMask).not.toHaveBeenCalled()
      expect(rerun).not.toHaveBeenCalled()

      // Reachable from the keyboard, and still nothing from it: Enter and
      // Space on the focused control run nothing, and focus stays put so
      // the reason is still what is read.
      await pressKey(retry, 'Enter')
      await pressKey(retry, ' ')
      expect(rerunMask).not.toHaveBeenCalled()
      expect(rerun).not.toHaveBeenCalled()
      expect(document.activeElement).toBe(retry)
      // Delete is still the live control beside it.
      expect(screen.getByRole('button', { name: t('masks.action.delete') }).hasAttribute('aria-disabled')).toBe(false)
    })
  }

  it('shows no Try again at all for an approved text-shape component', async () => {
    const { region } = await opened({ allowed: false })
    show({ ...region, id: 'c1-p001-hreview-sam-00001-deadbeef', mask: { ...region.mask, id: 'c1-p001-hreview-sam-00001-deadbeef-m1' } })
    expect(screen.queryByRole('button', { name: t('masks.action.retry') })).toBe(null)
    expect(screen.queryByText(t('masks.action.retryBlocked'))).toBe(null)
  })
})

describe('a row in Needs review', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    undoState.offered = true
  })

  afterEach(() => {
    undoState.offered = false
  })

  it('offers Keep, Rebuild and Undo, labelled, in the tab order, and each calls its own action', async () => {
    const { region } = await opened({ allowed: false })
    const flagged = withMask(region, { dependencyReview: 'changed' })
    show(flagged)

    const group = screen.getByRole('group', { name: t('masks.status.needsReview') })
    const keep = within(group).getByRole('button', { name: t('masks.dependency.keep') })
    const rebuild = within(group).getByRole('button', { name: t('masks.dependency.rebuild') })
    const undoButton = within(group).getByRole('button', { name: t('masks.dependency.undoLast') })

    // What each does, beyond its label. Rebuild is Try again and says so;
    // Undo names the action it would reverse, since it is the editor's undo
    // and not scoped to the edit that flagged this layer.
    expect(keep.getAttribute('title')).toBe(t('masks.dependency.keepHint'))
    expect(rebuild.getAttribute('title')).toBe(t('masks.action.retryHint'))
    expect(undoButton.getAttribute('title')).toBe(
      t('editor.action.undoCommand', { commandKey: 'masks.command.deleteMask' }),
    )

    // Native, enabled buttons, reached in this order by Tab.
    const stops = tabStops()
    const order = [keep, rebuild, undoButton].map((button) => stops.indexOf(button))
    expect(order.every((at) => at >= 0)).toBe(true)
    expect(order).toEqual([...order].sort((a, b) => a - b))
    expect(order[2] - order[0]).toBe(2)
    for (const button of [keep, rebuild, undoButton]) {
      button.focus()
      expect(document.activeElement).toBe(button)
    }

    vi.mocked(keepDependencyResult).mockResolvedValueOnce(true)
    await fireEvent.click(keep)
    expect(keepDependencyResult).toHaveBeenCalledTimes(1)
    expect(keepDependencyResult).toHaveBeenCalledWith(flagged)

    vi.mocked(rerunMask).mockResolvedValueOnce(false)
    await fireEvent.click(rebuild)
    expect(rerunMask).toHaveBeenCalledTimes(1)
    expect(rerunMask).toHaveBeenCalledWith(flagged, 'retry')

    await fireEvent.click(undoButton)
    expect(undo).toHaveBeenCalledTimes(1)
    // Each press is its own action and nothing else.
    expect(keepDependencyResult).toHaveBeenCalledTimes(1)
    expect(rerunMask).toHaveBeenCalledTimes(1)
  })

  it('leaves out Undo with nothing to undo, and Rebuild for a stroke that cannot be run again', async () => {
    undoState.offered = false
    const { region } = await opened({ allowed: false })
    show(withMask(strokeBy(region, 'paint'), { dependencyReview: 'unknown' }))

    const group = screen.getByRole('group', { name: t('masks.status.needsReview') })
    expect(within(group).getAllByRole('button').map((button) => button.textContent?.trim()))
      .toEqual([t('masks.dependency.keep')])
  })
})
