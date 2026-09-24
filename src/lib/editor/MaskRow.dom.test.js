/**
 * A Layers row's engine picker and the cloud: Cloud is offered last while a
 * cloud endpoint is ready and allowed, and left out otherwise; a mask the
 * cloud rendered reads as Cloud; and a pick that changes nothing, a cloud
 * consent cancelled, leaves the picker on the engine the mask has.
 *
 * Against the browser mock with every delay zero, the chapter open on a
 * region with a local mask. The consent dialog is answered through the modal
 * stack, which is what its buttons do.
 */
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/svelte'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { setBackend } from '../api/backend.js'
import { createMockBackend } from '../api/mock.js'
import { t } from '../i18n/index.js'
import { app, clearNotices, closeAllModals, closeModal } from '../state/app.svelte.js'
import { refreshCloudReadiness, stopCloud } from '../state/cloud.svelte.js'
import { editor } from '../state/editor.svelte.js'
import { setCloudAllowed } from '../state/session.svelte.js'
import MaskRow from './MaskRow.svelte'
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
          cloud: { provider: 'modal', profile_id: PROFILE.id, request_id: 'req-1', model: 'm', cost: null },
        },
      },
    }
    show(cloudPatch)
    expect(screen.getByText(t('ladder.rung.cloud'), { selector: '.title' })).toBeTruthy()
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
