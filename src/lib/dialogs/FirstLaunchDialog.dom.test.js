/**
 * The first-launch offer, mounted.
 *
 * `firstlaunch.test.js` pins the arithmetic - what is missing, what is ticked,
 * how many bytes that is - and what is left is a *sequence*: one press starts
 * several downloads one after another, each waiting for the `done` event on the
 * seam's process-wide `model-progress` channel. Three of the orderings in it
 * are races that were wrong before they were written this way, and
 * none of them can be asserted without a stub backend whose clock this file
 * owns: a download ends when this test says it ends.
 *
 * The `.dom.test.js` suffix is how the file asks for a browser: the suite is
 * two vitest projects and this is the one with a document *and* Svelte
 * resolved through its browser export (`vite.config.js`).
 *
 * The component holds nothing, so most of what is asserted here is the store
 * in `firstlaunch.svelte.js` seen through the dialog it draws - which is the
 * point of that split: the run outlives the mount.
 */

import { describe, expect, it, beforeEach, afterEach, vi } from 'vitest'
import { render, cleanup, fireEvent } from '@testing-library/svelte'

import { t } from '../i18n/index.js'
import { isDialogOutsideStackOpen } from '../shortcuts.js'
import { firstLaunchPlan, initialSelection, plannedBytes } from './firstlaunch.js'

const stubs = vi.hoisted(() => {
  /** @type {Set<(event: Object) => void>} */
  const handlers = new Set()
  return {
    handlers,
    /** @param {Object} event */
    emit(event) {
      for (const handler of [...handlers]) handler(event)
    },
    backend: {
      /** @param {(event: Object) => void} handler */
      subscribe(handler) {
        handlers.add(handler)
        return () => handlers.delete(handler)
      },
      downloadRuntime: vi.fn(async () => 'started'),
      downloadModel: vi.fn(async () => 'started'),
      cancelDownload: vi.fn(async () => true),
    },
    markFirstLaunchOffered: vi.fn(),
    loadCapabilities: vi.fn(async () => {}),
  }
})

vi.mock('../api/backend.js', () => ({ getBackend: () => stubs.backend }))
vi.mock('../state/session.svelte.js', () => ({
  markFirstLaunchOffered: stubs.markFirstLaunchOffered,
}))
vi.mock('../state/capabilities.svelte.js', () => ({ loadCapabilities: stubs.loadCapabilities }))

// Imported after the mocks so the store and the dialog pick the stubs up.
const { default: FirstLaunchDialog } = await import('./FirstLaunchDialog.svelte')
const { firstLaunch, offerFirstLaunch, resetFirstLaunch } = await import('./firstlaunch.svelte.js')

const RUNTIME_BYTES = 32_396_562

/**
 * The real catalogue with nothing installed, which is a fresh machine.
 *
 * @param {{runtime?: Object, models?: Object}} [overrides]
 */
function view({ runtime = {}, models = {} } = {}) {
  const row = (id, kindKey, bytes, requiredBy) => ({
    id,
    kindKey,
    bytes,
    requiredBy,
    installed: false,
    ...(models[id] ?? {}),
  })
  return {
    models: [
      row('textDetector', 'models.kind.textDetector', 94_669_756, ['autoClean']),
      row('inpainter', 'models.kind.inpainter', 207_482_644, ['lama']),
      row('scriptGate', 'models.kind.scriptGate', 3_722_314, ['autoClean']),
      row('scriptGateLabels', 'models.kind.scriptGateLabels', 1_163, ['autoClean']),
      row('balloonDetector', 'models.kind.balloonDetector', 11_120_765, ['autoClean']),
    ],
    runtime: { installed: false, bytes: RUNTIME_BYTES, available: true, ...runtime },
  }
}

/** Open the offer over a catalogue answer, then mount the dialog on it. */
function open(answer = view()) {
  offerFirstLaunch(/** @type {any} */ (answer))
  return render(FirstLaunchDialog)
}

/** The `done` event every download ends with, whatever happened to it. */
function done(id, error = null) {
  stubs.emit({ type: 'model-progress', id, downloaded: 1, total: 1, done: true, error })
}

/** Press the primary button, whatever total it happens to be promising. */
async function press(getByRole) {
  const plan = firstLaunchPlan(view())
  const bytes = plannedBytes(plan, initialSelection(plan))
  await fireEvent.click(
    getByRole('button', { name: t('models.firstLaunch.action.download', { bytes }) }),
  )
}

/** Whether this id's download has been asked for. @param {string} id */
function started(id) {
  if (id === 'runtime') return stubs.backend.downloadRuntime.mock.calls.length > 0
  return stubs.backend.downloadModel.mock.calls.some(([spec]) => spec.id === id)
}

beforeEach(() => {
  stubs.handlers.clear()
  stubs.backend.downloadRuntime.mockReset().mockResolvedValue('started')
  stubs.backend.downloadModel.mockReset().mockResolvedValue('started')
  stubs.backend.cancelDownload.mockReset().mockResolvedValue(true)
  stubs.markFirstLaunchOffered.mockClear()
  stubs.loadCapabilities.mockClear()
  resetFirstLaunch()
})

afterEach(() => {
  cleanup()
  resetFirstLaunch()
})

describe('the offer as it is drawn', () => {
  it('draws the Auto clean set and the runtime as one required group', () => {
    const { getByText } = open()
    for (const name of [
      'models.kind.textDetector',
      'models.kind.scriptGate',
      'models.kind.scriptGateLabels',
      'models.kind.balloonDetector',
      'settings.models.runtime.label',
    ]) {
      expect(getByText(t(name))).toBeTruthy()
    }
    // The group's own total, summed from the view rather than written down.
    const plan = firstLaunchPlan(view())
    expect(getByText(t('models.firstLaunch.requiredNote', { bytes: plan.requiredBytes }))).toBeTruthy()
  })

  it('draws the redraw engine as a choice, ticked, and nothing else as one', () => {
    const { getByText, getAllByRole } = open()
    expect(getByText(t('models.firstLaunch.optionalLabel'))).toBeTruthy()
    expect(getByText(t('models.kind.inpainter'))).toBeTruthy()
    // One tick and only one: the required group has none, because a file Auto
    // clean cannot run without is not a choice. MI-GAN was the second, and
    // the count is what says the five required rows did not inherit its
    // checkbox when it left.
    const ticks = getAllByRole('checkbox')
    expect(ticks.map((tick) => tick.checked)).toEqual([true])
  })

  it('draws no choice at all when the one redraw engine is already here', () => {
    // The plan's optional group is empty in that case (`firstlaunch.test.js`),
    // and an empty group is drawn as no section rather than as `Redraw
    // engines` over nothing - which is the shape the removal could have left
    // behind, because with two engines the group could never empty.
    const { queryByText, queryAllByRole } = open(view({ models: { inpainter: { installed: true } } }))
    expect(queryByText(t('models.firstLaunch.optionalLabel'))).toBeNull()
    expect(queryByText(t('models.firstLaunch.optionalNote'))).toBeNull()
    expect(queryAllByRole('checkbox')).toEqual([])
    // The offer is still made: the required set is what it is made of.
    expect(queryByText(t('models.kind.textDetector'))).toBeTruthy()
  })

  it('promises what the ticked rows cost, and moves when a tick moves', async () => {
    const { getByRole, getAllByRole } = open()
    const plan = firstLaunchPlan(view())
    const selection = initialSelection(plan)
    const label = (bytes) => t('models.firstLaunch.action.download', { bytes })
    expect(getByRole('button', { name: label(plannedBytes(plan, selection)) })).toBeTruthy()

    // The one tick a user can now move is the one that starts on, so the
    // figure is read falling rather than rising: LaMa's 207 MB comes off the
    // button and the required group's total is what is left.
    await fireEvent.click(getAllByRole('checkbox')[0])
    const withoutLama = plannedBytes(plan, { ...selection, inpainter: false })
    expect(withoutLama).toBeLessThan(plannedBytes(plan, selection))
    expect(getByRole('button', { name: label(withoutLama) })).toBeTruthy()

    // And back on again, so what is asserted is the tick driving the label and
    // not a one-way subtraction.
    await fireEvent.click(getAllByRole('checkbox')[0])
    expect(getByRole('button', { name: label(plannedBytes(plan, selection)) })).toBeTruthy()
  })

  it('says why the weights are not enough where no runtime is published', () => {
    // An Intel Mac. The rows are still offered - they are what an
    // offline install needs beside a library placed by hand - so the sentence
    // is what stops the offer reading as a promise it cannot keep.
    const { getByText, queryByText } = open(view({ runtime: { available: false } }))
    expect(getByText(t('models.firstLaunch.runtimeUnavailable'))).toBeTruthy()
    expect(queryByText(t('settings.models.runtime.label'))).toBeNull()
  })

  it('offers a download whose size the view could not state', async () => {
    // `total` is 0 for a row with no `bytes`, and a button disabled on the
    // total would refuse to fetch the one artefact this machine is missing.
    // The queue is what decides.
    const { getByRole } = open(
      view({
        runtime: { installed: true },
        models: {
          textDetector: { installed: false, bytes: null },
          inpainter: { installed: true },
          scriptGate: { installed: true },
          scriptGateLabels: { installed: true },
          balloonDetector: { installed: true },
        },
      }),
    )
    const button = getByRole('button', {
      name: t('models.firstLaunch.action.download', { bytes: 0 }),
    })
    expect(button.disabled).toBe(false)
    await fireEvent.click(button)
    expect(stubs.backend.downloadModel).toHaveBeenCalledWith({ id: 'textDetector' })
  })
})

describe('the two answers', () => {
  it('remembers that the offer was made when the user says Not now', async () => {
    const { getByRole } = open()
    // The keyboard layer is told about a dialog the modal stack cannot see.
    expect(isDialogOutsideStackOpen()).toBe(true)
    await fireEvent.click(getByRole('button', { name: t('models.firstLaunch.action.notNow') }))
    // The flag records that the user was *asked*, which is why declining sets
    // it: the modal must not come back on the next launch to ask again.
    expect(stubs.markFirstLaunchOffered).toHaveBeenCalledTimes(1)
    expect(firstLaunch.open).toBe(false)
    // And the layer has its table back.
    expect(isDialogOutsideStackOpen()).toBe(false)
    expect(stubs.backend.downloadRuntime).not.toHaveBeenCalled()
    expect(stubs.backend.downloadModel).not.toHaveBeenCalled()
  })

  it('sets the flag on Download too, and fetches the runtime before any weight', async () => {
    const { getByRole } = open()
    await press(getByRole)
    expect(stubs.markFirstLaunchOffered).toHaveBeenCalledTimes(1)

    await vi.waitFor(() => expect(stubs.backend.downloadRuntime).toHaveBeenCalledTimes(1))
    // One at a time: nothing else starts while the runtime is in flight.
    expect(stubs.backend.downloadModel).not.toHaveBeenCalled()

    done('runtime')
    await vi.waitFor(() => expect(stubs.backend.downloadModel).toHaveBeenCalledTimes(1))
    expect(stubs.backend.downloadModel).toHaveBeenLastCalledWith({ id: 'textDetector' })
  })

  it('reports each transfer from the event channel while it runs', async () => {
    const { getByRole, getByText } = open()
    await press(getByRole)
    await vi.waitFor(() => expect(stubs.backend.downloadRuntime).toHaveBeenCalled())

    stubs.emit({
      type: 'model-progress',
      id: 'runtime',
      downloaded: RUNTIME_BYTES / 2,
      total: RUNTIME_BYTES,
      done: false,
      error: null,
    })
    await vi.waitFor(() =>
      expect(getByText(new RegExp(t('settings.models.status.downloadingPercent', { percent: 50 })))).toBeTruthy(),
    )

    done('runtime')
    await vi.waitFor(() =>
      expect(getByText(new RegExp(`${t('settings.models.status.installed')}$`))).toBeTruthy(),
    )
  })

  it('runs the whole queue and then says so', async () => {
    const { getByRole, getByText } = open()
    await press(getByRole)
    for (const id of ['runtime', 'textDetector', 'scriptGate', 'scriptGateLabels', 'balloonDetector', 'inpainter']) {
      await vi.waitFor(() => expect(started(id)).toBe(true))
      done(id)
    }
    await vi.waitFor(() => expect(getByText(t('models.firstLaunch.done'))).toBeTruthy())
    // The queue and nothing beside it: five weights, each asked for once. The
    // press fetches what the ticks named, not what the catalogue holds.
    expect(stubs.backend.downloadModel.mock.calls.map(([spec]) => spec.id)).toEqual([
      'textDetector',
      'scriptGate',
      'scriptGateLabels',
      'balloonDetector',
      'inpainter',
    ])
    // What can run has changed, and the editor's pickers read that store.
    expect(stubs.loadCapabilities).toHaveBeenCalled()
  })

  it('never asks for a redraw engine the user unticked', async () => {
    // The half of the offer that is a *choice*, taken the other way. It used
    // to be read off MI-GAN's untouched checkbox; with one engine left the
    // only way to reach an unticked optional row is to untick it, so the
    // gesture is now part of what is asserted.
    const { getByRole, getByText, getAllByRole } = open()
    await fireEvent.click(getAllByRole('checkbox')[0])

    const plan = firstLaunchPlan(view())
    const bytes = plannedBytes(plan, { ...initialSelection(plan), inpainter: false })
    await fireEvent.click(
      getByRole('button', { name: t('models.firstLaunch.action.download', { bytes }) }),
    )

    for (const id of ['runtime', 'textDetector', 'scriptGate', 'scriptGateLabels', 'balloonDetector']) {
      await vi.waitFor(() => expect(started(id)).toBe(true))
      done(id)
    }
    await vi.waitFor(() => expect(getByText(t('models.firstLaunch.done'))).toBeTruthy())
    expect(stubs.backend.downloadModel).not.toHaveBeenCalledWith({ id: 'inpainter' })
  })
})

describe('the orderings a download sequence lives or dies by', () => {
  it('hears a `done` that arrives before the call that started it answers', async () => {
    // A cached artefact verifies in microseconds, so the event can beat the
    // `invoke` reply. A waiter registered after the call would wait for an
    // event that has already been and gone, and the sequence would stop dead.
    stubs.backend.downloadRuntime.mockImplementation(async () => {
      done('runtime')
      return 'started'
    })
    const { getByRole } = open()
    await press(getByRole)
    await vi.waitFor(() => expect(started('textDetector')).toBe(true))
  })

  it('waits for a transfer another window had already started', async () => {
    // `alreadyRunning` means this very artefact is in flight elsewhere and will
    // end with the same single `done` event. Moving on without waiting is the
    // parallelism the sequence exists to avoid.
    stubs.backend.downloadModel.mockResolvedValueOnce('alreadyRunning')
    const { getByRole } = open()
    await press(getByRole)
    await vi.waitFor(() => expect(started('runtime')).toBe(true))
    done('runtime')
    await vi.waitFor(() => expect(started('textDetector')).toBe(true))

    // Nothing after it, until the other window's transfer ends. Given a run of
    // turns to get it wrong in: a sequence that treated `alreadyRunning` as a
    // reason to move on would have asked for the next weight by now.
    for (let turn = 0; turn < 5; turn += 1) await new Promise((r) => setTimeout(r, 0))
    expect(stubs.backend.downloadModel).toHaveBeenCalledTimes(1)
    done('textDetector')
    await vi.waitFor(() => expect(started('scriptGate')).toBe(true))
  })

  it('carries on past a row another window already installed', async () => {
    stubs.backend.downloadRuntime.mockResolvedValue('alreadyInstalled')
    const { getByRole } = open()
    await press(getByRole)
    // No `done` event will ever arrive for a download that did not start, so a
    // sequence that waited for one would stop here forever.
    await vi.waitFor(() => expect(stubs.backend.downloadModel).toHaveBeenCalledTimes(1))
    expect(stubs.backend.downloadModel).toHaveBeenLastCalledWith({ id: 'textDetector' })
  })

  it('honours a Cancel pressed before the download had begun', async () => {
    // The window between asking for a download and being told it started:
    // `cancelDownload` for an id the backend has not begun answers `false` and
    // is lost, so the press is remembered and re-sent once there is a transfer
    // to stop.
    /** @type {(outcome: string) => void} */
    let startAnswers = () => {}
    stubs.backend.downloadRuntime.mockImplementation(
      () => new Promise((resolve) => { startAnswers = resolve }),
    )
    const { getByRole, getByText } = open()
    await press(getByRole)

    await fireEvent.click(getByRole('button', { name: t('settings.models.action.cancel') }))
    startAnswers('started')

    await vi.waitFor(() => expect(stubs.backend.cancelDownload.mock.calls.length).toBeGreaterThan(1))
    expect(stubs.backend.cancelDownload).toHaveBeenLastCalledWith({ id: 'runtime' })

    // A cancellation ends through the same `done` event a failure does, and is
    // deliberately not reported as one.
    done('runtime', 'cancelled')
    await vi.waitFor(() => expect(getByText(t('models.firstLaunch.stopped'))).toBeTruthy())
    expect(stubs.backend.downloadModel).not.toHaveBeenCalled()
  })
})

describe('when a download does not arrive', () => {
  it('stops the sequence and names the artefact that failed', async () => {
    const { getByRole, getByText } = open()
    await press(getByRole)
    await vi.waitFor(() => expect(stubs.backend.downloadRuntime).toHaveBeenCalled())

    done('runtime', 'connection reset')
    await vi.waitFor(() =>
      expect(
        getByText(t('models.firstLaunch.failed', { nameKey: 'settings.models.runtime.label' })),
      ).toBeTruthy(),
    )
    // The error itself is shown as the backend wrote it, the way Settings
    // shows one: it is the only thing on screen that says what went wrong.
    expect(getByText('connection reset')).toBeTruthy()
    // And nothing after it was started - four more failures would read as a
    // broken application rather than as one bad connection.
    expect(stubs.backend.downloadModel).not.toHaveBeenCalled()
  })

  it('stops promising bytes that have already arrived, and resumes where it stopped', async () => {
    const { getByRole } = open()
    await press(getByRole)
    await vi.waitFor(() => expect(stubs.backend.downloadRuntime).toHaveBeenCalledTimes(1))
    done('runtime')
    await vi.waitFor(() => expect(started('textDetector')).toBe(true))
    await fireEvent.click(getByRole('button', { name: t('settings.models.action.cancel') }))
    done('textDetector', 'cancelled')

    // The plan is the boot snapshot and still calls the runtime missing; the
    // button must not quote a price for what is already on disk.
    const plan = firstLaunchPlan(view())
    const left = plannedBytes(plan, { ...initialSelection(plan), runtime: false })
    const again = await vi.waitFor(() =>
      getByRole('button', { name: t('models.firstLaunch.action.download', { bytes: left }) }),
    )

    // And the second press starts where the first one stopped.
    await fireEvent.click(again)
    await vi.waitFor(() => expect(stubs.backend.downloadModel).toHaveBeenCalledTimes(2))
    expect(stubs.backend.downloadRuntime).toHaveBeenCalledTimes(1)
  })
})

describe('a dialog raised over the offer', () => {
  it('does not take the run with it when the offer leaves the screen', async () => {
    // `App.svelte` unmounts the offer while something is on the modal stack.
    // The sequence is in the store, so the transfer carries on being reported
    // and the dialog comes back as it was rather than rebuilt from the boot
    // snapshot with its ticks reset.
    const { getByRole } = open()
    await press(getByRole)
    await vi.waitFor(() => expect(started('runtime')).toBe(true))

    cleanup()
    done('runtime')
    await vi.waitFor(() => expect(started('textDetector')).toBe(true))

    const back = render(FirstLaunchDialog)
    expect(back.getByRole('button', { name: t('settings.models.action.cancel') })).toBeTruthy()
    // The runtime arrived while nothing was drawing it, and the row says so.
    const runtimeRow = back.getByText(t('settings.models.runtime.label')).closest('li')
    expect(runtimeRow?.textContent).toContain(t('settings.models.status.installed'))
  })
})
