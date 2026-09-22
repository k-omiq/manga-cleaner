import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/svelte'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { setBackend } from '../api/backend.js'
import InferenceSettings, {
  isOriginChanged,
  isValidEndpointUrl,
  isValidProfileId,
  isValidProfileName,
} from './InferenceSettings.svelte'

describe('InferenceSettings helper validations', () => {
  it('validates profile IDs correctly', () => {
    expect(isValidProfileId('modal-prod-1')).toBe(true)
    expect(isValidProfileId('beam_v2')).toBe(true)
    expect(isValidProfileId('profile123')).toBe(true)
    expect(isValidProfileId('a')).toBe(true)
    expect(isValidProfileId('A'.repeat(64))).toBe(true)

    expect(isValidProfileId('')).toBe(false)
    expect(isValidProfileId(' modal-prod ')).toBe(false)
    expect(isValidProfileId('-invalid-start')).toBe(false)
    expect(isValidProfileId('_invalid-start')).toBe(false)
    expect(isValidProfileId('has.dot')).toBe(false)
    expect(isValidProfileId('has/slash')).toBe(false)
    expect(isValidProfileId('has space')).toBe(false)
    expect(isValidProfileId('A'.repeat(65))).toBe(false)
  })

  it('validates profile names correctly', () => {
    expect(isValidProfileName('Production GPU')).toBe(true)
    expect(isValidProfileName('Worker-1')).toBe(true)
    expect(isValidProfileName('a')).toBe(true)
    expect(isValidProfileName('N'.repeat(128))).toBe(true)

    expect(isValidProfileName('')).toBe(false)
    expect(isValidProfileName(' leading-space')).toBe(false)
    expect(isValidProfileName('trailing-space ')).toBe(false)
    expect(isValidProfileName('control\nchar')).toBe(false)
    expect(isValidProfileName('N'.repeat(129))).toBe(false)
  })

  it('validates HTTPS endpoint URLs correctly', () => {
    expect(isValidEndpointUrl('https://modal-cleaner.run.modal.com/mc/v1')).toBe(true)
    expect(isValidEndpointUrl('https://api.beam.cloud:8443/endpoint')).toBe(true)
    expect(isValidEndpointUrl('https://gpu-worker.internal-cloud.org/v1/infer')).toBe(true)

    expect(isValidEndpointUrl('')).toBe(false)
    expect(isValidEndpointUrl('http://insecure.example.com')).toBe(false)
    expect(isValidEndpointUrl('  https://api.beam.cloud/ep')).toBe(false)
    expect(isValidEndpointUrl('https://api.beam.cloud/ep  ')).toBe(false)
    expect(isValidEndpointUrl('https://user:pass@api.modal.com/mc/v1')).toBe(false)
    expect(isValidEndpointUrl('https://api.modal.com/mc/v1?query=1')).toBe(false)
    expect(isValidEndpointUrl('https://api.modal.com/mc/v1#fragment')).toBe(false)
    expect(isValidEndpointUrl('https://localhost:8080')).toBe(false)
    expect(isValidEndpointUrl('https://mybox.local/ep')).toBe(false)
    expect(isValidEndpointUrl('https://internal.internal/ep')).toBe(false)
    expect(isValidEndpointUrl('https://127.0.0.1:443/ep')).toBe(false)
    expect(isValidEndpointUrl('https://[::1]:8443/ep')).toBe(false)
    expect(isValidEndpointUrl('https://2130706433/')).toBe(false)
  })

  it('detects origin changes across endpoint URLs', () => {
    expect(
      isOriginChanged(
        'https://modal-prod.run.modal.com/mc/v1',
        'https://modal-prod.run.modal.com/mc/v2',
      ),
    ).toBe(false)
    expect(
      isOriginChanged(
        'https://modal-prod.run.modal.com/mc/v1',
        'https://modal-dev.run.modal.com/mc/v1',
      ),
    ).toBe(true)
    expect(
      isOriginChanged(
        'https://api.beam.cloud/v1',
        'https://api.beam.cloud:8443/v1',
      ),
    ).toBe(true)
  })
})

describe('InferenceSettings Component', () => {
  let mockBackend

  const defaultMockConfig = {
    schemaVersion: 1,
    selectedTarget: { type: 'local' },
    beamProfiles: {},
    modalProfiles: {},
  }

  const populatedMockConfig = {
    schemaVersion: 1,
    selectedTarget: { type: 'modal', profile_id: 'modal-prod' },
    beamProfiles: {
      'beam-prod': {
        id: 'beam-prod',
        name: 'Beam Production',
        endpointUrl: 'https://api.beam.cloud/v1/clean',
        canonicalOrigin: 'https://api.beam.cloud',
        canonicalOriginFingerprint: 'beamfp123',
        createdAtMs: 1000,
        updatedAtMs: 1000,
      },
    },
    modalProfiles: {
      'modal-prod': {
        id: 'modal-prod',
        name: 'Modal Production',
        endpointUrl: 'https://modal-cleaner.run.modal.com/mc/v1',
        canonicalOrigin: 'https://modal-cleaner.run.modal.com',
        canonicalOriginFingerprint: 'modalfp123',
        createdAtMs: 2000,
        updatedAtMs: 2000,
      },
    },
  }

  beforeEach(() => {
    mockBackend = {
      readInferenceConfig: vi.fn().mockResolvedValue(structuredClone(defaultMockConfig)),
      writeInferenceConfig: vi.fn().mockImplementation(async ({ config }) => structuredClone(config)),
      storeCloudSecret: vi.fn().mockRejectedValue(new Error('no secret calls')),
      deleteCloudSecret: vi.fn().mockRejectedValue(new Error('no secret calls')),
      getCloudSecretSummary: vi.fn().mockRejectedValue(new Error('no secret calls')),
      checkCloudConnection: vi.fn().mockResolvedValue({
        ok: true,
        status: 'reachable',
        provider: 'modal',
        profileId: 'modal-prod',
        latencyMs: 38,
      }),
      reconcileCloudRecovery: vi.fn().mockResolvedValue({ decision: 'terminal' }),
    }
    setBackend(mockBackend)
  })

  afterEach(() => {
    cleanup()
    setBackend(null)
    vi.clearAllMocks()
  })

  it('loads configuration on mount and displays remote execution disclaimer', async () => {
    const { container } = render(InferenceSettings)

    await waitFor(() => {
      expect(mockBackend.readInferenceConfig).toHaveBeenCalledTimes(1)
    })

    const statusBox = container.querySelector('.disclaimer-box')
    expect(statusBox).toBeTruthy()
    expect(statusBox.textContent).toContain('Remote execution is unavailable in this build.')
    await waitFor(() => expect(container.textContent).toContain('Default execution target'))
    expect(container.textContent).toContain('Modal profiles')
    expect(container.textContent).toContain('Beam profiles')
  })

  it('renders both provider profiles simultaneously when populated', async () => {
    mockBackend.readInferenceConfig.mockResolvedValueOnce(structuredClone(populatedMockConfig))
    const { container } = render(InferenceSettings)

    await waitFor(() => {
      const rowNames = Array.from(container.querySelectorAll('.row-name')).map((el) => el.textContent)
      expect(rowNames.some((n) => n.includes('Modal Production'))).toBe(true)
      expect(rowNames.some((n) => n.includes('Beam Production'))).toBe(true)
    })

    const profileIds = Array.from(container.querySelectorAll('.profile-id')).map((el) => el.textContent)
    expect(profileIds).toContain('(modal-prod)')
    expect(profileIds).toContain('(beam-prod)')

    const rowMetas = Array.from(container.querySelectorAll('.row-meta')).map((el) => el.textContent)
    expect(rowMetas).toContain('https://modal-cleaner.run.modal.com/mc/v1')
    expect(rowMetas).toContain('https://api.beam.cloud/v1/clean')
  })

  it('switches execution target and persists both providers', async () => {
    mockBackend.readInferenceConfig.mockResolvedValueOnce(structuredClone(populatedMockConfig))
    const { container } = render(InferenceSettings)

    await waitFor(() => {
      expect(container.querySelector('.profile-id')?.textContent).toBe('(modal-prod)')
    })

    const select = container.querySelector('select.select')
    expect(select).toBeTruthy()

    // Switch to Beam target
    await fireEvent.change(select, { target: { value: 'beam:beam-prod' } })

    await waitFor(() => {
      expect(mockBackend.writeInferenceConfig).toHaveBeenCalledWith({
        config: expect.objectContaining({
          schemaVersion: 1,
          selectedTarget: { type: 'beam', profile_id: 'beam-prod' },
          modalProfiles: expect.objectContaining({ 'modal-prod': expect.any(Object) }),
          beamProfiles: expect.objectContaining({ 'beam-prod': expect.any(Object) }),
        }),
      })
    })

    // Wait for the first save to settle before the next user action.
    await waitFor(() => {
      expect(select.disabled).toBe(false)
      expect(select.value).toBe('beam:beam-prod')
    })
    // Switch to Local target
    await fireEvent.change(select, { target: { value: 'local' } })

    await waitFor(() => {
      expect(mockBackend.writeInferenceConfig).toHaveBeenCalledWith({
        config: expect.objectContaining({
          schemaVersion: 1,
          selectedTarget: { type: 'local' },
          modalProfiles: expect.objectContaining({ 'modal-prod': expect.any(Object) }),
          beamProfiles: expect.objectContaining({ 'beam-prod': expect.any(Object) }),
        }),
      })
    })
  })

  it('adds a new profile preserving existing profiles and selectedTarget', async () => {
    mockBackend.readInferenceConfig.mockResolvedValueOnce(structuredClone(populatedMockConfig))
    const { container } = render(InferenceSettings)

    await waitFor(() => {
      expect(container.querySelector('.profile-id')?.textContent).toBe('(modal-prod)')
    })

    const addButtons = screen.getAllByRole('button', { name: 'Add profile' })
    await fireEvent.click(addButtons[0]) // Modal add profile

    const idInput = screen.getByPlaceholderText('e.g. modal-prod-1')
    const nameInput = screen.getByPlaceholderText('e.g. Production GPU Worker')
    const endpointInput = screen.getByPlaceholderText('https://…')

    await fireEvent.input(idInput, { target: { value: 'modal-staging' } })
    await fireEvent.input(nameInput, { target: { value: 'Modal Staging' } })
    await fireEvent.input(endpointInput, {
      target: { value: 'https://modal-staging.run.modal.com/mc/v1' },
    })

    const saveButton = screen.getByRole('button', { name: 'Save profile' })
    await fireEvent.click(saveButton)

    await waitFor(() => {
      expect(mockBackend.writeInferenceConfig).toHaveBeenCalledWith({
        config: expect.objectContaining({
          schemaVersion: 1,
          selectedTarget: { type: 'modal', profile_id: 'modal-prod' },
          beamProfiles: expect.objectContaining({ 'beam-prod': expect.any(Object) }),
          modalProfiles: expect.objectContaining({
            'modal-prod': expect.any(Object),
            'modal-staging': expect.objectContaining({
              id: 'modal-staging',
              name: 'Modal Staging',
              endpointUrl: 'https://modal-staging.run.modal.com/mc/v1',
            }),
          }),
        }),
      })
    })
  })

  it('validates profile input and refuses invalid fields', async () => {
    mockBackend.readInferenceConfig.mockResolvedValueOnce(structuredClone(populatedMockConfig))
    const { container } = render(InferenceSettings)

    await waitFor(() => {
      expect(container.querySelector('.profile-id')?.textContent).toBe('(modal-prod)')
    })

    const addButtons = screen.getAllByRole('button', { name: 'Add profile' })
    await fireEvent.click(addButtons[0])

    const idInput = screen.getByPlaceholderText('e.g. modal-prod-1')
    const nameInput = screen.getByPlaceholderText('e.g. Production GPU Worker')
    const endpointInput = screen.getByPlaceholderText('https://…')
    const saveButton = screen.getByRole('button', { name: 'Save profile' })

    // Invalid ID: leading hyphen
    await fireEvent.input(idInput, { target: { value: '-invalid-id' } })
    await fireEvent.input(nameInput, { target: { value: 'Valid Name' } })
    await fireEvent.input(endpointInput, { target: { value: 'https://api.modal.com/v1' } })
    await fireEvent.click(saveButton)

    const alertBox = container.querySelector('.form-error')
    expect(alertBox).toBeTruthy()
    expect(alertBox.textContent).toContain('Profile ID must be 1 to 64 ASCII')
    expect(mockBackend.writeInferenceConfig).not.toHaveBeenCalled()

    // Duplicate ID
    await fireEvent.input(idInput, { target: { value: 'modal-prod' } })
    await fireEvent.click(saveButton)
    expect(alertBox.textContent).toContain('A profile with this ID already exists')
    expect(mockBackend.writeInferenceConfig).not.toHaveBeenCalled()

    // Invalid URL (insecure http)
    await fireEvent.input(idInput, { target: { value: 'modal-new' } })
    await fireEvent.input(endpointInput, { target: { value: 'http://insecure.modal.com/v1' } })
    await fireEvent.click(saveButton)
    expect(alertBox.textContent).toContain('Endpoint URL must be a valid HTTPS URL')
    expect(mockBackend.writeInferenceConfig).not.toHaveBeenCalled()
  })

  it('displays warning when editing an endpoint URL with a new origin', async () => {
    mockBackend.readInferenceConfig.mockResolvedValueOnce(structuredClone(populatedMockConfig))
    const { container } = render(InferenceSettings)

    await waitFor(() => {
      expect(container.querySelector('.profile-id')?.textContent).toBe('(modal-prod)')
    })

    const editButtons = screen.getAllByRole('button', { name: /^Edit / })
    await fireEvent.click(editButtons[0])

    const endpointInput = screen.getByPlaceholderText('https://…')
    await fireEvent.input(endpointInput, {
      target: { value: 'https://new-origin.modal.com/mc/v1' },
    })

    const warningBox = container.querySelector('.warning-note')
    expect(warningBox).toBeTruthy()
    expect(warningBox.textContent).toContain('Changing the endpoint URL changes the target origin.')

    const saveButton = screen.getByRole('button', { name: 'Save profile' })
    await fireEvent.click(saveButton)

    await waitFor(() => {
      expect(mockBackend.writeInferenceConfig).toHaveBeenCalledWith({
        config: expect.objectContaining({
          modalProfiles: expect.objectContaining({
            'modal-prod': expect.objectContaining({
              id: 'modal-prod',
              endpointUrl: 'https://new-origin.modal.com/mc/v1',
            }),
          }),
        }),
      })
    })
  })

  it('resets selectedTarget to local when deleting the currently selected profile', async () => {
    mockBackend.readInferenceConfig.mockResolvedValueOnce(structuredClone(populatedMockConfig))
    const { container } = render(InferenceSettings)

    await waitFor(() => {
      expect(container.querySelector('.profile-id')?.textContent).toBe('(modal-prod)')
    })

    const deleteButtons = screen.getAllByRole('button', { name: /^Delete / })
    // Delete modal-prod, which is currently selectedTarget
    await fireEvent.click(deleteButtons[0])

    await waitFor(() => {
      expect(mockBackend.writeInferenceConfig).toHaveBeenCalledWith({
        config: expect.objectContaining({
          schemaVersion: 1,
          selectedTarget: { type: 'local' },
          modalProfiles: {},
          beamProfiles: expect.objectContaining({ 'beam-prod': expect.any(Object) }),
        }),
      })
    })
  })

  it('preserves selectedTarget when deleting an unselected profile', async () => {
    mockBackend.readInferenceConfig.mockResolvedValueOnce(structuredClone(populatedMockConfig))
    const { container } = render(InferenceSettings)

    await waitFor(() => {
      expect(container.querySelectorAll('.row-name').length).toBe(2)
    })

    const deleteButtons = screen.getAllByRole('button', { name: /^Delete / })
    // Delete beam-prod (modal-prod is the selected target)
    await fireEvent.click(deleteButtons[1])

    await waitFor(() => {
      expect(mockBackend.writeInferenceConfig).toHaveBeenCalledWith({
        config: expect.objectContaining({
          schemaVersion: 1,
          selectedTarget: { type: 'modal', profile_id: 'modal-prod' },
          beamProfiles: {},
          modalProfiles: expect.objectContaining({ 'modal-prod': expect.any(Object) }),
        }),
      })
    })
  })

  it('handles read rejection then successful retry and write rejection with selector restoration', async () => {
    // 1. Initial read rejection
    mockBackend.readInferenceConfig.mockRejectedValueOnce(new Error('sensitive-connection-url-error'))
    const { container } = render(InferenceSettings)

    await waitFor(() => {
      const errorBox = container.querySelector('.status-banner.error')
      expect(errorBox).toBeTruthy()
      expect(errorBox.textContent).toContain('Failed to load inference configuration.')
    })
    expect(container.textContent).not.toContain('sensitive-connection-url-error')

    // 2. Successful retry
    mockBackend.readInferenceConfig.mockResolvedValueOnce(structuredClone(populatedMockConfig))
    const retryButton = screen.getByRole('button', { name: 'Try again' })
    await fireEvent.click(retryButton)

    await waitFor(() => {
      expect(container.querySelector('.profile-id')?.textContent).toBe('(modal-prod)')
    })

    const select = container.querySelector('select.select')
    expect(select).toBeTruthy()
    expect(select.value).toBe('modal:modal-prod')

    // 3. Write target change rejection and verify selector DOM restoration
    mockBackend.writeInferenceConfig.mockRejectedValueOnce(new Error('secret-write-token-failure'))
    await fireEvent.change(select, { target: { value: 'beam:beam-prod' } })

    await waitFor(() => {
      const errorBox = container.querySelector('.status-banner.error')
      expect(errorBox).toBeTruthy()
      expect(errorBox.textContent).toContain('Failed to save inference configuration.')
    })
    expect(container.textContent).not.toContain('secret-write-token-failure')

    // Verify rendered select.value in the DOM reverted back to persisted value ('modal:modal-prod')
    const restoredSelect = container.querySelector('select.select')
    expect(restoredSelect.value).toBe('modal:modal-prod')
  })

  it('performs zero secret operations and mounts no credential fields', async () => {
    mockBackend.readInferenceConfig.mockResolvedValueOnce(structuredClone(populatedMockConfig))
    const { container } = render(InferenceSettings)

    await waitFor(() => {
      expect(container.querySelector('.profile-id')?.textContent).toBe('(modal-prod)')
    })

    expect(mockBackend.storeCloudSecret).not.toHaveBeenCalled()
    expect(mockBackend.deleteCloudSecret).not.toHaveBeenCalled()
    expect(mockBackend.getCloudSecretSummary).not.toHaveBeenCalled()

    // Ensure no password or token inputs exist
    expect(container.querySelectorAll('input[type="password"]').length).toBe(0)
    expect(container.textContent).not.toContain('API Key')
    expect(container.textContent).not.toContain('Bearer Token')
  })

  it('tests profile connection and displays accessible latency badge without triggering GPU work', async () => {
    mockBackend.readInferenceConfig.mockResolvedValueOnce(structuredClone(populatedMockConfig))
    const { container } = render(InferenceSettings)

    await waitFor(() => {
      expect(container.querySelector('.profile-id')?.textContent).toBe('(modal-prod)')
    })

    const testButtons = screen.getAllByRole('button', { name: /Test connection for Modal Production/ })
    expect(testButtons.length).toBeGreaterThan(0)

    await fireEvent.click(testButtons[0])

    expect(mockBackend.checkCloudConnection).toHaveBeenCalledWith({
      provider: 'modal',
      profileId: 'modal-prod',
    })

    await waitFor(() => {
      const statusEl = container.querySelector('.status-indicator.ok')
      expect(statusEl).toBeTruthy()
      expect(statusEl.textContent).toContain('Reachable (38 ms)')
    })
  })

  it('handles connection test failure or unregistered command gracefully', async () => {
    mockBackend.readInferenceConfig.mockResolvedValueOnce(structuredClone(populatedMockConfig))
    mockBackend.checkCloudConnection.mockRejectedValueOnce(new Error('Tauri command not registered'))
    const { container } = render(InferenceSettings)

    await waitFor(() => {
      expect(container.querySelector('.profile-id')?.textContent).toBe('(modal-prod)')
    })

    const testButtons = screen.getAllByRole('button', { name: /Test connection for Modal Production/ })
    await fireEvent.click(testButtons[0])

    await waitFor(() => {
      const statusEl = container.querySelector('.status-indicator.unavail')
      expect(statusEl).toBeTruthy()
      expect(statusEl.textContent).toContain('Connection check command not registered in this build')
    })
  })

  it('reconciles recovery status and truthfully displays interrupted attempt states', async () => {
    mockBackend.readInferenceConfig.mockResolvedValueOnce(structuredClone(populatedMockConfig))
    mockBackend.reconcileCloudRecovery.mockResolvedValueOnce({ decision: 'ambiguous_unknown' })
    const { container } = render(InferenceSettings)

    await waitFor(() => {
      expect(container.querySelector('.profile-id')?.textContent).toBe('(modal-prod)')
    })

    const checkRecoveryButton = screen.getByRole('button', { name: 'Check recovery' })
    expect(checkRecoveryButton).toBeTruthy()

    await fireEvent.click(checkRecoveryButton)

    await waitFor(() => {
      const recoveryBox = container.querySelector('.recovery-status-box')
      expect(recoveryBox).toBeTruthy()
      expect(recoveryBox.textContent).toContain('Interrupted attempt detected (status unknown). Automatic re-dispatch is forbidden.')
    })

    // Test cached result state
    mockBackend.reconcileCloudRecovery.mockResolvedValueOnce({ decision: 'result_cached_ready' })
    await fireEvent.click(checkRecoveryButton)

    await waitFor(() => {
      const recoveryBox = container.querySelector('.recovery-status-box')
      expect(recoveryBox.textContent).toContain('Validated remote result cached and ready for local project review.')
    })

    // Test stale attachment state
    mockBackend.reconcileCloudRecovery.mockResolvedValueOnce({ decision: 'stale_attachment' })
    await fireEvent.click(checkRecoveryButton)

    await waitFor(() => {
      const recoveryBox = container.querySelector('.recovery-status-box')
      expect(recoveryBox.textContent).toContain('Interrupted attempt result retained in cache; region was modified locally (stale attachment rejected).')
    })
  })

  it('truthfully displays remote execution unavailable status in disclaimer box', async () => {
    mockBackend.readInferenceConfig.mockResolvedValueOnce(structuredClone(populatedMockConfig))
    const { container } = render(InferenceSettings)

    await waitFor(() => {
      expect(container.querySelector('.profile-id')?.textContent).toBe('(modal-prod)')
    })

    const statusText = container.querySelector('.execution-status-text')
    expect(statusText).toBeTruthy()
    expect(statusText.textContent).toContain('Remote execution:')
    expect(statusText.textContent).toContain('Unavailable (provider execution is not enabled in this build)')
  })
})
