/**
 * The Layers panel's ways into the cloud, and the one that stays local, against
 * the browser mock with every delay zero.
 *
 * Clean with > Cloud and Try again on a mask the cloud rendered each put the
 * consent dialog up before anything is sent, and each asks again: a grant is
 * for one render. Clean anyway never goes to the cloud. The dialog is answered
 * through the modal stack, which is what its buttons do.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { setBackend } from '../api/backend.js'
import { createMockBackend } from '../api/mock.js'
import { CLOUD_ENGINE, isCloudMask } from '../model/masks.js'
import { app, clearNotices, closeAllModals, closeModal } from '../state/app.svelte.js'
import { stopCloud } from '../state/cloud.svelte.js'
import { editor } from '../state/editor.svelte.js'
import { setCloudAllowed } from '../state/session.svelte.js'
import { cleanAnyway, rerunMask, runRegionMenuItem } from './maskactions.svelte.js'

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
 * A mock with cloud allowed and one Modal endpoint as the default, and the
 * chapter open on a region that has a local Fill mask.
 */
async function opened() {
  const backend = createMockBackend({ timing: ZERO })
  await backend.writeSettings({ cloudEngines: 'allowed' })
  setCloudAllowed(true)
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

  const indices = [0, 1, 2, 3, 4, 5]
  const first = await backend.loadPages({ chapterId: CHAPTER, indices })
  const page = /** @type {any} */ (first.find((candidate) => candidate.regions.length > 0))
  const regionId = page.regions[0].id
  const local = await backend.applyTool({
    tool: 'contentAwareFill',
    params: { engine: 'local' },
    chapterId: CHAPTER,
    pageIndex: page.index,
    regionId,
  })
  expect(local.status).toBe('applied')
  editor.chapter = /** @type {any} */ ({
    id: CHAPTER,
    review: [],
    pages: await backend.loadPages({ chapterId: CHAPTER, indices }),
  })
  return { backend, regionId }
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

/**
 * Wait for the consent dialog, the only one up, then answer it.
 *
 * @param {'confirm'|'cancel'} action
 */
async function answerConsent(action) {
  await vi.waitFor(() => expect(app.modals.map((modal) => modal.kind)).toEqual(['cloudConsent']))
  closeModal(action)
}

describe('the cloud from the Layers panel', () => {
  it('cleans with Cloud only after consent, and lands the render as one undoable edit', async () => {
    const { backend, regionId } = await opened()
    const prepare = vi.spyOn(backend, 'prepareCloudConsent')
    const rerun = vi.spyOn(backend, 'rerunMask')
    const before = current(regionId)
    const maskId = before.mask.id
    expect(isCloudMask(before.mask)).toBe(false)
    const undoable = editor.history.entries.length

    const done = runRegionMenuItem(`engine:${CLOUD_ENGINE}`, before)
    await answerConsent('confirm')
    expect(await done).toBe(true)

    expect(prepare).toHaveBeenCalledTimes(1)
    expect(prepare.mock.calls[0][0]).toMatchObject({
      regionId,
      chapterId: CHAPTER,
      intent: { action: 'rerunMask', mask_id: maskId, kind: 'engine', engine: CLOUD_ENGINE },
    })
    expect(rerun).toHaveBeenCalledTimes(1)
    expect(rerun.mock.calls[0][0]).toMatchObject({
      maskId,
      kind: 'engine',
      engine: CLOUD_ENGINE,
      params: { grantNonce: expect.stringMatching(/^grant-/) },
    })

    const after = current(regionId)
    expect(isCloudMask(after.mask)).toBe(true)
    expect(after.mask.provenance.cloud).toMatchObject({ provider: 'modal', profile_id: PROFILE.id })
    // A cloud render runs the recipe a local FLUX run does, so it is not held
    // for review on that account.
    expect(editor.chapter?.review.map((entry) => entry.id)).not.toContain(regionId)
    expect(editor.history.entries).toHaveLength(undoable + 1)
    expect(app.notices.map((notice) => notice.key)).toContain('notice.cloud.finished')
  })

  it('asks again for Try again on a cloud mask, and renders it in the cloud again', async () => {
    const { backend, regionId } = await opened()
    const first = runRegionMenuItem(`engine:${CLOUD_ENGINE}`, current(regionId))
    await answerConsent('confirm')
    expect(await first).toBe(true)
    const rendered = current(regionId)
    expect(isCloudMask(rendered.mask)).toBe(true)

    const prepare = vi.spyOn(backend, 'prepareCloudConsent')
    const rerun = vi.spyOn(backend, 'rerunMask')
    const again = rerunMask(rendered, 'retry')
    // Nothing goes out before the answer.
    await answerConsent('confirm')
    expect(await again).toBe(true)

    expect(prepare).toHaveBeenCalledTimes(1)
    expect(prepare.mock.calls[0][0].intent).toEqual({
      action: 'rerunMask',
      mask_id: rendered.mask.id,
      kind: 'engine',
      engine: CLOUD_ENGINE,
    })
    expect(rerun).toHaveBeenCalledTimes(1)
    expect(rerun.mock.calls[0][0]).toMatchObject({ kind: 'engine', engine: CLOUD_ENGINE })
    // Never a grantless retry, which the native side would refuse.
    expect(rerun.mock.calls[0][0].params.grantNonce).toEqual(expect.stringMatching(/^grant-/))
    const after = current(regionId)
    expect(isCloudMask(after.mask)).toBe(true)
    expect(after.mask.provenance.cloud.attempt_id).not.toBe(rendered.mask.provenance.cloud.attempt_id)
  })

  it('sends nothing and changes nothing when the consent is cancelled', async () => {
    const { backend, regionId } = await opened()
    const confirm = vi.spyOn(backend, 'confirmCloudConsent')
    const rerun = vi.spyOn(backend, 'rerunMask')
    const before = current(regionId)
    const undoable = editor.history.entries.length

    const done = rerunMask(before, 'engine', CLOUD_ENGINE)
    await answerConsent('cancel')
    expect(await done).toBe(false)

    expect(confirm).not.toHaveBeenCalled()
    expect(rerun).not.toHaveBeenCalled()
    expect(current(regionId).mask.id).toBe(before.mask.id)
    expect(editor.history.entries).toHaveLength(undoable)
  })

  it('re-runs a local mask locally, with no dialog', async () => {
    const { backend, regionId } = await opened()
    const prepare = vi.spyOn(backend, 'prepareCloudConsent')
    const rerun = vi.spyOn(backend, 'rerunMask')
    const before = current(regionId)

    expect(await rerunMask(before, 'retry')).toBe(true)
    expect(app.modals).toHaveLength(0)
    expect(prepare).not.toHaveBeenCalled()
    expect(rerun).toHaveBeenCalledWith({ maskId: before.mask.id, kind: 'retry', engine: undefined })
    expect(isCloudMask(current(regionId).mask)).toBe(false)
  })

  it('keeps Clean anyway local', async () => {
    const { backend } = await opened()
    const prepare = vi.spyOn(backend, 'prepareCloudConsent')
    const clean = vi.spyOn(backend, 'cleanAnyway').mockResolvedValue(null)
    const region = /** @type {any} */ ({ id: 'skipped-1', gateSkipCause: 'outside-bubble', mask: null })

    expect(await cleanAnyway(region)).toBe(false)
    expect(app.modals).toHaveLength(0)
    expect(prepare).not.toHaveBeenCalled()
    expect(clean).toHaveBeenCalledTimes(1)
    const [request] = clean.mock.calls[0]
    expect(Object.keys(request).sort()).toEqual(['engine', 'regionId'])
    expect(request.engine).not.toBe(CLOUD_ENGINE)
  })
})
