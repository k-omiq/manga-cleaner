/**
 * The setup a first launch opens, mounted: the six steps over the real
 * session, the real stores and a hand-written seam stub.
 *
 * The session and capabilities are the real modules, so a setting the setup
 * changes is checked where the rest of the app reads it. The backend is a stub
 * (`setBackend`) whose `subscribe` the tests drive, because the download run
 * is a sequence of calls and `model-progress` events and the interesting part
 * is the order they come in. The cloud provisioner is replaced by
 * `onboarding/ProvisionerStub.svelte`: the cloud step is tested against the
 * provisioner's props (IC-5), not against its flow, which is tested beside it.
 */

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render, waitFor } from '@testing-library/svelte'

vi.mock('./CloudProvisioner.svelte', async () => ({
  default: (await import('./onboarding/ProvisionerStub.svelte')).default,
}))

import { setBackend } from '../api/backend.js'
import { t } from '../i18n/index.js'
import { app, closeAllModals, pushModal } from '../state/app.svelte.js'
import { capabilities } from '../state/capabilities.svelte.js'
import {
  session,
  setAccelerator,
  setCloseToTray,
  setCloudAllowed,
  setFluxBackend,
  setFluxModel,
  setReadingDirection,
  setSidecarPath,
} from '../state/session.svelte.js'
import FirstLaunchDialog from './FirstLaunchDialog.svelte'
import SettingsDialog from './SettingsDialog.svelte'
import { DEFAULT_FLUX_MODEL, RUNTIME_ID } from './firstlaunch.js'
import {
  configureFirstLaunchCloud,
  dismissFirstLaunch,
  firstLaunch,
  offerFirstLaunch,
  pauseFirstLaunchDownloads,
  resetFirstLaunch,
  setFirstLaunchStep,
  startFirstLaunchDownloads,
} from './firstlaunch.svelte.js'

/** Each step's heading, in the order the steps come. */
const HEADINGS = [
  'onboarding.welcome.heading',
  'onboarding.models.heading',
  'onboarding.defaults.heading',
  'onboarding.cloud.heading',
  'onboarding.behavior.heading',
  'onboarding.done.heading',
]

/** Sizes small enough to add up by eye. */
const REQUIRED_BYTES = 95 + 4 + 1 + 11
const RUNTIME_BYTES = 32
const REDRAW_BYTES = 207
const READER_BYTES = 343 + 117 + 1
/** What Download costs with the recommended ticks. */
const PRICE = REQUIRED_BYTES + RUNTIME_BYTES + REDRAW_BYTES

/** The spec `pushModal({kind: 'settings'})` would have handed Settings. */
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

/**
 * The catalogue with nothing installed: the Auto clean set, the redraw
 * engine, the Japanese reader's three files, and the runtime. Whole
 * `ModelsView` rows, because Settings draws every field of them when the
 * replay test mounts it.
 *
 * @param {{runtimeInstalled?: boolean}} [options]
 */
function view({ runtimeInstalled = false } = {}) {
  const row = (id, kindKey, bytes, requiredBy) => ({
    id,
    fileName: `${id}.onnx`,
    kindKey,
    bytes,
    requiredBy,
    installed: false,
    path: null,
    readOnly: false,
    sha256Ok: null,
    downloading: false,
    partialBytes: null,
  })
  return {
    modelsDir: '/app-data/models',
    runtimeDir: '/app-data/runtimes',
    hasToken: false,
    tokenStore: 'keychain',
    tokenStoreReason: null,
    models: [
      row('textDetector', 'models.kind.textDetector', 95, ['autoClean']),
      row('inpainter', 'models.kind.inpainter', REDRAW_BYTES, ['lama']),
      row('scriptGate', 'models.kind.scriptGate', 4, ['autoClean']),
      row('scriptGateLabels', 'models.kind.scriptGateLabels', 1, ['autoClean']),
      row('balloonDetector', 'models.kind.balloonDetector', 11, ['autoClean']),
      row('ocrEncoder', 'models.kind.ocr', 343, []),
      row('ocrDecoder', 'models.kind.ocrDecoder', 117, []),
      row('ocrVocab', 'models.kind.ocrVocab', 1, []),
    ],
    runtime: {
      installed: runtimeInstalled,
      bytes: RUNTIME_BYTES,
      available: true,
      path: null,
      readOnly: false,
      downloading: false,
      version: '1.28.0',
      flavour: 'cpu',
      flavours: [],
      installedFlavour: null,
      installedVersion: null,
      partialBytes: null,
    },
  }
}

/**
 * What the runtime answers about its processors: one it can use, one it
 * cannot, and the stored preference marked the way the backend marks it.
 *
 * @param {string} preference
 */
function accelerators(preference) {
  const provider = (id, available, reasonKey) => ({
    id,
    labelKey: `accel.${id}`,
    available,
    reasonKey,
    measured: false,
    active: available,
    selected: id === preference,
  })
  return {
    preference,
    providers: [provider('cpu', true, null), provider('coreml', false, 'accel.declined.unavailable')],
    models: [],
  }
}

function makeBackend() {
  const handlers = new Set()
  return {
    /** Deliver one event to everything subscribed, as the process-wide channel does. */
    emit(event) {
      for (const handler of [...handlers]) handler(event)
    },
    subscribe: vi.fn((handler) => {
      handlers.add(handler)
      return () => handlers.delete(handler)
    }),
    listModels: vi.fn(async () => view()),
    writeSettings: vi.fn(async () => ({})),
    downloadRuntime: vi.fn(async () => 'started'),
    downloadModel: vi.fn(async () => 'started'),
    cancelDownload: vi.fn(async () => true),
    listAccelerators: vi.fn(async () => accelerators('auto')),
    listSidecarModels: vi.fn(async () => []),
    sidecarAvailable: vi.fn(async () => ({ available: false, reasonKey: null })),
    about: vi.fn(async () => ({ appVersion: '0.0.0-test', facts: [] })),
    discardPartial: vi.fn(async () => true),
  }
}

/** @type {ReturnType<typeof makeBackend>} */
let backend

beforeEach(() => {
  backend = makeBackend()
  setBackend(/** @type {any} */ (backend))
  setCloseToTray(false)
  setReadingDirection('rtl')
  setCloudAllowed(false)
  setAccelerator('auto')
  setFluxModel('')
  setFluxBackend('auto')
  setSidecarPath('')
  session.firstLaunchOffered = false
  capabilities.sidecar = false
})

afterEach(() => {
  cleanup()
  resetFirstLaunch()
  closeAllModals()
  setBackend(null)
  vi.clearAllMocks()
})

/** Open the setup the way a first launch does, on one of its steps. */
function open(step = 'welcome', answer = view()) {
  expect(offerFirstLaunch(answer)).toBe(true)
  setFirstLaunchStep(step)
  return render(FirstLaunchDialog)
}

/** @param {ReturnType<typeof render>} rendered */
function heading(rendered) {
  return rendered.getByRole('heading', { level: 3 })
}

/**
 * @param {ReturnType<typeof render>} rendered
 * @param {string} name
 */
async function press(rendered, name) {
  await fireEvent.click(rendered.getByRole('button', { name }))
}

/** A download ending well, as the backend reports it. @param {string} id */
function finish(id) {
  backend.emit({ type: 'model-progress', id, downloaded: 1, total: 1, done: true, error: null })
}

describe('the six steps', () => {
  it('come in order, forward and back, with one choice on each', async () => {
    const rendered = open()
    const seen = [heading(rendered).textContent]
    expect(rendered.getByText(t('onboarding.stepOf', { current: 1, total: 6 }))).toBeTruthy()
    // Nothing to go back to on the first step.
    expect(rendered.queryByRole('button', { name: t('onboarding.action.back') })).toBeNull()

    await press(rendered, t('onboarding.action.start'))
    expect(firstLaunch.step).toBe('models')
    await press(rendered, t('onboarding.action.back'))
    expect(firstLaunch.step).toBe('welcome')

    await press(rendered, t('onboarding.action.start'))
    seen.push(heading(rendered).textContent)
    await press(rendered, t('onboarding.action.notNow'))
    seen.push(heading(rendered).textContent)
    await press(rendered, t('onboarding.action.next'))
    seen.push(heading(rendered).textContent)
    await press(rendered, t('onboarding.action.notNow'))
    seen.push(heading(rendered).textContent)
    await press(rendered, t('onboarding.action.next'))
    seen.push(heading(rendered).textContent)

    expect(seen).toEqual(HEADINGS.map((key) => t(key)))
    expect(rendered.getByText(t('onboarding.stepOf', { current: 6, total: 6 }))).toBeTruthy()
    // Walking through with the defaults fetches nothing and changes nothing.
    expect(backend.downloadRuntime).not.toHaveBeenCalled()
    expect(backend.downloadModel).not.toHaveBeenCalled()
    expect(backend.writeSettings).not.toHaveBeenCalled()
  })

  it('put focus on each heading, where Enter presses the primary action', async () => {
    const rendered = open()
    await waitFor(() => expect(document.activeElement).toBe(heading(rendered)))
    await fireEvent.keyDown(heading(rendered), { key: 'Enter' })
    expect(firstLaunch.step).toBe('models')
    await waitFor(() => expect(document.activeElement).toBe(heading(rendered)))
    expect(heading(rendered).textContent).toBe(t('onboarding.models.heading'))

    // Download is withheld from Enter: a key pressed twice on the step before
    // must not start a transfer of hundreds of megabytes.
    await fireEvent.keyDown(heading(rendered), { key: 'Enter' })
    expect(firstLaunch.step).toBe('models')
    expect(backend.downloadRuntime).not.toHaveBeenCalled()

    await press(rendered, t('onboarding.action.notNow'))
    await waitFor(() => expect(document.activeElement).toBe(heading(rendered)))
    // Enter on a control belongs to the control, and a held key is not a press.
    await fireEvent.keyDown(rendered.getByRole('button', { name: t('onboarding.action.back') }), { key: 'Enter' })
    await fireEvent.keyDown(heading(rendered), { key: 'Enter', repeat: true })
    expect(firstLaunch.step).toBe('defaults')
    await fireEvent.keyDown(heading(rendered), { key: 'Enter' })
    expect(firstLaunch.step).toBe('cloud')
  })
})

describe('leaving early', () => {
  it('Skip setup closes it from any step and records that it was offered', async () => {
    const rendered = open('defaults')
    await press(rendered, t('onboarding.action.skip'))
    expect(firstLaunch.open).toBe(false)
    expect(session.firstLaunchOffered).toBe(true)

    // The next launch does not ask again; Settings still can.
    expect(offerFirstLaunch(view())).toBe(false)
    expect(firstLaunch.open).toBe(false)
    expect(offerFirstLaunch(view(), { force: true })).toBe(true)
    expect(firstLaunch.step).toBe('welcome')
  })

  it('Escape closes it the same way', async () => {
    open('behavior')
    await fireEvent.keyDown(window, { key: 'Escape' })
    expect(firstLaunch.open).toBe(false)
    expect(session.firstLaunchOffered).toBe(true)
  })
})

describe('the recommended choices', () => {
  it('are already made when the setup opens', async () => {
    const rendered = open('models')
    const required = /** @type {HTMLInputElement} */ (rendered.getByLabelText(t('onboarding.models.required.label')))
    const redraw = /** @type {HTMLInputElement} */ (rendered.getByLabelText(t('models.kind.inpainter')))
    const reader = /** @type {HTMLInputElement} */ (rendered.getByLabelText(t('models.kind.ocr')))
    // Cleaning needs every required file, so there is no box to clear.
    expect([required.checked, required.disabled]).toEqual([true, true])
    expect([redraw.checked, redraw.disabled]).toEqual([true, false])
    expect([reader.checked, reader.disabled]).toEqual([false, false])
    expect(rendered.getByRole('button', { name: t('onboarding.models.download', { bytes: PRICE }) })).toBeTruthy()

    // The reader is three files and one tick.
    await fireEvent.click(reader)
    expect([firstLaunch.selection.ocrEncoder, firstLaunch.selection.ocrDecoder, firstLaunch.selection.ocrVocab]).toEqual(
      [true, true, true],
    )
    expect(
      rendered.getByRole('button', { name: t('onboarding.models.download', { bytes: PRICE + READER_BYTES }) }),
    ).toBeTruthy()

    setFirstLaunchStep('defaults')
    await waitFor(() => expect(heading(rendered).textContent).toBe(t('onboarding.defaults.heading')))
    expect(/** @type {HTMLSelectElement} */ (rendered.getByLabelText(t('settings.accel.label'))).value).toBe('auto')
    // Asking the runtime maps its library, which its own download has to be
    // able to replace, so a machine without it is not asked.
    expect(backend.listAccelerators).not.toHaveBeenCalled()
    expect(rendered.getByText(t('onboarding.defaults.accel.later'))).toBeTruthy()

    setFirstLaunchStep('behavior')
    await waitFor(() => expect(heading(rendered).textContent).toBe(t('onboarding.behavior.heading')))
    expect(/** @type {HTMLInputElement} */ (rendered.getByLabelText(t('settings.background.label'))).checked).toBe(false)
    expect(rendered.getByRole('radio', { name: t('settings.direction.rtl') }).getAttribute('aria-checked')).toBe('true')
    expect(backend.writeSettings).not.toHaveBeenCalled()
  })

  it('include the recommended AI redraw model when a helper is installed', async () => {
    capabilities.sidecar = true
    backend.sidecarAvailable.mockResolvedValue({ available: true, reasonKey: null })
    backend.listSidecarModels.mockResolvedValue([
      { id: 'other', label: 'Other model' },
      { id: DEFAULT_FLUX_MODEL, label: 'FLUX.2 klein 4B' },
    ])
    const rendered = open('defaults')

    await waitFor(() => expect(session.fluxModel).toBe(DEFAULT_FLUX_MODEL))
    // Written through, because the backend's own fallback need not be a
    // model this helper has.
    expect(backend.writeSettings).toHaveBeenLastCalledWith(expect.objectContaining({ fluxModel: DEFAULT_FLUX_MODEL }))
    const model = /** @type {HTMLSelectElement} */ (rendered.getByLabelText(t('settings.sidecarModel.label')))
    await waitFor(() => expect(model.value).toBe(DEFAULT_FLUX_MODEL))
  })
})

describe('the download step', () => {
  it('fetches the runtime first, pauses the transfer in flight, and resumes where it stopped', async () => {
    const rendered = open('models')
    await press(rendered, t('onboarding.models.download', { bytes: PRICE }))
    await waitFor(() => expect(backend.downloadRuntime).toHaveBeenCalledTimes(1))
    expect(backend.downloadModel).not.toHaveBeenCalled()
    expect(session.firstLaunchOffered).toBe(true)

    const status = () => rendered.getByRole('status').textContent
    await waitFor(() =>
      expect(status()).toBe(t('onboarding.models.run.downloading', { nameKey: 'settings.models.runtime.label' })),
    )
    backend.emit({ type: 'model-progress', id: RUNTIME_ID, downloaded: 16, total: 32, done: false })
    await waitFor(() =>
      expect(rendered.getByRole('progressbar').getAttribute('aria-valuenow')).toBe(
        String(Math.floor((16 / PRICE) * 100)),
      ),
    )

    // The setup goes on while it runs, and the bar is there on the way back.
    expect(rendered.getByText(t('onboarding.models.background'))).toBeTruthy()
    await press(rendered, t('onboarding.action.next'))
    expect(firstLaunch.step).toBe('defaults')
    expect(firstLaunch.running).toBe(true)
    await press(rendered, t('onboarding.action.back'))
    expect(rendered.getByRole('progressbar')).toBeTruthy()

    await press(rendered, t('onboarding.models.pause'))
    expect(backend.cancelDownload).toHaveBeenCalledWith({ id: RUNTIME_ID })
    backend.emit({ type: 'model-progress', id: RUNTIME_ID, downloaded: 16, total: 32, done: true, error: 'cancelled' })
    await waitFor(() => expect(status()).toBe(t('onboarding.models.run.paused')))

    await press(rendered, t('onboarding.models.resume'))
    await waitFor(() => expect(backend.downloadRuntime).toHaveBeenCalledTimes(2))
    finish(RUNTIME_ID)
    const weights = ['textDetector', 'scriptGate', 'scriptGateLabels', 'balloonDetector', 'inpainter']
    for (const id of weights) {
      await waitFor(() => expect(backend.downloadModel).toHaveBeenLastCalledWith({ id }))
      finish(id)
    }
    await waitFor(() => expect(status()).toBe(t('onboarding.models.run.done')))
    // The reader was not ticked, so nothing of it was fetched.
    expect(backend.downloadModel.mock.calls.map(([spec]) => spec.id)).toEqual(weights)
    expect(rendered.getByLabelText(t('onboarding.models.required.label')).closest('li')?.textContent).toContain(
      t('onboarding.models.status.installed'),
    )
  })

  it('keeps a run still in flight when the setup is opened again', async () => {
    open('models')
    const run = startFirstLaunchDownloads()
    await waitFor(() => expect(backend.downloadRuntime).toHaveBeenCalled())
    const plan = firstLaunch.plan

    dismissFirstLaunch()
    expect(offerFirstLaunch(view({ runtimeInstalled: true }), { force: true })).toBe(true)
    // A new plan would quote bytes already on their way.
    expect(firstLaunch.plan).toBe(plan)
    expect(firstLaunch.running).toBe(true)

    await pauseFirstLaunchDownloads()
    backend.emit({ type: 'model-progress', id: RUNTIME_ID, downloaded: 0, total: 32, done: true, error: 'cancelled' })
    await run
    expect(firstLaunch.paused).toBe(true)
  })
})

describe('a setting changed in the setup', () => {
  it('goes through the session setter and on to the backend, as Settings sends it', async () => {
    const rendered = open('behavior')
    await fireEvent.click(rendered.getByLabelText(t('settings.background.label')))
    await waitFor(() => expect(backend.writeSettings).toHaveBeenCalledTimes(1))
    expect(session.closeToTray).toBe(true)
    expect(backend.writeSettings).toHaveBeenLastCalledWith(
      expect.objectContaining({ closeToTray: true, readingDirection: 'rtl', cloudEngines: 'blocked' }),
    )

    await fireEvent.click(rendered.getByRole('radio', { name: t('settings.direction.ltr') }))
    await waitFor(() => expect(backend.writeSettings).toHaveBeenCalledTimes(2))
    expect(session.readingDirection).toBe('ltr')
    expect(backend.writeSettings).toHaveBeenLastCalledWith(
      expect.objectContaining({ closeToTray: true, readingDirection: 'ltr' }),
    )
  })

  it('is taken back, and the step says so, when the backend refuses it', async () => {
    const rendered = open('behavior')
    backend.writeSettings.mockRejectedValueOnce(new Error('disk full'))
    const tray = /** @type {HTMLInputElement} */ (rendered.getByLabelText(t('settings.background.label')))
    await fireEvent.click(tray)

    await waitFor(() => expect(rendered.getByRole('alert').textContent).toBe(t('onboarding.saveFailed')))
    expect(session.closeToTray).toBe(false)
    expect(tray.checked).toBe(false)
  })

  it('asks the runtime for its processors once it is here, and writes the one chosen', async () => {
    // The backend marks whichever preference it was last sent.
    backend.listAccelerators.mockImplementation(async () =>
      accelerators(backend.writeSettings.mock.lastCall?.[0]?.accelerator ?? 'auto'),
    )
    const rendered = open('defaults', view({ runtimeInstalled: true }))
    const accel = /** @type {HTMLSelectElement} */ (rendered.getByLabelText(t('settings.accel.label')))
    await waitFor(() => expect(accel.options).toHaveLength(3))
    expect([...accel.options].map((option) => [option.value, option.disabled])).toEqual([
      ['auto', false],
      ['cpu', false],
      ['coreml', true],
    ])
    expect(accel.value).toBe('auto')

    await fireEvent.change(accel, { target: { value: 'cpu' } })
    await waitFor(() => expect(backend.listAccelerators).toHaveBeenCalledTimes(2))
    expect(session.accelerator).toBe('cpu')
    expect(backend.writeSettings).toHaveBeenLastCalledWith(expect.objectContaining({ accelerator: 'cpu' }))
    await waitFor(() => expect(accel.value).toBe('cpu'))

    // A refused change puts the picker back on the value in force.
    backend.writeSettings.mockRejectedValueOnce(new Error('disk full'))
    await fireEvent.change(accel, { target: { value: 'auto' } })
    await waitFor(() => expect(rendered.getByRole('alert').textContent).toBe(t('onboarding.saveFailed')))
    expect(session.accelerator).toBe('cpu')
    expect(accel.value).toBe('cpu')
  })
})

describe('the cloud step', () => {
  it('Not now leaves cloud cleaning off and moves on', async () => {
    const rendered = open('cloud')
    expect(rendered.getByText(t('onboarding.cloud.consent'))).toBeTruthy()
    await press(rendered, t('onboarding.action.notNow'))
    expect(firstLaunch.step).toBe('behavior')
    expect(session.cloudAllowed).toBe(false)
    expect(backend.writeSettings).not.toHaveBeenCalled()
  })

  it('Set up now opens the provisioner in place, and its success turns cloud cleaning on', async () => {
    const rendered = open('cloud')
    await press(rendered, t('onboarding.cloud.setUp'))
    const provisioner = rendered.getByTestId('provisioner')
    expect(provisioner.dataset.inline).toBe('true')
    expect(provisioner.dataset.provider).toBe('modal')
    await waitFor(() => expect(provisioner.parentElement?.contains(document.activeElement)).toBe(true))
    // The provisioner draws its own buttons until it has finished.
    expect(rendered.queryByRole('button', { name: t('onboarding.action.back') })).toBeNull()
    expect(rendered.queryByRole('button', { name: t('onboarding.action.next') })).toBeNull()

    await fireEvent.click(rendered.getByTestId('provisioner-finish'))
    await waitFor(() => expect(session.cloudAllowed).toBe(true))
    expect(backend.writeSettings).toHaveBeenLastCalledWith(expect.objectContaining({ cloudEngines: 'allowed' }))
    // What the step keeps is what it says back: never the endpoint or a credential.
    expect(firstLaunch.cloud).toEqual({ provider: 'beam', name: 'Beam (mc-ab12cd)', healthy: true })

    await fireEvent.click(rendered.getByTestId('provisioner-close'))
    expect(rendered.queryByTestId('provisioner')).toBeNull()
    expect(rendered.getByRole('status').textContent).toBe(t('onboarding.cloud.ready', { name: 'Beam (mc-ab12cd)' }))
    await press(rendered, t('onboarding.action.next'))
    expect(firstLaunch.step).toBe('behavior')

    setFirstLaunchStep('done')
    await waitFor(() =>
      expect(rendered.container.querySelector('dl')?.textContent).toContain(
        t('onboarding.done.value.cloudOnNamed', { name: 'Beam (mc-ab12cd)' }),
      ),
    )
  })

  it('keeps an endpoint that did not answer its first check, leaves cloud cleaning off, and offers no second setup', async () => {
    const rendered = open('cloud')
    await press(rendered, t('onboarding.cloud.setUp'))
    await fireEvent.click(rendered.getByTestId('provisioner-finish-unchecked'))
    await waitFor(() => expect(firstLaunch.cloud).toEqual({ provider: 'beam', name: 'Beam (mc-ab12cd)', healthy: false }))
    expect(session.cloudAllowed).toBe(false)
    expect(backend.writeSettings).not.toHaveBeenCalled()
    expect(firstLaunch.cloudSaveFailed).toBe(false)

    await fireEvent.click(rendered.getByTestId('provisioner-close'))
    expect(rendered.getByRole('status').textContent).toBe(t('onboarding.cloud.unchecked', { name: 'Beam (mc-ab12cd)' }))
    // A second setup would be a second installation in the account.
    expect(rendered.queryByRole('button', { name: t('onboarding.cloud.setUp') })).toBeNull()
    await press(rendered, t('onboarding.action.next'))
    expect(firstLaunch.step).toBe('behavior')

    setFirstLaunchStep('done')
    await waitFor(() =>
      expect(rendered.container.querySelector('dl')?.textContent).toContain(
        t('onboarding.done.value.cloudOffSaved', { name: 'Beam (mc-ab12cd)' }),
      ),
    )
  })

  it('Escape closes the provisioner first, and a refused permission is said rather than shown as on', async () => {
    const rendered = open('cloud')
    await press(rendered, t('onboarding.cloud.setUp'))
    await fireEvent.keyDown(window, { key: 'Escape' })
    expect(firstLaunch.provisioning).toBe(false)
    expect(firstLaunch.open).toBe(true)

    backend.writeSettings.mockRejectedValueOnce(new Error('disk full'))
    await press(rendered, t('onboarding.cloud.setUp'))
    await fireEvent.click(rendered.getByTestId('provisioner-finish'))
    await waitFor(() => expect(firstLaunch.cloudSaveFailed).toBe(true))
    expect(session.cloudAllowed).toBe(false)

    // Once it has finished, Continue is the dialog's as well as the provisioner's.
    await press(rendered, t('onboarding.action.next'))
    expect(firstLaunch.step).toBe('behavior')
    await press(rendered, t('onboarding.action.back'))
    expect(rendered.getByRole('alert').textContent).toBe(t('onboarding.cloud.saveFailed'))
  })

  it('offers no way out while the provisioner is working in the account', async () => {
    const rendered = open('cloud')
    await press(rendered, t('onboarding.cloud.setUp'))
    await fireEvent.click(rendered.getByTestId('provisioner-busy'))
    const skip = /** @type {HTMLButtonElement} */ (rendered.getByRole('button', { name: t('onboarding.action.skip') }))
    expect(skip.disabled).toBe(true)
    await fireEvent.keyDown(window, { key: 'Escape' })
    expect(firstLaunch.provisioning).toBe(true)
    expect(firstLaunch.open).toBe(true)

    // Once it has stopped, Escape closes the provisioner as before.
    await fireEvent.click(rendered.getByTestId('provisioner-idle'))
    expect(skip.disabled).toBe(false)
    await fireEvent.keyDown(window, { key: 'Escape' })
    expect(firstLaunch.provisioning).toBe(false)
    expect(firstLaunch.open).toBe(true)

    // A provisioner that closes itself mid-run takes its busy state with it.
    await press(rendered, t('onboarding.cloud.setUp'))
    await fireEvent.click(rendered.getByTestId('provisioner-busy'))
    await fireEvent.click(rendered.getByTestId('provisioner-close'))
    expect(firstLaunch.provisionerBusy).toBe(false)
    expect(skip.disabled).toBe(false)
  })

  it('leaves cloud cleaning off when the setup names no saved endpoint', async () => {
    open('cloud')
    await configureFirstLaunchCloud({ provider: 'modal', profileId: '  ', name: 'Modal (mc-ab12cd)' })
    expect(firstLaunch.cloud).toBeNull()
    expect(session.cloudAllowed).toBe(false)
    expect(backend.writeSettings).not.toHaveBeenCalled()
  })
})

describe('the last step', () => {
  it('reads back what is in force and starts a new project', async () => {
    const rendered = open('done')
    expect(rendered.getByText(t('onboarding.done.bodyMissing'))).toBeTruthy()
    const summary = /** @type {HTMLElement} */ (rendered.container.querySelector('dl')).textContent
    for (const value of [
      t('onboarding.done.value.notDownloaded'),
      t('settings.accel.auto'),
      t('onboarding.done.value.cloudOff'),
      t('onboarding.done.value.trayQuit'),
      t('settings.direction.rtl'),
    ]) {
      expect(summary).toContain(value)
    }
    // Nothing is left to skip.
    expect(rendered.queryByRole('button', { name: t('onboarding.action.skip') })).toBeNull()
    expect(rendered.getByRole('button', { name: t('onboarding.action.close') })).toBeTruthy()

    await press(rendered, t('home.action.newProject'))
    expect(firstLaunch.open).toBe(false)
    expect(session.firstLaunchOffered).toBe(true)
    await waitFor(() => expect(app.modals.map((modal) => modal.kind)).toEqual(['newProject']))
  })
})

describe('Run setup again', () => {
  it('opens the setup from Settings > General over a fresh catalogue, with Settings closed', async () => {
    session.firstLaunchOffered = true
    pushModal({ kind: 'settings' })
    const rendered = render(SettingsDialog, { props: { spec: SPEC } })
    backend.listModels.mockClear()

    await press(rendered, t('onboarding.replay.action'))
    await waitFor(() => expect(firstLaunch.open).toBe(true))
    expect(backend.listModels).toHaveBeenCalledTimes(1)
    expect(firstLaunch.step).toBe('welcome')
    expect(firstLaunch.plan?.required.map((row) => row.id)).toContain(RUNTIME_ID)
    // The setup is drawn only while the modal stack is empty.
    expect(app.modals).toHaveLength(0)
  })

  it('says so, and leaves Settings open, when the catalogue cannot be read', async () => {
    backend.listModels.mockRejectedValue(new Error('offline'))
    pushModal({ kind: 'settings' })
    const rendered = render(SettingsDialog, { props: { spec: SPEC } })

    await press(rendered, t('onboarding.replay.action'))
    await waitFor(() => expect(rendered.getByText(t('onboarding.replay.failed'))).toBeTruthy())
    expect(firstLaunch.open).toBe(false)
    expect(app.modals).toHaveLength(1)
  })
})
