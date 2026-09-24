/**
 * The cloud setup, driven the way a person drives it: the key, Continue, the
 * review, one approval, Start, the live checklist, and each way it can end.
 *
 * Two stand-ins for the native side. The mock backend (`api/mock.js`, every
 * delay zero) runs the whole pipeline, IC-2 progress included, for the paths
 * that go end to end. A scripted helper stands in where a test has to hold the
 * setup at one step: its answers are promises the test settles, and its
 * progress events are sent by hand.
 */
import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/svelte'
import { tick } from 'svelte'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { createMockBackend, PROVISION_APPLY_STEPS } from '../api/mock.js'
import { t } from '../i18n/index.js'
import { clearNotices } from '../state/app.svelte.js'
import CloudProvisioner, { isValidInstallationId, isValidPlanHash } from './CloudProvisioner.svelte'
import { forgetUnfinished, setup } from './provisioning.svelte.js'

const ZERO = { method: 0, openChapter: 0, export: 0, cloud: 0, provision: 0, region: 0, pageTail: 0, noticeStagger: 0 }
const HASH = 'ab'.repeat(32)
const ID = 'mc-test01'
const MODAL = { token_id: 'ak-test-id', token_secret: 'as-test-secret' }
const MODAL_KEY = 'settings.inference.provider.modal'
const BEAM_KEY = 'settings.inference.provider.beam'

/**
 * Local storage for the test. The node running the suite has none of its own,
 * and the unfinished-setup marker lives there.
 */
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
  setup.run = null
  forgetUnfinished()
  clearNotices()
  vi.unstubAllGlobals()
  vi.restoreAllMocks()
})

/* ---------- stand-ins ---------- */

function deferred() {
  /** @type {(value: any) => void} */
  let resolve = () => {}
  const promise = new Promise((done) => {
    resolve = done
  })
  return { promise, resolve }
}

/** @param {any} data */
const ok = (data) => ({ success: true, data, error: null })
/** @param {string} code */
const refused = (code) => ({ success: false, data: null, error: { code, message: 'refused' } })

/**
 * A plan in the shape the helper answers with.
 *
 * @param {{provider?: 'modal'|'beam', gpu?: string, hash?: string}} [options]
 */
function planData({ provider = 'modal', gpu, hash = HASH } = {}) {
  const gpus = provider === 'modal' ? ['L4', 'A10', 'L40S'] : ['RTX4090', 'A10G', 'RTX5090']
  return {
    plan_hash: hash,
    installation_id: ID,
    provider,
    resource_allocation: {
      gpu: gpu ?? gpus[0],
      gpu_options: gpus,
      idle_seconds: 120,
      model_weights_bytes: 5_475_930_180,
    },
    resources_to_create: [
      { type: 'volume', name: `mc-weights-${ID}` },
      { type: 'app', name: `mc-${ID}` },
      { type: 'proxy_token', name: `mc-token-${ID}` },
    ],
  }
}

const ENDPOINT = `https://ws--mc-${ID}-gateway.modal.run/mc/v1`

/** What the native side answers once it has saved and selected the endpoint (IC-1). */
function installed() {
  return ok({
    profile: { provider: 'modal', profile_id: ID, name: `Modal (${ID})`, endpoint_url: ENDPOINT },
    health: { ok: true, status: 'reachable', latency_ms: 42 },
    selected: true,
  })
}

/**
 * A helper that answers each op from `answers`: a value, or a function of the
 * spec, which may return a promise the test holds.
 *
 * @param {Record<string, any>} answers
 */
function scripted(answers) {
  return vi.fn(async (/** @type {{op: string}} */ spec) => {
    const answer = answers[spec.op]
    if (answer === undefined) throw new Error(`no answer for ${spec.op}`)
    return typeof answer === 'function' ? answer(spec) : answer
  })
}

/** The mock backend with the progress channel and Stop in the test's hands. */
function handBackend() {
  const base = createMockBackend({ timing: ZERO })
  /** @type {((event: unknown) => void)|null} */
  let listener = null
  const backend = Object.assign(Object.create(base), {
    onProvisionProgress: vi.fn((/** @type {(event: unknown) => void} */ handler) => {
      listener = handler
      return () => {
        if (listener === handler) listener = null
      }
    }),
    cancelCloudProvisioner: vi.fn(async () => ({ cancelled: true })),
  })
  return {
    backend,
    /** @param {Record<string, unknown>} event */
    async emit(event) {
      listener?.({ op: 'apply', provider: 'modal', ...event })
      await tick()
    },
  }
}

/* ---------- driving it ---------- */

/** @param {string} name */
const button = (name) => /** @type {HTMLButtonElement} */ (screen.getByRole('button', { name }))

/** @param {string} key */
const heading = (key) => screen.findByRole('heading', { name: t(key) })

/**
 * @param {string} labelKey
 * @param {string} value
 */
async function typeKey(labelKey, value) {
  await fireEvent.input(screen.getByLabelText(t(labelKey)), { target: { value } })
}

async function typeModalKeys() {
  await typeKey('settings.cloud.setup.connect.modalTokenId', MODAL.token_id)
  await typeKey('settings.cloud.setup.connect.modalTokenSecret', MODAL.token_secret)
}

/**
 * What WebKit does when the focused control turns disabled: focus falls to the
 * body. jsdom keeps it on the disabled control, and will not blur one either,
 * so focus is moved away through a stand-in that is then removed.
 */
function dropFocus() {
  const stand = document.createElement('input')
  document.body.append(stand)
  stand.focus()
  stand.remove()
}

async function connectModal() {
  await typeModalKeys()
  await fireEvent.click(button(t('settings.cloud.setup.connect.continue')))
  await heading('settings.cloud.setup.heading.review')
}

/** @param {string} [providerKey] */
function approval(providerKey = MODAL_KEY) {
  return /** @type {HTMLInputElement} */ (
    screen.getByRole('checkbox', { name: t('settings.cloud.setup.review.approve', { providerKey }) })
  )
}

async function approveAndStart() {
  await fireEvent.click(approval())
  await fireEvent.click(button(t('settings.cloud.setup.review.start')))
}

/**
 * One line of the live checklist, by its step. The checklist is named by the
 * heading above it, which says how the run is going.
 *
 * @param {string} step
 * @param {string} [headingKey]
 */
function stepRow(step, headingKey = 'settings.cloud.setup.heading.running') {
  const list = screen.getByRole('list', { name: t(headingKey) })
  return /** @type {HTMLElement} */ (within(list).getByText(t(`settings.cloud.setup.step.${step}`)).closest('li'))
}

/** Every value in local storage, where a secret must never be. */
function stored() {
  const values = []
  for (let index = 0; index < localStorage.length; index += 1) {
    const key = localStorage.key(index)
    if (key !== null) values.push(`${key}=${localStorage.getItem(key)}`)
  }
  return values.join('\n')
}

/* ---------- tests ---------- */

describe('CloudProvisioner helper validations', () => {
  it('validates installation IDs correctly', () => {
    expect(isValidInstallationId('mc-inst-prod-1')).toBe(true)
    expect(isValidInstallationId('worker_01')).toBe(true)
    expect(isValidInstallationId('a')).toBe(true)
    expect(isValidInstallationId('A'.repeat(64))).toBe(true)

    expect(isValidInstallationId('')).toBe(false)
    expect(isValidInstallationId(' mc-inst ')).toBe(false)
    expect(isValidInstallationId('has.dot')).toBe(false)
    expect(isValidInstallationId('has/slash')).toBe(false)
    expect(isValidInstallationId('has space')).toBe(false)
    expect(isValidInstallationId('A'.repeat(65))).toBe(false)
  })

  it('validates 64-character hex plan hashes correctly', () => {
    expect(isValidPlanHash('a'.repeat(64))).toBe(true)
    expect(isValidPlanHash('e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855')).toBe(true)

    expect(isValidPlanHash('')).toBe(false)
    expect(isValidPlanHash('a'.repeat(63))).toBe(false)
    expect(isValidPlanHash('a'.repeat(65))).toBe(false)
    expect(isValidPlanHash('g'.repeat(64))).toBe(false)
    expect(isValidPlanHash('has non-hex!'.padEnd(64, '0'))).toBe(false)
  })
})

describe('setting up end to end on the mock backend', () => {
  it('reviews the plan, runs the checklist, and hands over the saved endpoint', async () => {
    const backend = createMockBackend({ timing: ZERO })
    /** @type {any[]} */
    const progress = []
    backend.onProvisionProgress((/** @type {any} */ event) => progress.push(event))
    const onconfigured = vi.fn()
    const onbusychange = vi.fn()
    render(CloudProvisioner, { props: { inline: true, installationId: ID, backend, onconfigured, onbusychange } })

    expect(screen.getByRole('heading', { name: t('settings.cloud.setup.heading.connect') })).toBeTruthy()
    expect(screen.getByText(t('settings.cloud.setup.connect.keyNoteModal'))).toBeTruthy()
    expect(button(t('settings.cloud.setup.connect.continue')).disabled).toBe(true)
    await connectModal()

    // The GPU and idle choices are the plan's own, not a list of ours.
    const gpu = /** @type {HTMLSelectElement} */ (screen.getByLabelText(t('settings.cloud.setup.review.gpu')))
    expect(Array.from(gpu.options, (option) => option.value)).toEqual(['L4', 'A10', 'L40S'])
    expect(gpu.value).toBe('L4')
    const idle = /** @type {HTMLSelectElement} */ (screen.getByLabelText(t('settings.cloud.setup.review.idle')))
    expect(idle.value).toBe('120')
    for (const name of [`mc-weights-${ID}`, `mc-jobs-${ID}`, `mc-${ID}`, `mc-token-${ID}`]) {
      expect(screen.getByText(name)).toBeTruthy()
    }
    expect(screen.getByText(t('settings.cloud.setup.review.weights', { size: '5.5' }))).toBeTruthy()
    expect(screen.getByText(t('settings.cloud.setup.review.costGpu', { providerKey: MODAL_KEY }))).toBeTruthy()
    expect(screen.getByText(t('settings.cloud.setup.review.tokenModal'))).toBeTruthy()
    expect(button(t('settings.cloud.setup.review.start')).disabled).toBe(true)

    await approveAndStart()
    await heading('settings.cloud.setup.heading.done')

    // Every IC-2 step reached the checklist, in the helper's order.
    expect(progress.filter((event) => event.state === 'done').map((event) => event.step)).toEqual(PROVISION_APPLY_STEPS)
    expect(setup.run?.steps.map((step) => [step.id, step.state])).toEqual(
      PROVISION_APPLY_STEPS.map((step) => [step, 'done']),
    )

    const name = `Modal (${ID})`
    expect(screen.getByText(t('settings.cloud.setup.done.saved', { name }))).toBeTruthy()
    expect(screen.getByText(t('settings.inference.health.reachable'))).toBeTruthy()
    expect(screen.getByText(t('settings.inference.health.latency', { latency: 42 }))).toBeTruthy()
    expect(screen.getByText(t('settings.cloud.setup.done.tryIt'))).toBeTruthy()
    expect(onconfigured).toHaveBeenCalledTimes(1)
    expect(onconfigured).toHaveBeenCalledWith({
      provider: 'modal',
      profileId: ID,
      endpointUrl: `https://mock-workspace--mc-${ID}-gateway.modal.run/mc/v1`,
      name,
      healthy: true,
    })
    await waitFor(() => expect(onbusychange.mock.calls.map(([busy]) => busy)).toEqual([true, false, true, false]))

    // The native side saved and selected it, with its token in the keychain.
    const config = await backend.readInferenceConfig()
    expect(config.selectedTarget).toEqual({ type: 'modal', profile_id: ID })
    const token = await backend.getCloudSecretSummary({ provider: 'modal', profileId: ID, role: 'runtime' })
    expect(token.present).toBe(true)

    // The setup key went in the calls that needed it and nowhere else.
    expect(setup.unfinished).toBeNull()
    for (const where of [JSON.stringify(setup), stored(), document.body.innerHTML]) {
      expect(where).not.toContain(MODAL.token_secret)
      expect(where).not.toContain(MODAL.token_id)
    }
  })

  it('for Beam, asks for the API key, offers Beam’s GPUs and says the endpoint uses that key', async () => {
    const backend = createMockBackend({ timing: ZERO })
    render(CloudProvisioner, { props: { inline: true, installationId: ID, backend } })

    const beam = screen.getAllByRole('radio').find((radio) => /** @type {HTMLInputElement} */ (radio).value === 'beam')
    await fireEvent.click(/** @type {HTMLElement} */ (beam))
    expect(screen.queryByLabelText(t('settings.cloud.setup.connect.modalTokenId'))).toBeNull()
    expect(screen.getByText(t('settings.cloud.setup.connect.keyNoteBeam'))).toBeTruthy()

    await typeKey('settings.cloud.setup.connect.beamToken', 'bk-test-key')
    await fireEvent.click(button(t('settings.cloud.setup.connect.continue')))
    await heading('settings.cloud.setup.heading.review')

    const gpu = /** @type {HTMLSelectElement} */ (screen.getByLabelText(t('settings.cloud.setup.review.gpu')))
    expect(Array.from(gpu.options, (option) => option.value)).toEqual(['RTX4090', 'A10G', 'RTX5090'])
    expect(gpu.value).toBe('RTX4090')
    expect(screen.getByText(t('settings.cloud.setup.review.lead', { providerKey: BEAM_KEY }))).toBeTruthy()
    expect(screen.getByText(t('settings.cloud.setup.review.tokenBeam'))).toBeTruthy()
    expect(screen.queryByText(t('settings.cloud.setup.review.tokenModal'))).toBeNull()
    expect(approval(BEAM_KEY)).toBeTruthy()
  })

  it('offers where to find the key', async () => {
    render(CloudProvisioner, { props: { inline: true, backend: createMockBackend({ timing: ZERO }) } })
    const help = button(t('settings.cloud.setup.connect.help'))
    expect(help.getAttribute('aria-expanded')).toBe('false')
    await fireEvent.click(help)
    expect(help.getAttribute('aria-expanded')).toBe('true')
    expect(screen.getByText(t('settings.cloud.setup.connect.helpModal'))).toBeTruthy()
    expect(screen.getByText('https://modal.com/settings/tokens')).toBeTruthy()
  })
})

describe('the live checklist', () => {
  it('shows each step with its time, and Stop asks the helper to stop', async () => {
    const applied = deferred()
    const runner = scripted({ inspect: ok({ eligible: true }), plan: ok(planData()), apply: () => applied.promise })
    const { backend, emit } = handBackend()
    const onbusychange = vi.fn()
    render(CloudProvisioner, {
      props: { inline: true, installationId: ID, runCloudProvisioner: runner, backend, onbusychange },
    })
    await connectModal()
    await approveAndStart()

    const running = await heading('settings.cloud.setup.heading.running')
    expect(document.activeElement).toBe(running)
    expect(runner).toHaveBeenLastCalledWith({
      op: 'apply',
      provider: 'modal',
      params: {
        credentials: MODAL,
        installation_id: ID,
        approved_plan_hash: HASH,
        options: {},
        forget_setup_credential: true,
      },
    })

    await emit({ step: 'validate', state: 'start' })
    await emit({ step: 'validate', state: 'done' })
    await emit({ step: 'volume', state: 'skip' })
    await emit({ step: 'weights', state: 'start', pct: 0 })
    await emit({ step: 'weights', state: 'start', pct: 50 })
    // Another operation's event is not this checklist's.
    await emit({ op: 'cleanup_apply', step: 'image', state: 'start' })

    const validate = stepRow('validate')
    expect(validate.classList.contains('done')).toBe(true)
    expect(within(validate).getByText(t('settings.cloud.setup.state.done'))).toBeTruthy()
    expect(validate.querySelector('.step-time')?.textContent?.trim()).toMatch(/^\d+:\d\d$/)
    expect(stepRow('volume').classList.contains('skip')).toBe(true)
    const weights = stepRow('weights')
    expect(weights.classList.contains('running')).toBe(true)
    expect(weights.querySelector('.step-time')?.textContent?.trim()).toBe('50%')
    expect(screen.queryByText(t('settings.cloud.setup.step.image'))).toBeNull()

    await fireEvent.click(button(t('settings.cloud.setup.running.stop')))
    expect(backend.cancelCloudProvisioner).toHaveBeenCalledTimes(1)
    await waitFor(() => expect(button(t('settings.cloud.setup.running.stopping')).disabled).toBe(true))

    applied.resolve(refused('ERR_CANCELLED'))
    await heading('settings.cloud.setup.heading.stopped')
    const alert = screen.getByRole('alert')
    expect(alert.textContent).toContain(t('settings.cloud.setup.error.cancelled'))
    expect(alert.textContent).toContain(t('settings.cloud.setup.failed.code', { code: 'ERR_CANCELLED' }))
    // The step it stopped in reads as stopped, not as still running.
    expect(stepRow('weights', 'settings.cloud.setup.heading.stopped').classList.contains('fail')).toBe(true)
    expect(button(t('settings.cloud.setup.failed.resume')).disabled).toBe(false)
    expect(button(t('settings.cloud.setup.failed.cleanup'))).toBeTruthy()
    expect(onbusychange.mock.calls.map(([busy]) => busy)).toEqual([true, false, true, false])
  })

  it('cannot be dismissed while it runs, and Escape closes it once it has ended', async () => {
    const applied = deferred()
    const runner = scripted({ inspect: ok({ eligible: true }), plan: ok(planData()), apply: () => applied.promise })
    const { backend } = handBackend()
    const onclose = vi.fn()
    render(CloudProvisioner, { props: { installationId: ID, runCloudProvisioner: runner, backend, onclose } })
    expect(screen.getByRole('dialog')).toBeTruthy()

    await connectModal()
    await approveAndStart()
    await heading('settings.cloud.setup.heading.running')
    await fireEvent.keyDown(window, { key: 'Escape' })
    expect(onclose).not.toHaveBeenCalled()
    expect(screen.queryByRole('button', { name: t('shell.action.close') })).toBeNull()

    applied.resolve(refused('ERR_EXECUTION_FAILED'))
    await heading('settings.cloud.setup.heading.failed')
    await fireEvent.keyDown(window, { key: 'Escape' })
    expect(onclose).toHaveBeenCalledTimes(1)
  })
})

describe('when setup does not finish', () => {
  it('says why, keeps what finished, and Resume finishes it with the same key', async () => {
    const runner = scripted({
      inspect: ok({ eligible: true }),
      plan: ok(planData()),
      apply: refused('ERR_SECRET_STORE'),
      resume: installed(),
    })
    const { backend } = handBackend()
    const onconfigured = vi.fn()
    render(CloudProvisioner, { props: { inline: true, installationId: ID, runCloudProvisioner: runner, backend, onconfigured } })
    await connectModal()
    await approveAndStart()

    await heading('settings.cloud.setup.heading.failed')
    const alert = screen.getByRole('alert')
    expect(alert.textContent).toContain(t('settings.cloud.setup.error.secretStore'))
    expect(alert.textContent).toContain(t('settings.cloud.setup.failed.code', { code: 'ERR_SECRET_STORE' }))
    expect(screen.getByText(t('settings.cloud.setup.failed.kept'))).toBeTruthy()
    expect(setup.unfinished).toEqual({ provider: 'modal', installationId: ID, options: {} })
    expect(onconfigured).not.toHaveBeenCalled()

    await fireEvent.click(button(t('settings.cloud.setup.failed.resume')))
    await heading('settings.cloud.setup.heading.done')
    expect(runner).toHaveBeenLastCalledWith({
      op: 'resume',
      provider: 'modal',
      params: { credentials: MODAL, installation_id: ID, options: {}, forget_setup_credential: true },
    })
    expect(screen.getByText(t('settings.cloud.setup.done.saved', { name: `Modal (${ID})` }))).toBeTruthy()
    expect(onconfigured).toHaveBeenCalledTimes(1)
    expect(onconfigured).toHaveBeenCalledWith({
      provider: 'modal',
      profileId: ID,
      endpointUrl: ENDPOINT,
      name: `Modal (${ID})`,
      healthy: true,
    })
    expect(setup.unfinished).toBeNull()
  })

  it('offers a setup the last run left unfinished, with the choices it was planned with', async () => {
    setup.unfinished = { provider: 'modal', installationId: 'mc-half01', options: { gpu: 'A10' } }
    const runner = scripted({ resume: ok({ resumed: false }) })
    const onconfigured = vi.fn()
    render(CloudProvisioner, {
      props: { inline: true, runCloudProvisioner: runner, backend: handBackend().backend, onconfigured },
    })

    await heading('settings.cloud.setup.heading.resume')
    expect(
      screen.getByText(t('settings.cloud.setup.resume.lead', { providerKey: MODAL_KEY, id: 'mc-half01' })),
    ).toBeTruthy()
    const resume = button(t('settings.cloud.setup.failed.resume'))
    expect(resume.disabled).toBe(true)
    const secret = screen.getByLabelText(t('settings.cloud.setup.connect.modalTokenSecret'))
    await typeModalKeys()
    // The fields stay while they are typed in, so a typo can still be fixed.
    expect(screen.getByLabelText(t('settings.cloud.setup.connect.modalTokenSecret'))).toBe(secret)
    expect(resume.disabled).toBe(false)
    await fireEvent.click(resume)

    // A resume of a setup that had already finished changes nothing.
    await heading('settings.cloud.setup.heading.finished')
    expect(runner).toHaveBeenLastCalledWith({
      op: 'resume',
      provider: 'modal',
      params: { credentials: MODAL, installation_id: 'mc-half01', options: { gpu: 'A10' }, forget_setup_credential: true },
    })
    expect(screen.getByText(t('settings.cloud.setup.done.nothing'))).toBeTruthy()
    expect(onconfigured).not.toHaveBeenCalled()
  })

  it('never calls an endpoint ready when its first check failed, and still hands it over as saved', async () => {
    const failing = installed()
    failing.data.health = { ok: false, status: 'unreachable', latency_ms: null }
    const runner = scripted({ inspect: ok({ eligible: true }), plan: ok(planData()), apply: failing })
    const onconfigured = vi.fn()
    const onbusychange = vi.fn()
    render(CloudProvisioner, {
      props: { inline: true, installationId: ID, runCloudProvisioner: runner, backend: handBackend().backend, onconfigured, onbusychange },
    })
    await connectModal()
    await approveAndStart()

    await heading('settings.cloud.setup.heading.finished')
    const name = `Modal (${ID})`
    expect(screen.getByText(t('settings.inference.health.unreachable'))).toBeTruthy()
    expect(screen.getByText(t('settings.cloud.setup.done.unchecked', { name }))).toBeTruthy()
    expect(screen.queryByRole('heading', { name: t('settings.cloud.setup.heading.done') })).toBeNull()
    expect(screen.queryByText(t('settings.cloud.setup.done.saved', { name }))).toBeNull()
    expect(screen.queryByText(t('settings.cloud.setup.done.tryIt'))).toBeNull()
    // The host hears of it, so it does not offer a second setup, and is told
    // it is not ready, so it does not turn the cloud on.
    expect(onconfigured).toHaveBeenCalledTimes(1)
    expect(onconfigured).toHaveBeenCalledWith({ provider: 'modal', profileId: ID, endpointUrl: ENDPOINT, name, healthy: false })
    expect(onbusychange).toHaveBeenLastCalledWith(false)
  })

  it('reads an answer that carries no check as a check that could not be made', async () => {
    const unchecked = installed()
    delete unchecked.data.health
    const runner = scripted({ inspect: ok({ eligible: true }), plan: ok(planData()), apply: unchecked })
    const onconfigured = vi.fn()
    render(CloudProvisioner, {
      props: { inline: true, installationId: ID, runCloudProvisioner: runner, backend: handBackend().backend, onconfigured },
    })
    await connectModal()
    await approveAndStart()

    await heading('settings.cloud.setup.heading.finished')
    expect(screen.getByText(t('settings.inference.health.unknown'))).toBeTruthy()
    expect(screen.getByText(t('settings.cloud.setup.done.unchecked', { name: `Modal (${ID})` }))).toBeTruthy()
    expect(onconfigured).toHaveBeenCalledTimes(1)
    expect(onconfigured.mock.calls[0][0]).toMatchObject({ profileId: ID, healthy: false })
  })
})

describe('the Review step', () => {
  it('a different GPU is a new plan: approval is asked again and focus stays on the select', async () => {
    const second = deferred()
    let plans = 0
    const runner = scripted({
      inspect: ok({ eligible: true }),
      plan: () => {
        plans += 1
        return plans === 1 ? ok(planData()) : second.promise
      },
    })
    render(CloudProvisioner, {
      props: { inline: true, installationId: ID, runCloudProvisioner: runner, backend: handBackend().backend },
    })
    await connectModal()
    await fireEvent.click(approval())
    expect(approval().checked).toBe(true)

    const gpu = /** @type {HTMLSelectElement} */ (screen.getByLabelText(t('settings.cloud.setup.review.gpu')))
    gpu.focus()
    await fireEvent.change(gpu, { target: { value: 'A10' } })
    expect(runner).toHaveBeenLastCalledWith({
      op: 'plan',
      provider: 'modal',
      params: { credentials: MODAL, installation_id: ID, options: { gpu: 'A10' } },
    })
    expect(gpu.disabled).toBe(true)
    expect(approval().disabled).toBe(true)
    dropFocus()
    expect(document.activeElement).toBe(document.body)

    const replanned = 'cd'.repeat(32)
    second.resolve(ok(planData({ gpu: 'A10', hash: replanned })))
    await waitFor(() => expect(gpu.disabled).toBe(false))
    await waitFor(() => expect(document.activeElement).toBe(gpu))
    expect(gpu.value).toBe('A10')
    expect(approval().checked).toBe(false)
    expect(button(t('settings.cloud.setup.review.start')).disabled).toBe(true)

    // The plan's hash, which apply repeats back, is behind a disclosure.
    expect(screen.queryByText(replanned)).toBeNull()
    await fireEvent.click(button(t('settings.cloud.setup.review.details')))
    expect(screen.getByText(replanned)).toBeTruthy()
  })

  it('a refused key is said on the Connect step, with focus back on Continue', async () => {
    const inspected = deferred()
    const runner = scripted({ inspect: () => inspected.promise })
    render(CloudProvisioner, { props: { inline: true, runCloudProvisioner: runner, backend: handBackend().backend } })
    await typeModalKeys()

    const go = button(t('settings.cloud.setup.connect.continue'))
    go.focus()
    expect(document.activeElement).toBe(go)
    await fireEvent.click(go)
    expect(go.textContent?.trim()).toBe(t('settings.cloud.setup.connect.checking'))
    expect(go.disabled).toBe(true)
    dropFocus()

    inspected.resolve(refused('ERR_ACTIONABLE_MISSING_PERMISSION'))
    const alert = await screen.findByRole('alert')
    expect(alert.textContent).toContain(t('settings.cloud.setup.error.permission'))
    expect(alert.textContent).toContain(t('settings.cloud.setup.failed.code', { code: 'ERR_ACTIONABLE_MISSING_PERMISSION' }))
    await waitFor(() => expect(document.activeElement).toBe(go))
    expect(screen.getByRole('heading', { name: t('settings.cloud.setup.heading.connect') })).toBeTruthy()
  })

  it('Cancel wipes the keys before it hands back', async () => {
    const onclose = vi.fn()
    render(CloudProvisioner, { props: { inline: true, backend: handBackend().backend, onclose } })
    await typeModalKeys()
    await fireEvent.click(button(t('shell.action.cancel')))
    expect(onclose).toHaveBeenCalledTimes(1)
    for (const key of ['settings.cloud.setup.connect.modalTokenId', 'settings.cloud.setup.connect.modalTokenSecret']) {
      expect(/** @type {HTMLInputElement} */ (screen.getByLabelText(t(key))).value).toBe('')
    }
  })
})

describe('deleting what a setup created', () => {
  it('lists it, asks for the key again, and removes the endpoint once it is gone', async () => {
    const backend = createMockBackend({ timing: ZERO })
    const credentials = { token_id: 'ak-old', token_secret: 'as-old' }
    const planned = await backend.runCloudProvisioner({
      op: 'plan',
      provider: 'modal',
      params: { credentials, installation_id: 'mc-old001' },
    })
    await backend.runCloudProvisioner({
      op: 'apply',
      provider: 'modal',
      params: { credentials, installation_id: 'mc-old001', approved_plan_hash: planned.data.plan_hash },
    })
    const oncleaned = vi.fn()
    const onbusychange = vi.fn()
    render(CloudProvisioner, {
      props: {
        inline: true,
        existing: { provider: 'modal', installationId: 'mc-old001', action: 'cleanup' },
        backend,
        oncleaned,
        onbusychange,
      },
    })

    await heading('settings.cloud.setup.heading.cleanup')
    await screen.findByText(t('settings.cloud.setup.cleanup.lead', { providerKey: MODAL_KEY }))
    for (const name of ['mc-weights-mc-old001', 'mc-jobs-mc-old001', 'mc-mc-old001', 'mc-token-mc-old001']) {
      expect(screen.getByText(name)).toBeTruthy()
    }
    const remove = button(t('settings.cloud.setup.cleanup.delete'))
    expect(remove.disabled).toBe(true)
    await typeKey('settings.cloud.setup.connect.modalTokenId', 'ak-old')
    await typeKey('settings.cloud.setup.connect.modalTokenSecret', 'as-old')
    expect(remove.disabled).toBe(true)
    await fireEvent.click(screen.getByRole('checkbox', { name: t('settings.cloud.setup.cleanup.approve') }))
    await fireEvent.click(remove)

    await heading('settings.cloud.setup.heading.cleaned')
    expect(screen.getByText(t('settings.cloud.setup.cleanup.done', { providerKey: MODAL_KEY }))).toBeTruthy()
    await waitFor(() => expect(oncleaned).toHaveBeenCalledTimes(1))
    const config = await backend.readInferenceConfig()
    expect(config.modalProfiles['mc-old001']).toBeUndefined()
    expect(config.selectedTarget).toEqual({ type: 'local' })
    await waitFor(() => expect(onbusychange.mock.calls.at(-1)?.[0]).toBe(false))
  })

  it('forgets a setup that left nothing behind, without asking for a key', async () => {
    const oncleaned = vi.fn()
    render(CloudProvisioner, {
      props: {
        inline: true,
        existing: { provider: 'beam', installationId: 'mc-gone01', action: 'cleanup' },
        backend: createMockBackend({ timing: ZERO }),
        oncleaned,
      },
    })
    await screen.findByText(t('settings.cloud.setup.cleanup.none', { providerKey: BEAM_KEY }))
    expect(screen.queryByLabelText(t('settings.cloud.setup.connect.beamToken'))).toBeNull()
    await fireEvent.click(button(t('settings.cloud.setup.cleanup.forget')))
    await heading('settings.cloud.setup.heading.cleaned')
    expect(screen.getByText(t('settings.cloud.setup.cleanup.doneEmpty', { providerKey: BEAM_KEY }))).toBeTruthy()
    expect(oncleaned).toHaveBeenCalledTimes(1)
  })
})
