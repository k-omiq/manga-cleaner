/**
 * The Models section of the Settings dialog, mounted: the three things a row
 * says that nothing on the row used to say at all.
 *
 * - **What a stopped download left**, and the press that gives it back.
 *   The bytes are kept so the next Download
 *   resumes from them, which is exactly why they need reporting: 180 MB of a
 *   207 MB transfer nobody came back for is disk the user did not agree to
 *   spend, under a row reading "Not installed".
 * - **Which runtime build is actually installed**, said only when it is
 *   not the one the row names - two true statements that read as one false one
 *   when only the chosen build is on screen.
 * - **Why the credential store would not answer**, which the note under
 *   the token field could not say while the reason was prose in the platform's
 *   own words.
 *
 * And the one thing the dialog asks *for*: the credential-store retry, on the
 * open and on nothing else.
 *
 * A hand-written seam stub rather than the mock, for the reason
 * `SettingsDialog.dom.test.js` beside this file gives: the dialog opens six
 * calls on mount and only the catalogue is what this file is about, so the rest
 * answer the emptiest true thing.
 *
 * All of it lives in the **Models** tab, so `open()` presses that tab and
 * checks the panel is shown before anything is asserted inside it. The panels
 * are mounted and merely `hidden`, so a query that skipped the press would
 * still find the row - which is exactly why the press is asserted rather than
 * assumed.
 */

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render, waitFor } from '@testing-library/svelte'

import { setBackend } from '../api/backend.js'
import { t } from '../i18n/index.js'
import SettingsDialog from './SettingsDialog.svelte'

/** The spec `pushModal({kind: 'settings'})` would have handed the dialog. */
const SPEC = {
  id: 'modal-1',
  kind: 'settings',
  titleKey: 'modal.title.settings',
  props: {},
  actions: [{ id: 'close', labelKey: 'shell.action.close' }],
  blocking: false,
  dismissable: true,
  onresolve: null,
}

/** One weight and the runtime, with whatever this test is about set on them. */
function view({ model = {}, runtime = {}, token = {} } = {}) {
  return {
    models: [
      {
        id: 'inpainter',
        fileName: 'lama-manga.onnx',
        bytes: 207_482_644,
        kindKey: 'models.kind.inpainter',
        requiredBy: ['lama'],
        installed: false,
        path: null,
        readOnly: false,
        sha256Ok: null,
        downloading: false,
        partialBytes: null,
        ...model,
      },
    ],
    runtime: {
      installed: true,
      path: '/app-data/runtimes/onnxruntime.dll',
      readOnly: false,
      downloading: false,
      version: '1.28.0',
      flavour: 'cuda12',
      bytes: 455_344_532,
      flavours: [],
      available: true,
      installedFlavour: null,
      installedVersion: null,
      partialBytes: null,
      ...runtime,
    },
    modelsDir: '/app-data/models',
    runtimeDir: '/app-data/runtimes',
    hasToken: false,
    tokenStore: 'keychain',
    tokenStoreReason: null,
    ...token,
  }
}

/** @type {ReturnType<typeof vi.fn>} */
let listModels
/** @type {ReturnType<typeof vi.fn>} */
let discardPartial

function stub(answer) {
  listModels = vi.fn(async () => answer())
  discardPartial = vi.fn(async () => true)
  setBackend(
    /** @type {any} */ ({
      listModels: (/** @type {any} */ options) => listModels(options),
      discardPartial: (/** @type {any} */ spec) => discardPartial(spec),
      listAccelerators: vi.fn(async () => ({ providers: [], models: [] })),
      listSidecarModels: vi.fn(async () => []),
      sidecarAvailable: vi.fn(async () => ({ available: false, reasonKey: null })),
      about: vi.fn(async () => ({ appVersion: '0.0.0-test', facts: [] })),
      subscribe: vi.fn(() => () => {}),
      writeSettings: vi.fn(async () => ({})),
    }),
  )
}

afterEach(() => {
  cleanup()
  setBackend(null)
  vi.clearAllMocks()
})

/** Mount, open the Models tab, and wait for the first catalogue to be drawn. */
async function open(answer) {
  stub(answer)
  const rendered = render(SettingsDialog, { props: { spec: SPEC } })

  const tab = rendered.getByRole('tab', { name: t('settings.section.models') })
  await fireEvent.click(tab)
  expect(tab.getAttribute('aria-selected')).toBe('true')
  const panel = /** @type {HTMLElement} */ (
    rendered.container.querySelector(`#${tab.getAttribute('aria-controls')}`)
  )
  expect(panel.hasAttribute('hidden')).toBe(false)

  await waitFor(() => expect(listModels).toHaveBeenCalled())
  return rendered
}

describe('the bytes a stopped download left', () => {
  it('are reported under the row, with a press that gives them back', async () => {
    const rendered = await open(() => view({ model: { partialBytes: 104_857_600 } }))

    const line = t('settings.models.status.partial', { bytes: 104_857_600 })
    await waitFor(() => expect(rendered.getByText(line)).toBeTruthy())

    const discard = /** @type {HTMLButtonElement} */ (
      rendered.getByText(t('settings.models.action.discard')).closest('button')
    )
    await fireEvent.click(discard)
    expect(discardPartial).toHaveBeenCalledWith({ id: 'inpainter' })
    // And the press asks again, because the answer changes the row it was
    // pressed on - including when the answer is `false` for a download that
    // started underneath it.
    await waitFor(() => expect(listModels).toHaveBeenCalledTimes(2))
  })

  it('are not offered for a row with no unfinished download', async () => {
    const rendered = await open(() => view())
    await waitFor(() => expect(rendered.getByText(t('settings.models.action.download'))).toBeTruthy())
    expect(rendered.queryByText(t('settings.models.action.discard'))).toBe(null)
  })
})

describe('the runtime build that is actually installed', () => {
  it('is named when it is not the one the row would download', async () => {
    const rendered = await open(() =>
      view({ runtime: { installedFlavour: 'directml', installedVersion: '1.24.4' } }),
    )

    const line = t('settings.models.runtime.installedDiffers', {
      installed: 'directml',
      installedVersion: '1.24.4',
      chosen: 'cuda12',
      chosenVersion: '1.28.0',
    })
    await waitFor(() => expect(rendered.getByText(line)).toBeTruthy())
  })

  it('is said nothing about when it agrees, or when there is no record of it', async () => {
    const agreeing = await open(() =>
      view({ runtime: { installedFlavour: 'cuda12', installedVersion: '1.28.0' } }),
    )
    await waitFor(() => expect(agreeing.getByText(t('settings.models.runtime.label'))).toBeTruthy())
    expect(agreeing.container.querySelectorAll('.row-partial')).toHaveLength(0)
    cleanup()

    // `null` is unknown rather than none - a runtime this application did not
    // unpack - and an unknown build is nothing to report a difference about.
    const unknown = await open(() => view())
    await waitFor(() => expect(unknown.getByText(t('settings.models.runtime.label'))).toBeTruthy())
    expect(unknown.container.querySelectorAll('.row-partial')).toHaveLength(0)
  })
})

describe('a credential store that would not answer', () => {
  it('says which of the four things it did', async () => {
    const rendered = await open(() =>
      view({ token: { tokenStore: 'fileStoreUnavailable', tokenStoreReason: 'locked' } }),
    )

    await waitFor(() =>
      expect(rendered.getByText(t('settings.models.token.storeUnreachable'))).toBeTruthy(),
    )
    // The reason is the second line: what is wrong with the place the token
    // should be, rather than where it went instead.
    expect(rendered.getByText(t('settings.models.token.reason.locked'))).toBeTruthy()
  })

  it('says nothing extra when the store took the token', async () => {
    const rendered = await open(() => view({ token: { hasToken: true } }))
    await waitFor(() => expect(rendered.getByText(t('settings.models.token.saved'))).toBeTruthy())
    for (const reason of ['locked', 'unreachable', 'ambiguous', 'unknown']) {
      expect(rendered.queryByText(t(`settings.models.token.reason.${reason}`))).toBe(null)
    }
  })
})

describe('the once-per-process credential-store retry', () => {
  it('is asked for on the open and on no other call', async () => {
    const rendered = await open(() => view({ model: { partialBytes: 4_096 } }))
    expect(listModels).toHaveBeenCalledWith({ retryStore: true })

    // Any press that refreshes the list is a refresh, not an open: asking again
    // there is the prompt-per-poll removed.
    await waitFor(() => expect(rendered.getByText(t('settings.models.action.discard'))).toBeTruthy())
    const discard = /** @type {HTMLButtonElement} */ (
      rendered.getByText(t('settings.models.action.discard')).closest('button')
    )
    await fireEvent.click(discard)
    await waitFor(() => expect(listModels).toHaveBeenCalledTimes(2))
    expect(listModels.mock.calls[1][0]).toEqual({})
  })
})
