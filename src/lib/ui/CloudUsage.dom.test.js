import { afterEach, it, expect, vi } from 'vitest'
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/svelte'
import CloudUsage from './CloudUsage.svelte'
import { session } from '../state/session.svelte.js'
import { cloud } from '../state/cloud.svelte.js'
import { billing } from '../state/billing.svelte.js'
const mocks = vi.hoisted(() => ({ getCloudUsage: vi.fn(), getCloudSecretSummary: vi.fn(), getCloudBilling: vi.fn(), deleteCloudSecret: vi.fn() }))
vi.mock('../api/backend.js', async (original) => ({ ...(await original()), getBackend: () => mocks }))
afterEach(() => { cleanup(); session.cloudAllowed = false; cloud.readiness.target = null; cloud.readiness.configured = false; billing.data = null; vi.clearAllMocks() })
it('shows monthly and session cost above expandable request details', async () => {
  session.cloudAllowed = true
  mocks.getCloudUsage.mockResolvedValue({month:{attempts:3,reportedUsd:.23,unpricedAttempts:1},session:{attempts:1,reportedUsd:0,unpricedAttempts:1},unreadableAttempts:0})
  render(CloudUsage)
  await screen.findByText('$0.23 + ?')
  expect(screen.getByText('Not reported')).toBeTruthy()
  await fireEvent.click(screen.getByRole('button', {name:'Cloud cost'}))
  expect(screen.getByText('3 requests this month · 1 this session')).toBeTruthy()
  expect(screen.getByText('Price not reported: 1 this month · 1 this session.')).toBeTruthy()
})
it('retries billing with a stored setup credential for a Modal target', async () => {
  session.cloudAllowed = true
  cloud.readiness.target = {type:'modal',profile_id:'mc-test'}
  mocks.getCloudUsage.mockResolvedValue({month:{attempts:0,reportedUsd:0,unpricedAttempts:0},session:{attempts:0,reportedUsd:0,unpricedAttempts:0},unreadableAttempts:0})
  mocks.getCloudSecretSummary.mockResolvedValue({present:true})
  mocks.getCloudBilling.mockResolvedValue({workspace:'studio',cycle:'2026-09',metered_cost:12.5,billed_cost:0})
  render(CloudUsage)
  await waitFor(() => expect(mocks.getCloudBilling).toHaveBeenCalled())
  await fireEvent.click(screen.getByRole('button', {name:'Cloud cost'}))
  expect(await screen.findByText('$12.50')).toBeTruthy()
  expect(screen.getByText('Modal workspace: studio')).toBeTruthy()
  mocks.deleteCloudSecret.mockResolvedValue({})
  await fireEvent.click(screen.getByRole('button', { name: 'Disconnect billing and forget token' }))
  await waitFor(() => expect(billing.data).toBeNull())
  expect(mocks.deleteCloudSecret).toHaveBeenCalledWith({provider:'modal',profileId:'mc-test',role:'setup'})
})
it('shows corrupt ledger totals as unknown without requiring expansion', async () => {
  mocks.getCloudUsage.mockResolvedValue({month:{attempts:0,reportedUsd:0,unpricedAttempts:0},session:{attempts:0,reportedUsd:0,unpricedAttempts:0},unreadableAttempts:1})
  render(CloudUsage)
  expect(await screen.findByRole('status')).toBeTruthy()
  expect(screen.queryByText('$0.00')).toBeNull()
  expect(screen.getAllByText('Not reported')).toHaveLength(2)
})
it('does not misreport a failed usage read as a zero total', async () => {
  session.cloudAllowed = true
  mocks.getCloudUsage.mockRejectedValue(new Error('unavailable'))
  render(CloudUsage)
  expect(await screen.findByRole('status')).toHaveProperty('textContent', 'Usage is unavailable. Retrying…')
  expect(screen.queryByText('$0.00')).toBeNull()
})
