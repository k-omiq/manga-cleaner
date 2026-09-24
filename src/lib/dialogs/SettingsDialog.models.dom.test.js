/**
 * The file rows of the Settings screen, mounted: the three things a row says
 * that nothing on the row used to say at all.
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
 * The rows live in three sections now: the redraw weight under **Cleaning**,
 * the runtime under **Performance**, the token under **General**. `open()`
 * presses the section a test names and checks its panel is shown, and the
 * assertions are scoped to that panel: the panels are mounted and merely
 * `hidden`, so a query over the whole screen would find a row in a hidden
 * panel too.
 *
 * The end of the file is the two pipelines themselves: which section a file
 * is managed from, what the per-language pickers store, what the engine
 * tables say, and the FLUX helper's folder field.
 */

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render, waitFor, within } from '@testing-library/svelte'

import { setBackend } from '../api/backend.js'
import { t } from '../i18n/index.js'
import { DEFAULT_DETECTOR } from '../model/pipelines.js'
import { capabilities } from '../state/capabilities.svelte.js'
import { session, setDetection, setFluxModel, setSidecarPath } from '../state/session.svelte.js'
import SettingsDialog from './SettingsDialog.svelte'
import { resetFirstLaunch } from './firstlaunch.svelte.js'

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
/** @type {ReturnType<typeof vi.fn>} */
let writeSettings
/** What the FLUX helper lists, for the tests that turn it on. */
let helperModels = []

function stub(answer) {
  listModels = vi.fn(async () => answer())
  discardPartial = vi.fn(async () => true)
  writeSettings = vi.fn(async () => ({}))
  setBackend(
    /** @type {any} */ ({
      listModels: (/** @type {any} */ options) => listModels(options),
      discardPartial: (/** @type {any} */ spec) => discardPartial(spec),
      listAccelerators: vi.fn(async () => ({ providers: [], models: [] })),
      listSidecarModels: vi.fn(async () => helperModels),
      sidecarAvailable: vi.fn(async () => ({ available: capabilities.sidecar, reasonKey: null })),
      about: vi.fn(async () => ({ appVersion: '0.0.0-test', facts: [] })),
      subscribe: vi.fn(() => () => {}),
      writeSettings: (/** @type {any} */ patch) => writeSettings(patch),
    }),
  )
}

afterEach(() => {
  cleanup()
  resetFirstLaunch()
  setBackend(null)
  vi.clearAllMocks()
})

/**
 * Mount, open a section, and wait for the first catalogue to be drawn. The
 * queries returned are scoped to that section's panel.
 *
 * @param {() => any} answer
 * @param {string} sectionKey - the tab's label key
 */
async function open(answer, sectionKey = 'pipelines.cleaning') {
  stub(answer)
  const rendered = render(SettingsDialog, { props: { spec: SPEC } })

  const tab = rendered.getByRole('tab', { name: t(sectionKey) })
  await fireEvent.click(tab)
  expect(tab.getAttribute('aria-selected')).toBe('true')
  const panel = /** @type {HTMLElement} */ (
    rendered.container.querySelector(`#${tab.getAttribute('aria-controls')}`)
  )
  expect(panel.hasAttribute('hidden')).toBe(false)

  await waitFor(() => expect(listModels).toHaveBeenCalled())
  return { ...within(panel), container: panel }
}

const PERFORMANCE = 'settings.section.performance'
const GENERAL = 'settings.section.general'

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
    const rendered = await open(
      () => view({ runtime: { installedFlavour: 'directml', installedVersion: '1.24.4' } }),
      PERFORMANCE,
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
    const agreeing = await open(
      () => view({ runtime: { installedFlavour: 'cuda12', installedVersion: '1.28.0' } }),
      PERFORMANCE,
    )
    await waitFor(() => expect(agreeing.getByText(t('settings.models.runtime.label'))).toBeTruthy())
    expect(agreeing.container.querySelectorAll('.row-partial')).toHaveLength(0)
    cleanup()

    // `null` is unknown rather than none - a runtime this application did not
    // unpack - and an unknown build is nothing to report a difference about.
    const unknown = await open(() => view(), PERFORMANCE)
    await waitFor(() => expect(unknown.getByText(t('settings.models.runtime.label'))).toBeTruthy())
    expect(unknown.container.querySelectorAll('.row-partial')).toHaveLength(0)
  })
})

describe('a credential store that would not answer', () => {
  it('says which of the four things it did', async () => {
    const rendered = await open(
      () => view({ token: { tokenStore: 'fileStoreUnavailable', tokenStoreReason: 'locked' } }),
      GENERAL,
    )

    await waitFor(() =>
      expect(rendered.getByText(t('settings.models.token.storeUnreachable'))).toBeTruthy(),
    )
    // The reason is the second line: what is wrong with the place the token
    // should be, rather than where it went instead.
    expect(rendered.getByText(t('settings.models.token.reason.locked'))).toBeTruthy()
  })

  it('says nothing extra when the store took the token', async () => {
    const rendered = await open(() => view({ token: { hasToken: true } }), GENERAL)
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

describe('the two pipelines', () => {
  /** The catalogue with one detection weight beside the redraw weight. */
  function both() {
    const answer = view()
    answer.models.push({
      ...answer.models[0],
      id: 'textDetector',
      fileName: 'comictextdetector.onnx',
      bytes: 94_669_756,
      kindKey: 'models.kind.textDetector',
      requiredBy: ['autoClean'],
    })
    return answer
  }

  afterEach(() => {
    for (const language of ['ja', 'zh', 'ko']) setDetection(language, DEFAULT_DETECTOR)
    capabilities.sidecar = false
    helperModels = []
    setFluxModel('')
    setSidecarPath('')
  })

  it('manage each file in the section whose engines need it', async () => {
    const detection = await open(both, 'pipelines.detection')
    await waitFor(() => expect(detection.getByText(t('models.kind.textDetector'))).toBeTruthy())
    expect(detection.queryByText(t('models.kind.inpainter'))).toBe(null)
    cleanup()

    const cleaning = await open(both)
    await waitFor(() => expect(cleaning.getByText(t('models.kind.inpainter'))).toBeTruthy())
    expect(cleaning.queryByText(t('models.kind.textDetector'))).toBe(null)
  })

  it('store Skip as null, and a detector by its id', async () => {
    const detection = await open(both, 'pipelines.detection')
    const japanese = /** @type {HTMLSelectElement} */ (
      detection.getByRole('combobox', {
        name: t('pipelines.detectorFor', { language: t('pipelines.language.ja') }),
      })
    )
    expect(japanese.value).toBe(DEFAULT_DETECTOR)

    await fireEvent.change(japanese, { target: { value: '' } })
    expect(session.detection.ja).toBe(null)
    expect(japanese.value).toBe('')

    await fireEvent.change(japanese, { target: { value: 'ctd-rtdetr-ocr' } })
    expect(session.detection.ja).toBe('ctd-rtdetr-ocr')
  })

  it('offer each language only the detectors that serve it, and Skip', async () => {
    const detection = await open(both, 'pipelines.detection')
    const options = (/** @type {string} */ language) =>
      [
        .../** @type {HTMLSelectElement} */ (
          detection.getByRole('combobox', {
            name: t('pipelines.detectorFor', { language: t(`pipelines.language.${language}`) }),
          })
        ).options,
      ].map((option) => option.value)
    expect(options('ja')).toEqual(['ctd-rtdetr', 'ctd-rtdetr-ocr', ''])
    expect(options('zh')).toEqual(['ctd-rtdetr', ''])
    expect(options('ko')).toEqual(['ctd-rtdetr', ''])
  })

  it('say what each engine still costs, or why it cannot be chosen', async () => {
    const detection = await open(both, 'pipelines.detection')
    const table = detection.getByRole('table', { name: t('pipelines.detection') })
    const state = (/** @type {string} */ name) =>
      within(table).getByText(name).closest('[role="row"]')?.querySelector('.state')?.textContent?.trim()
    // The base detector needs the text detector here, which is not installed.
    await waitFor(() => expect(state('CTD + RT-DETR v2')).toBe(t('models.value.size', { bytes: 94_669_756 })))
    expect(state('RT-DETR v2 + COO + SAM-TS')).toBe(t('pipelines.status.soon'))
  })

  it('mark a FLUX model the helper lists as found, and the rest as needing it', async () => {
    capabilities.sidecar = true
    helperModels = [{ id: 'flux2-klein-4b', label: 'FLUX.2 Klein 4B' }]
    const cleaning = await open(both)
    const table = cleaning.getByRole('table', { name: t('pipelines.cleaning') })
    const row = (/** @type {string} */ name) =>
      /** @type {HTMLElement} */ (within(table).getByText(name).closest('[role="row"]'))

    await waitFor(() => expect(row('FLUX.2 Klein 4B').textContent).toContain(t('pipelines.status.found')))
    expect(row('FLUX.2 Klein 4B').classList.contains('soon')).toBe(false)
    expect(row('FLUX.2 Klein 9B').textContent).toContain(t('pipelines.status.needsHelper'))
    expect(row('FLUX.2 Klein 9B').classList.contains('soon')).toBe(true)
    expect(row('Big LaMa').textContent).toContain(t('pipelines.status.soon'))
    // The fallback model the helper chose is written, not only remembered.
    await waitFor(() => expect(writeSettings).toHaveBeenCalledWith(expect.objectContaining({ fluxModel: 'flux2-klein-4b' })))
  })

  it('draw a stored FLUX model the helper no longer lists as itself', async () => {
    capabilities.sidecar = true
    helperModels = [{ id: 'flux2-klein-4b', label: 'FLUX.2 Klein 4B' }]
    setFluxModel('flux1-dev')
    const cleaning = await open(both)
    const model = /** @type {HTMLSelectElement} */ (
      await waitFor(() => cleaning.getByLabelText(t('settings.sidecarModel.label')))
    )
    await waitFor(() => expect(model.options).toHaveLength(2))
    expect(model.value).toBe('flux1-dev')
    expect(model.selectedOptions[0].textContent?.trim()).toBe(t('settings.sidecarModel.missing', { id: 'flux1-dev' }))
    expect(session.fluxModel).toBe('flux1-dev')
  })

  it('write the helper folder when the field is left, not on every keystroke', async () => {
    const cleaning = await open(both)
    const field = /** @type {HTMLInputElement} */ (cleaning.getByLabelText(t('settings.sidecar.label')))
    writeSettings.mockClear()

    await fireEvent.input(field, { target: { value: '/opt/fl' } })
    await fireEvent.input(field, { target: { value: '/opt/flux' } })
    expect(writeSettings).not.toHaveBeenCalled()
    expect(session.sidecarPath).toBe('')

    await fireEvent.blur(field)
    expect(session.sidecarPath).toBe('/opt/flux')
    await waitFor(() => expect(writeSettings).toHaveBeenCalledTimes(1))
    expect(writeSettings).toHaveBeenCalledWith(expect.objectContaining({ sidecarPath: '/opt/flux' }))
  })
})
