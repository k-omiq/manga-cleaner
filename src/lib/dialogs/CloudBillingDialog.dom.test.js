import { afterEach, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/svelte'
import CloudBillingDialog from './CloudBillingDialog.svelte'
import { t } from '../i18n/index.js'

const mocks = vi.hoisted(() => ({ storeCloudSecret: vi.fn(), refreshBilling: vi.fn(), closeModal: vi.fn() }))
vi.mock('../api/backend.js', () => ({ getBackend: () => mocks }))
vi.mock('../state/billing.svelte.js', () => ({ refreshBilling: mocks.refreshBilling }))
vi.mock('../state/app.svelte.js', () => ({ closeModal: mocks.closeModal }))
afterEach(() => { cleanup(); vi.resetAllMocks() })

it('keeps a failed billing connection open and clears the entered secret', async () => {
  mocks.storeCloudSecret.mockResolvedValue({})
  mocks.refreshBilling.mockResolvedValue(false)
  render(CloudBillingDialog, { spec: { props: { profileId: 'studio' } } })
  await fireEvent.input(screen.getByLabelText(t('settings.cloud.usage.tokenId')), { target: { value: 'ak-test' } })
  const secret = screen.getByLabelText(t('settings.cloud.usage.tokenSecret'))
  await fireEvent.input(secret, { target: { value: 'as-test' } })
  await fireEvent.submit(secret.closest('form'))
  await screen.findByRole('alert')
  expect(mocks.closeModal).not.toHaveBeenCalled()
  expect(secret.value).toBe('')
  expect(mocks.storeCloudSecret).toHaveBeenCalledWith({ provider: 'modal', profileId: 'studio', role: 'setup', tokenId: 'ak-test', secret: 'as-test', sessionOnly: true })
})

it('closes only after the stored credential can read billing', async () => {
  mocks.storeCloudSecret.mockResolvedValue({})
  mocks.refreshBilling.mockResolvedValue(true)
  render(CloudBillingDialog, { spec: { props: { profileId: 'studio' } } })
  await fireEvent.input(screen.getByLabelText(t('settings.cloud.usage.tokenId')), { target: { value: 'ak-test' } })
  const secret = screen.getByLabelText(t('settings.cloud.usage.tokenSecret'))
  await fireEvent.input(secret, { target: { value: 'as-test' } })
  await fireEvent.submit(secret.closest('form'))
  await waitFor(() => expect(mocks.closeModal).toHaveBeenCalledWith('connected'))
  expect(secret.value).toBe('')
})
