import { afterEach, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render, waitFor } from '@testing-library/svelte'
import { setBackend } from '../api/backend.js'
import { session, setModelAccelerator } from '../state/session.svelte.js'
import SettingsDialog from './SettingsDialog.svelte'

const SPEC = {
  id: 'settings-test', kind: 'settings', titleKey: 'modal.title.settings', props: {},
  actions: [], blocking: false, dismissable: true, onresolve: null,
}

afterEach(() => {
  cleanup()
  setBackend(null)
  setModelAccelerator('samTs', 'inherit')
})

it('shows the native model capability matrix and persists a backend choice for the next session', async () => {
  const writeSettings = vi.fn(async () => ({}))
  const accelerators = {
    preference: 'auto',
    providers: [
      { id: 'cpu', labelKey: 'accel.cpu', available: true, measured: true, active: true, selected: false },
      { id: 'webgpu', labelKey: 'accel.webgpu', available: true, measured: false, active: false, selected: false },
      { id: 'cuda', labelKey: 'accel.cuda', available: false, measured: false, active: false, selected: false },
    ],
    models: [{
      id: 'samTs', modelName: 'SAM-TS-L', modelKey: 'models.kind.samTs', preference: 'auto',
      supportedIds: ['cpu', 'webgpu'], acceleratorId: 'cpu', labelKey: 'accel.cpu',
      noteKey: null, declinedKey: null, declinedId: null, neededBytes: null, roomBytes: null,
      backendStatus: [
        { id: 'cpu', supported: true, installed: true, available: true, verified: true, reasonKey: null },
        { id: 'webgpu', supported: true, installed: true, available: true, verified: false, reasonKey: null },
        { id: 'cuda', supported: false, installed: false, available: false, verified: false, reasonKey: null },
      ],
    }],
  }
  setBackend(/** @type {any} */ ({
    listModels: vi.fn(async () => null),
    listAccelerators: vi.fn(async () => accelerators),
    listSidecarModels: vi.fn(async () => []),
    sidecarAvailable: vi.fn(async () => ({ available: false, reasonKey: null })),
    about: vi.fn(async () => ({ appVersion: 'test', facts: [] })),
    subscribe: vi.fn(() => () => {}),
    writeSettings,
    readInferenceConfig: vi.fn(async () => ({ schemaVersion: 1, selectedTarget: { type: 'local' }, beamProfiles: {}, modalProfiles: {} })),
  }))

  const rendered = render(SettingsDialog, { props: { spec: SPEC } })
  await fireEvent.click(rendered.getByRole('tab', { name: 'Performance' }))
  const picker = /** @type {HTMLSelectElement} */ (await rendered.findByRole('combobox', { name: 'Local backend for SAM-TS-L' }))
  const cuda = /** @type {HTMLOptionElement} */ (picker.querySelector('option[value="cuda"]'))
  expect(cuda.disabled).toBe(true)
  expect(picker.textContent).toContain('WebGPU · available; execution has not been verified')
  expect(rendered.getByText(/Cloud GPU is available through explicit Review analysis only/)).not.toBeNull()

  await fireEvent.change(picker, { target: { value: 'webgpu' } })
  await waitFor(() => expect(writeSettings).toHaveBeenCalledWith(expect.objectContaining({
    modelAccelerators: { samTs: 'webgpu' },
  })))
  expect(session.modelAccelerators.samTs).toBe('webgpu')
})
