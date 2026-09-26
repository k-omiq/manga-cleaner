<script module>
  /**
   * Whether an installation id is one the helper accepts as a name part: 1 to
   * 64 characters of ASCII letters, digits, `-` and `_`, with nothing around
   * it. The ids this component makes itself are narrower still
   * (`provisioning.svelte.js#newInstallationId`).
   *
   * @param {string} id
   * @returns {boolean}
   */
  export function isValidInstallationId(id) {
    if (!id || typeof id !== 'string') return false
    if (id !== id.trim()) return false
    return /^[a-zA-Z0-9_-]{1,64}$/.test(id)
  }

  /**
   * Whether a plan hash is a SHA-256 in hex, which is what the helper signs a
   * plan with and what `apply` must repeat back.
   *
   * @param {string} hash
   * @returns {boolean}
   */
  export function isValidPlanHash(hash) {
    if (!hash || typeof hash !== 'string') return false
    return /^[a-fA-F0-9]{64}$/.test(hash)
  }
</script>

<script>
  import { onDestroy, onMount, tick, untrack } from 'svelte'
  import Icon from '../icons/Icon.svelte'
  import { Button, Disclosure, Field, Modal, Select, TextInput } from '../ui/index.js'
  import { getBackend } from '../api/backend.js'
  import { t } from '../i18n/index.js'
  import { parseModalTokenCommand } from './modal-token-command.js'
  import {
    applyCleanup,
    backendRunner,
    cleanOptions,
    clearSetupRun,
    errorCodeOf,
    forgetUnfinished,
    healthKey,
    newInstallationId,
    planCleanup,
    removeEndpoint,
    resourceKey,
    runSetup,
    setup,
    setupErrorKey,
    setupStepKey,
    stopSetup,
    watchSetup,
  } from './provisioning.svelte.js'

  /**
   * Set up a private cloud GPU in the user's own Modal or Beam account.
   *
   * Four steps, each one screen:
   *
   * 1. **Connect.** Pick the provider and paste its key. "Where do I find
   *    this?" says where the provider shows it. Continue runs the helper's
   *    read-only `inspect` and `plan`; nothing is created.
   * 2. **Review.** What will be created, by name; the GPU and the idle window
   *    the plan offers (never a list of our own); the weights download; what
   *    it costs and who bills it; one approval. The plan's hash, which
   *    `apply` repeats back so the helper runs exactly what was shown, is in a
   *    disclosure.
   * 3. **Setting up.** The helper's own steps as a live checklist, with the
   *    time each one took. It cannot be closed by accident: there is no close
   *    control while it runs, the dialog form ignores Escape, and a host that
   *    embeds it hears `onbusychange(true)` so it can hold its own. Stop asks
   *    the helper to stop; its journal keeps what finished, so a failure or a
   *    stop offers Resume, and Clean up.
   * 4. **Done.** The endpoint is saved and selected by the native side (IC-1);
   *    this says so, shows the health check it ran, and hands the profile to
   *    `onconfigured` with whether that check passed. Only a passed check is
   *    "ready"; an endpoint that did not answer is still saved and selected,
   *    and the host still hears of it, since a host that never did would
   *    offer a second setup, which is a second installation in the account.
   *
   * The run itself lives in `provisioning.svelte.js`, not here, so closing
   * Settings does not orphan a setup that is minutes long: mounting this again
   * picks the checklist back up, and a setup that never finished is offered
   * for Resume after a restart.
   *
   * **Secrets.** The keys live in this component's state and nowhere else:
   * not in the store, not in storage, not in the DOM beyond the password
   * fields. They are sent in the one helper call that needs them and wiped on
   * close, on success and on destroy. The native side keeps the runtime
   * credential in the system keychain (for Modal the access token setup
   * makes, for Beam the person's own key, since Beam has no narrower one) and
   * never hands it back here.
   *
   * @type {{
   *   initialProvider?: 'modal'|'beam',
   *   installationId?: string,
   *   inline?: boolean,
   *   existing?: {provider: 'modal'|'beam', installationId: string, action: 'resume'|'cleanup'}|null,
   *   onclose?: () => unknown,
   *   onconfigured?: (info: {provider: 'modal'|'beam', profileId: string, endpointUrl: string, name: string, healthy: boolean}) => unknown,
   *   oncleaned?: () => unknown,
   *   onbusychange?: (busy: boolean) => unknown,
   *   runCloudProvisioner?: (spec: {op: string, provider: string, params: Record<string, unknown>}) => Promise<any>,
   *   backend?: import('../api/backend.js').Backend,
   * }}
   */
  let {
    initialProvider = 'modal',
    installationId: givenInstallationId = '',
    inline = false,
    existing = null,
    onclose,
    onconfigured,
    oncleaned,
    onbusychange,
    runCloudProvisioner,
    backend: givenBackend,
  } = $props()

  /** Where each provider shows the key, as its own documentation links it. */
  const HELP_URLS = Object.freeze({
    modal: 'https://modal.com/settings/tokens',
    beam: 'https://platform.beam.cloud/settings/api-keys',
  })
  const PROVIDERS = /** @type {const} */ (['modal', 'beam'])
  /** The idle windows offered when the plan has one; the plan's own value is always in the list. */
  const IDLE_CHOICES = [60, 120, 300, 600]
  const GPU_NAME = /^[A-Za-z0-9_-]{1,32}$/
  /** What the model weights weigh when the plan does not say. */
  const DEFAULT_WEIGHTS_GB = 5.5

  /* ---------- fixed at mount: one provisioner is one setup ---------- */

  const backend = untrack(() => givenBackend ?? getBackend())
  const runner = untrack(() => runCloudProvisioner ?? backendRunner(backend))
  const task = untrack(() => taskOf(existing))
  const uid = $props.id()
  const ids = {
    heading: `${uid}-heading`,
    tokenId: `${uid}-token-id`,
    tokenSecret: `${uid}-token-secret`,
    tokenCommand: `${uid}-token-command`,
    beamToken: `${uid}-beam-token`,
    help: `${uid}-help`,
    gpu: `${uid}-gpu`,
    model: `${uid}-model`,
    analysis: `${uid}-analysis`,
    idle: `${uid}-idle`,
  }

  /**
   * @param {unknown} value
   * @returns {{provider: 'modal'|'beam', installationId: string, action: 'resume'|'cleanup'}|null}
   */
  function taskOf(value) {
    if (!value || typeof value !== 'object') return null
    const { provider, installationId, action } = /** @type {Record<string, unknown>} */ (value)
    if (provider !== 'modal' && provider !== 'beam') return null
    if (typeof installationId !== 'string' || !isValidInstallationId(installationId)) return null
    if (action !== 'resume' && action !== 'cleanup') return null
    return { provider, installationId, action }
  }

  /* ---------- the keys: component state only ---------- */

  let modalTokenId = $state('')
  let modalTokenSecret = $state('')
  let modalCommand = $state('')
  let modalCommandError = $state(false)
  let modalProfile = $state('')
  let beamToken = $state('')

  /** Wipe the keys. Called on close, on success and on destroy. */
  export function clearSecrets() {
    modalTokenId = ''
    modalTokenSecret = ''
    modalCommand = ''
    modalCommandError = false
    modalProfile = ''
    beamToken = ''
  }

  /** Modal's copy button supplies a CLI command. Parse it; never execute it. */
  function importModalCommand(value) {
    modalCommand = value
    const parsed = parseModalTokenCommand(value)
    if (!parsed) {
      modalCommandError = value.trim().length > 0
      modalTokenId = ''
      modalTokenSecret = ''
      modalProfile = ''
      return
    }
    modalTokenId = parsed.tokenId
    modalTokenSecret = parsed.tokenSecret
    modalProfile = parsed.profile
    modalCommandError = false
    // TextInput is controlled. Give Svelte one render of the pasted value so
    // clearing the field also clears the DOM input, not only component state.
    void tick().then(() => {
      if (alive && modalCommand === value) modalCommand = ''
    })
  }

  function setModalTokenId(value) {
    modalTokenId = value
    modalCommand = ''
    modalCommandError = false
    modalProfile = ''
  }

  function setModalTokenSecret(value) {
    modalTokenSecret = value
    modalCommand = ''
    modalCommandError = false
    modalProfile = ''
  }

  /** @param {'modal'|'beam'} which */
  function hasKeys(which) {
    return which === 'modal'
      ? modalTokenId.trim() !== '' && modalTokenSecret.trim() !== ''
      : beamToken.trim() !== ''
  }

  /**
   * The helper's `credentials` for one provider, built at the moment of the
   * call and never kept.
   *
   * @param {'modal'|'beam'} which
   */
  function credentials(which) {
    return which === 'modal'
      ? { token_id: modalTokenId.trim(), token_secret: modalTokenSecret.trim() }
      : { token: beamToken.trim() }
  }

  /* ---------- where the flow is ---------- */

  /** @type {'modal'|'beam'} */
  let provider = $state(untrack(() => task?.provider ?? (initialProvider === 'beam' ? 'beam' : 'modal')))
  let installationId = $state(
    untrack(() => (isValidInstallationId(givenInstallationId) ? givenInstallationId : newInstallationId())),
  )

  /** @type {'connect'|'review'|'resume'|'cleanup'} */
  let page = $state('connect')
  /**
   * The installation Resume and Clean up act on, and the plan choices it was
   * made with.
   *
   * @type {{provider: 'modal'|'beam', installationId: string, options: import('./provisioning.svelte.js').SetupOptions}|null}
   */
  let target = $state(null)

  let planning = $state(false)
  /** @type {string|null} an i18n key */
  let planError = $state(null)
  /** @type {string|null} */
  let planCode = $state(null)
  let accountName = $state('')
  /** @type {any} */
  let plan = $state(null)
  /** @type {import('./provisioning.svelte.js').SetupOptions} */
  let planOptions = $state({})
  let approved = $state(false)
  let detailsOpen = $state(false)
  let helpOpen = $state(false)
  let copied = $state(false)

  let cleanupPlanning = $state(false)
  /** @type {any} */
  let cleanupPlan = $state(null)
  /** @type {string|null} */
  let cleanupCode = $state(null)
  let cleanupApproved = $state(false)
  /** A cleanup that found nothing to delete ends here, without a helper run. */
  let cleanedEmpty = $state(false)

  /** Which store run this provisioner shows: one it started, or one running when it mounted. */
  let adoptedId = $state(0)
  let stopping = $state(false)
  let now = $state(Date.now())
  let alive = true

  // A run that ended before this mount was already said by a notice; a new
  // provisioner starts clean rather than on someone else's result.
  untrack(() => {
    const current = setup.run
    if (current?.status === 'running') {
      adoptedId = current.id
      target = {
        provider: current.provider,
        installationId: current.installationId,
        options: setup.unfinished?.installationId === current.installationId ? setup.unfinished.options : {},
      }
      provider = current.provider
      return
    }
    if (current) clearSetupRun()
    if (task) {
      target = {
        provider: task.provider,
        installationId: task.installationId,
        options: setup.unfinished?.installationId === task.installationId ? setup.unfinished.options : {},
      }
      page = task.action === 'resume' ? 'resume' : 'cleanup'
    } else if (setup.unfinished) {
      target = { ...setup.unfinished, options: { ...setup.unfinished.options } }
      provider = setup.unfinished.provider
      page = 'resume'
    }
  })

  const run = $derived(setup.run && setup.run.id === adoptedId ? setup.run : null)

  const view = $derived.by(() => {
    if (run) {
      const cleanup = run.op === 'cleanup_apply'
      if (run.status === 'running') return cleanup ? 'cleaning' : 'running'
      if (run.status === 'failed') return cleanup ? 'cleanupFailed' : 'failed'
      return cleanup ? 'cleaned' : 'done'
    }
    if (page === 'cleanup' && cleanedEmpty) return 'cleaned'
    return page
  })

  const busy = $derived(planning || cleanupPlanning || run?.status === 'running')

  /**
   * Whether this screen asks for the key, decided as the screen opens: keys
   * still held from an earlier screen (a failed run's, say) are not asked for
   * again, and fields that are shown stay while they are typed in.
   */
  let askKeys = $state(false)
  let askedOn = ''
  $effect.pre(() => {
    const current = view
    if (current === askedOn) return
    askedOn = current
    untrack(() => {
      askKeys = target !== null && !hasKeys(target.provider)
    })
  })

  /* ---------- callbacks ---------- */

  /**
   * A host callback, which must not be able to break the flow: a throw is
   * swallowed, and so is a rejected promise.
   *
   * @param {((...args: any[]) => unknown)|undefined} callback
   * @param {...unknown} args
   */
  function call(callback, ...args) {
    try {
      const result = callback?.(...args)
      if (result && typeof (/** @type {any} */ (result).then) === 'function') {
        /** @type {Promise<unknown>} */ (result).then(undefined, () => {})
      }
    } catch {
      // The setup did what it did whatever the host made of it.
    }
  }

  let reportedBusy = false
  $effect(() => {
    const next = busy
    if (next === reportedBusy) return
    reportedBusy = next
    untrack(() => call(onbusychange, next))
  })

  // The clock the checklist reads, ticking only while something runs.
  $effect(() => {
    if (run?.status !== 'running') return
    now = Date.now()
    const timer = setInterval(() => {
      now = Date.now()
    }, 1000)
    return () => clearInterval(timer)
  })

  // A run this provisioner shows has ended: once per run.
  let handledId = 0
  $effect(() => {
    const current = run
    if (!current || current.status === 'running' || handledId === current.id) return
    handledId = current.id
    untrack(() => ended(current))
  })

  /** @param {import('./provisioning.svelte.js').SetupRun} current */
  function ended(current) {
    stopping = false
    if (current.status !== 'done') return
    clearSecrets()
    if (current.op === 'cleanup_apply') {
      call(oncleaned)
      return
    }
    // IC-1: the native side saved and selected the endpoint, then checked it.
    // The host hears of every endpoint saved, checked or not, and is told
    // which: only one whose check passed is ready to turn the cloud on for.
    const saved = profileOf(current)
    if (saved) call(onconfigured, { ...saved, healthy: healthOf(current)?.ok === true })
  }

  /**
   * The health check the native side ran on the endpoint it saved (IC-1
   * `data.health`), or null when the answer carries none.
   *
   * @param {import('./provisioning.svelte.js').SetupRun|null|undefined} current
   * @returns {{ok: boolean, status: unknown, latency: number|null}|null}
   */
  function healthOf(current) {
    const value = current?.data?.health
    if (!value || typeof value !== 'object') return null
    return {
      ok: value.ok === true,
      status: value.status,
      latency: typeof value.latency_ms === 'number' && Number.isFinite(value.latency_ms) ? Math.round(value.latency_ms) : null,
    }
  }

  /**
   * The endpoint the native side saved and selected (IC-1 `data.profile`), or
   * null when the answer names none: a `resume` of a setup that had already
   * finished answers without one, and nothing was saved then.
   *
   * @param {import('./provisioning.svelte.js').SetupRun|null} current
   * @returns {{provider: 'modal'|'beam', profileId: string, endpointUrl: string, name: string}|null}
   */
  function profileOf(current) {
    const profile = current?.data?.profile
    if (!current || !profile || typeof profile !== 'object') return null
    const which = profile.provider === 'beam' || profile.provider === 'modal' ? profile.provider : current.provider
    const profileId =
      typeof profile.profile_id === 'string' && profile.profile_id ? profile.profile_id : current.installationId
    const endpointUrl = typeof profile.endpoint_url === 'string' ? profile.endpoint_url : ''
    const name =
      typeof profile.name === 'string' && profile.name.trim()
        ? profile.name.trim()
        : t('settings.cloud.setup.done.fallbackName', { providerKey: providerKey(which), id: profileId })
    return { provider: which, profileId, endpointUrl, name }
  }

  /* ---------- focus ---------- */

  /** @type {HTMLElement|undefined} */
  let root = $state()
  /** @type {HTMLElement|undefined} */
  let heading = $state()

  // Each new screen is read from its heading. Only when focus was in here or
  // was lost with the control that vanished: never pulled from elsewhere.
  let shownView = ''
  $effect(() => {
    const current = view
    if (!shownView) {
      shownView = current
      return
    }
    if (current === shownView) return
    shownView = current
    tick().then(() => {
      if (!alive || !root || !heading) return
      const active = document.activeElement
      if (active && active !== document.body && !root.contains(active)) return
      heading.focus()
    })
  })

  onMount(() => {
    // Opened to delete what an installation created: say what that is at once.
    // The person already asked for it, so this is not a speculative plan.
    if (task?.action === 'cleanup' && !adoptedId) void startCleanup()
    return watchSetup()
  })

  onDestroy(() => {
    alive = false
    clearSecrets()
    if (reportedBusy) {
      reportedBusy = false
      call(onbusychange, false)
    }
  })

  /* ---------- labels ---------- */

  /** @param {'modal'|'beam'} which */
  function providerKey(which) {
    return which === 'beam' ? 'settings.inference.provider.beam' : 'settings.inference.provider.modal'
  }

  /** @param {number} ms */
  function clock(ms) {
    const seconds = Math.max(0, Math.floor(ms / 1000))
    return `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, '0')}`
  }

  /** @param {import('./provisioning.svelte.js').SetupStep} step */
  function stepTime(step) {
    if (step.state === 'skip' || step.startedAt === null) return ''
    return clock((step.endedAt ?? now) - step.startedAt)
  }

  /**
   * A step still marked running on a run that has ended stopped there.
   *
   * @param {import('./provisioning.svelte.js').SetupStep} step
   */
  function shownState(step) {
    return step.state === 'running' && run?.status !== 'running' ? 'fail' : step.state
  }

  const STATE_KEYS = {
    running: 'settings.cloud.setup.state.running',
    done: 'settings.cloud.setup.state.done',
    fail: 'settings.cloud.setup.state.fail',
    skip: 'settings.cloud.setup.state.skip',
  }

  const headingText = $derived.by(() => {
    switch (view) {
      case 'connect':
        return t('settings.cloud.setup.heading.connect')
      case 'review':
        return t('settings.cloud.setup.heading.review')
      case 'running':
        return t('settings.cloud.setup.heading.running')
      case 'cleaning':
        return t('settings.cloud.setup.heading.cleaning')
      case 'failed':
        return run?.errorCode === 'ERR_CANCELLED'
          ? t('settings.cloud.setup.heading.stopped')
          : t('settings.cloud.setup.heading.failed')
      case 'cleanupFailed':
        return t('settings.cloud.setup.heading.cleanupFailed')
      case 'done':
        return profileOf(run) && health?.ok
          ? t('settings.cloud.setup.heading.done')
          : t('settings.cloud.setup.heading.finished')
      case 'resume':
        return t('settings.cloud.setup.heading.resume')
      case 'cleanup':
        return t('settings.cloud.setup.heading.cleanup')
      default:
        return t('settings.cloud.setup.heading.cleaned')
    }
  })

  /* ---------- plan data ---------- */

  const allocation = $derived(plan?.resource_allocation ?? {})

  /** What the plan creates, as it names it. */
  const planResources = $derived(
    (Array.isArray(plan?.resources_to_create) ? plan.resources_to_create : [])
      .filter((/** @type {any} */ item) => item && typeof item.name === 'string')
      .map((/** @type {any} */ item) => ({ type: item.type ?? item.resource_type, name: item.name.slice(0, 160) })),
  )

  /** The GPUs this plan accepts, from the plan. Empty when it offers no choice. */
  const gpuOptions = $derived(
    (Array.isArray(allocation.gpu_options) ? allocation.gpu_options : [])
      .filter((/** @type {unknown} */ name) => typeof name === 'string' && GPU_NAME.test(name))
      .map((/** @type {string} */ name) => ({ value: name, label: name })),
  )
  const gpu = $derived(typeof allocation.gpu === 'string' && GPU_NAME.test(allocation.gpu) ? allocation.gpu : null)
  const model = $derived(typeof allocation.model_id === 'string' ? allocation.model_id.slice(0, 120) : '')
  const modelOptions = $derived(
    (Array.isArray(allocation.model_options) ? allocation.model_options : [])
      .filter((/** @type {any} */ item) => item && typeof item.model_id === 'string' &&
        typeof item.label === 'string' && item.model_id.length <= 120 && item.label.length <= 80)
      .map((/** @type {any} */ item) => ({ value: item.model_id, label: item.label,
        requiredGpu: typeof item.required_gpu === 'string' && GPU_NAME.test(item.required_gpu) ? item.required_gpu : null })),
  )
  const analysisModels = $derived(
    Array.isArray(allocation.analysis_models) ? allocation.analysis_models : [],
  )
  const analysisOptions = $derived(
    (Array.isArray(allocation.analysis_options) ? allocation.analysis_options : [])
      .filter((/** @type {any} */ item) => item &&
        (item.capability === 'text_regions_rt@1' || item.capability === 'text_mask_sam_ts@1') &&
        typeof item.label === 'string' && item.label.length <= 80),
  )
  const analysisGraphGb = $derived(
    typeof allocation.analysis_graph_bytes === 'number' && allocation.analysis_graph_bytes > 0
      ? (allocation.analysis_graph_bytes / 1e9).toFixed(1) : null,
  )
  const modelLicense = $derived(typeof allocation.model_license === 'string' ? allocation.model_license.slice(0, 80) : '')
  const idleSeconds = $derived(
    typeof allocation.idle_seconds === 'number' && Number.isInteger(allocation.idle_seconds) && allocation.idle_seconds > 0
      ? allocation.idle_seconds
      : null,
  )
  const idleOptions = $derived.by(() => {
    if (idleSeconds === null) return []
    const values = [...new Set([...IDLE_CHOICES, idleSeconds])].sort((a, b) => a - b)
    return values.map((seconds) => ({
      value: String(seconds),
      label: t('settings.cloud.setup.review.idleOption', { count: Math.round((seconds / 60) * 10) / 10 }),
    }))
  })
  const weightsGb = $derived.by(() => {
    const bytes = allocation.model_weights_bytes
    const gb = typeof bytes === 'number' && Number.isFinite(bytes) && bytes > 0 ? bytes / 1e9 : DEFAULT_WEIGHTS_GB
    return (Math.round(gb * 10) / 10).toFixed(1)
  })

  const cleanupResources = $derived(
    (Array.isArray(cleanupPlan?.resources_to_delete) ? cleanupPlan.resources_to_delete : [])
      .filter((/** @type {any} */ item) => item && typeof item.name === 'string')
      .map((/** @type {any} */ item) => ({ type: item.resource_type ?? item.type, name: item.name.slice(0, 160) })),
  )
  const ignoredCount = $derived(
    Array.isArray(cleanupPlan?.foreign_resources_ignored) ? cleanupPlan.foreign_resources_ignored.length : 0,
  )

  const health = $derived(healthOf(run))

  /** The step running now, for the live region: the last one started. */
  const currentStep = $derived.by(() => {
    const steps = run?.steps ?? []
    for (let index = steps.length - 1; index >= 0; index -= 1) {
      if (steps[index].state === 'running') return t(setupStepKey(steps[index].id))
    }
    return t('settings.cloud.setup.running.starting')
  })

  /**
   * A refused `apply` whose plan changed created nothing, so there is nothing
   * to resume: the way on is a new plan.
   */
  const resumable = $derived(
    !(run?.op === 'apply' && run.errorCode === 'ERR_UNAPPROVED_PLAN') && run?.errorCode !== 'ERR_ORPHANED_TOKEN',
  )

  /**
   * A Modal access token may exist that the setup never recorded, so neither
   * Resume nor Cleanup can find it. Resume stays refused until the setup is
   * cleaned up; the way on is the three steps the failed view lists, in the
   * catalogue's words (the helper's own steps are free text and never shown).
   */
  const orphanedToken = $derived(view === 'failed' && run?.errorCode === 'ERR_ORPHANED_TOKEN')

  /** The provider the key fields are for on this screen. */
  const keysFor = $derived(view === 'connect' || view === 'review' ? provider : (target?.provider ?? provider))

  /* ---------- actions ---------- */

  /** @param {'modal'|'beam'} which */
  function chooseProvider(which) {
    if (planning) return
    provider = which
    accountName = ''
    planError = null
    planCode = null
  }

  /** @param {string|null} code */
  function planFailed(code) {
    planCode = code
    planError = setupErrorKey(code)
  }

  /** Connect → Review: read the account, then plan. Nothing is created. */
  async function continueToReview() {
    if (planning || !hasKeys(provider)) return
    const pressed = document.activeElement
    planning = true
    planError = null
    planCode = null
    const which = provider
    try {
      const inspected = await runner({ op: 'inspect', provider: which, params: { credentials: credentials(which) } })
      if (!alive) return
      if (inspected?.success !== true) {
        planFailed(errorCodeOf(inspected) ?? 'ERR_EXECUTION_FAILED')
        return
      }
      if (inspected.data?.eligible === false) {
        planError = 'settings.cloud.setup.connect.notEligible'
        return
      }
      accountName = typeof inspected.data?.workspace_name === 'string' ? inspected.data.workspace_name.slice(0, 120) : ''
      if (await makePlan(which, {})) page = 'review'
    } catch {
      if (alive) planFailed('ERR_EXECUTION_FAILED')
    } finally {
      if (alive) planning = false
      // Still on this screen: the button that was disabled under the cursor
      // dropped focus. A new screen focuses its own heading.
      if (alive && page === 'connect') void refocus(pressed)
    }
  }

  /**
   * @param {'modal'|'beam'} which
   * @param {import('./provisioning.svelte.js').SetupOptions} options
   * @returns {Promise<boolean>} whether a plan came back
   */
  async function makePlan(which, options) {
    const planned = await runner({
      op: 'plan',
      provider: which,
      params: { credentials: credentials(which), installation_id: installationId, options },
    })
    if (!alive) return false
    if (planned?.success !== true || !planned.data || !isValidPlanHash(planned.data.plan_hash)) {
      planFailed(errorCodeOf(planned) ?? 'ERR_EXECUTION_FAILED')
      return false
    }
    plan = planned.data
    planOptions = options
    approved = false
    return true
  }

  /**
   * A different GPU or idle window is a different plan: plan again, and ask
   * for the approval again.
   *
   * @param {import('./provisioning.svelte.js').SetupOptions} options
   */
  async function replan(options) {
    if (planning) return
    const changed = document.activeElement
    planning = true
    planError = null
    planCode = null
    try {
      await makePlan(provider, cleanOptions(options))
    } catch {
      if (alive) planFailed('ERR_EXECUTION_FAILED')
    } finally {
      if (alive) planning = false
      if (alive) void refocus(changed)
    }
  }

  /**
   * A control disabled while it had focus drops it, and WebKit does not give
   * it back. Put it back once the control is usable again, unless the person
   * has moved on.
   *
   * @param {Element|null} element
   */
  async function refocus(element) {
    await tick()
    if (!alive || !(element instanceof HTMLElement) || !element.isConnected) return
    const active = document.activeElement
    if (active && active !== document.body) return
    element.focus()
  }

  /** After a refused plan: plan again from the keys, which are still here. */
  function startAgain() {
    leaveRun()
    backToConnect()
  }

  function backToConnect() {
    if (planning) return
    plan = null
    approved = false
    planError = null
    planCode = null
    page = 'connect'
  }

  /**
   * @param {Parameters<typeof runSetup>[0]} spec
   */
  function begin(spec) {
    target = { provider: spec.provider, installationId: spec.installationId, options: cleanOptions(spec.params.options) }
    const pending = runSetup(spec, runner, backend)
    // `runSetup` puts the run in the store before its first await.
    adoptedId = setup.run?.id ?? 0
    return pending
  }

  function startSetup() {
    if (busy || !plan || !approved || !hasKeys(provider)) return
    void begin({
      op: 'apply',
      provider,
      installationId,
      params: {
        credentials: credentials(provider),
        installation_id: installationId,
        approved_plan_hash: plan.plan_hash,
        options: planOptions,
        forget_setup_credential: true,
      },
    })
  }

  function resume() {
    const who = target
    if (busy || !who || !hasKeys(who.provider)) return
    void begin({
      op: 'resume',
      provider: who.provider,
      installationId: who.installationId,
      params: {
        credentials: credentials(who.provider),
        installation_id: who.installationId,
        options: who.options,
        forget_setup_credential: true,
      },
    })
  }

  async function stop() {
    if (stopping || run?.status !== 'running') return
    stopping = true
    const delivered = await stopSetup(backend)
    if (!delivered && alive) stopping = false
  }

  /** Leave a finished run's screen for another. */
  function leaveRun() {
    if (run && run.status !== 'running') clearSetupRun()
    adoptedId = 0
  }

  /** What would Clean up delete? Asks the helper's journal; nothing changes. */
  async function startCleanup() {
    const who = target
    if (busy || !who) return
    leaveRun()
    page = 'cleanup'
    cleanupPlanning = true
    cleanupPlan = null
    cleanupCode = null
    cleanupApproved = false
    cleanedEmpty = false
    const outcome = await planCleanup({ provider: who.provider, installationId: who.installationId }, runner)
    if (!alive) return
    cleanupPlanning = false
    if (outcome.ok) cleanupPlan = outcome.plan
    else cleanupCode = outcome.errorCode
  }

  async function confirmCleanup() {
    const who = target
    if (busy || !who || !cleanupPlan) return
    if (cleanupResources.length === 0) {
      await finishEmptyCleanup(who)
      return
    }
    if (!cleanupApproved || !hasKeys(who.provider) || !isValidPlanHash(cleanupPlan.plan_hash)) return
    target = { ...who }
    const pending = applyCleanup(
      {
        provider: who.provider,
        installationId: who.installationId,
        credentials: credentials(who.provider),
        planHash: cleanupPlan.plan_hash,
      },
      runner,
      backend,
    )
    adoptedId = setup.run?.id ?? 0
    await pending
  }

  /**
   * Nothing that setup made is left: forget the setup, and the endpoint it
   * saved, without a helper run.
   *
   * @param {{provider: 'modal'|'beam', installationId: string}} who
   */
  async function finishEmptyCleanup(who) {
    if (setup.unfinished?.installationId === who.installationId) forgetUnfinished()
    try {
      await removeEndpoint({ provider: who.provider, profileId: who.installationId }, backend)
    } catch {
      // Still listed; Remove takes it away.
    }
    if (!alive) return
    cleanedEmpty = true
    clearSecrets()
    call(oncleaned)
  }

  function backFromCleanup() {
    if (busy) return
    if (task?.action === 'cleanup') {
      close()
      return
    }
    page = 'resume'
  }

  async function copyHelpLink() {
    try {
      await globalThis.navigator?.clipboard?.writeText(HELP_URLS[keysFor])
      if (alive) copied = true
    } catch {
      if (alive) copied = false
    }
  }

  function close() {
    if (busy) return
    clearSecrets()
    leaveRun()
    call(onclose)
  }
</script>

{#snippet keyFields(which)}
  <div class="keys">
    {#if which === 'modal'}
      <div class="key">
        <label class="key-label" for={ids.tokenCommand}>{t('settings.cloud.setup.connect.modalCommand')}</label>
        <TextInput
          id={ids.tokenCommand}
          value={modalCommand}
          onchange={importModalCommand}
          type="password"
          autocomplete="off"
          autocapitalize="off"
          spellcheck="false"
          maxlength="1024"
          aria-invalid={modalCommandError}
          disabled={busy}
        />
        {#if modalCommandError}<p class="field-error" role="alert">{t('settings.cloud.setup.connect.modalCommandInvalid')}</p>{/if}
        {#if modalProfile}<p class="note" role="status">{t('settings.cloud.setup.connect.modalCommandImported', { profile: modalProfile })}</p>{/if}
      </div>
      <p class="note">{t('settings.cloud.setup.connect.modalCommandOr')}</p>
      <div class="key">
        <label class="key-label" for={ids.tokenId}>{t('settings.cloud.setup.connect.modalTokenId')}</label>
        <TextInput
          id={ids.tokenId}
          value={modalTokenId}
          onchange={setModalTokenId}
          type="password"
          autocomplete="off"
          autocapitalize="off"
          spellcheck="false"
          disabled={busy}
        />
      </div>
      <div class="key">
        <label class="key-label" for={ids.tokenSecret}>{t('settings.cloud.setup.connect.modalTokenSecret')}</label>
        <TextInput
          id={ids.tokenSecret}
          value={modalTokenSecret}
          onchange={setModalTokenSecret}
          type="password"
          autocomplete="off"
          autocapitalize="off"
          spellcheck="false"
          disabled={busy}
        />
      </div>
    {:else}
      <div class="key">
        <label class="key-label" for={ids.beamToken}>{t('settings.cloud.setup.connect.beamToken')}</label>
        <TextInput
          id={ids.beamToken}
          value={beamToken}
          onchange={(/** @type {string} */ value) => (beamToken = value)}
          type="password"
          autocomplete="off"
          autocapitalize="off"
          spellcheck="false"
          disabled={busy}
        />
      </div>
    {/if}
    <button
      type="button"
      class="help-toggle"
      aria-expanded={helpOpen}
      aria-controls={helpOpen ? ids.help : undefined}
      onclick={() => {
        helpOpen = !helpOpen
        copied = false
      }}
    >
      <Icon name="help" size={12} />
      <span>{t('settings.cloud.setup.connect.help')}</span>
    </button>
    {#if helpOpen}
      <div class="help" id={ids.help}>
        <p>
          {which === 'modal'
            ? t('settings.cloud.setup.connect.helpModal')
            : t('settings.cloud.setup.connect.helpBeam')}
        </p>
        <div class="help-link">
          <code class="mono">{HELP_URLS[which]}</code>
          <Button size="sm" onclick={copyHelpLink}>
            {copied ? t('settings.cloud.setup.connect.copied') : t('settings.cloud.setup.connect.copyLink')}
          </Button>
        </div>
      </div>
    {/if}
  </div>
{/snippet}

{#snippet problem(key, code)}
  <div class="problem" role="alert">
    <Icon name="warning-triangle" size={13} />
    <div>
      <p>{t(key)}</p>
      {#if code}<p class="code">{t('settings.cloud.setup.failed.code', { code })}</p>{/if}
    </div>
  </div>
{/snippet}

{#snippet checklist()}
  {#if run}
    <ol class="steps" aria-labelledby={ids.heading}>
      {#if run.steps.length === 0}
        <li class="step" class:running={run.status === 'running'}>
          <span class="mark" aria-hidden="true"><span class="pulse"></span></span>
          <span class="step-label">{t('settings.cloud.setup.running.starting')}</span>
          <span class="step-time">{run.status === 'running' ? clock(now - run.startedAt) : ''}</span>
        </li>
      {/if}
      {#each run.steps as step (step.id)}
        {@const state = shownState(step)}
        <li class="step {state}">
          <span class="mark" aria-hidden="true">
            {#if state === 'done'}
              <Icon name="check" size={12} />
            {:else if state === 'fail'}
              <Icon name="warning-triangle" size={12} />
            {:else if state === 'running'}
              <span class="pulse"></span>
            {:else}
              <span class="dot"></span>
            {/if}
          </span>
          <span class="step-label">
            {t(setupStepKey(step.id))}
            <span class="vh">{t(STATE_KEYS[state])}</span>
          </span>
          <span class="step-time">
            {#if state === 'skip'}
              {t('settings.cloud.setup.state.skip')}
            {:else if state === 'running' && step.pct !== null}
              {step.pct}%
            {:else}
              {stepTime(step)}
            {/if}
          </span>
          {#if state === 'running' && step.pct !== null}
            <span class="bar" aria-hidden="true"><span style:transform="scaleX({step.pct / 100})"></span></span>
          {/if}
        </li>
      {/each}
    </ol>
  {/if}
{/snippet}

{#snippet body()}
  <svelte:element
    this={inline ? 'h4' : 'h3'}
    class="heading"
    id={ids.heading}
    tabindex="-1"
    bind:this={heading}
  >
    {headingText}
  </svelte:element>

  {#if view === 'connect'}
    <p class="lead">{t('settings.cloud.setup.connect.lead')}</p>
    <fieldset class="providers" disabled={planning}>
      <legend class="vh">{t('settings.cloud.setup.connect.provider')}</legend>
      {#each PROVIDERS as option (option)}
        <label class="provider" class:chosen={provider === option}>
          <input
            type="radio"
            name="{uid}-provider"
            value={option}
            checked={provider === option}
            onchange={() => chooseProvider(option)}
          />
          <span class="provider-text">
            <span class="provider-name">{t(providerKey(option))}</span>
            <span class="provider-note">
              {option === 'modal'
                ? t('settings.cloud.setup.connect.modalNote')
                : t('settings.cloud.setup.connect.beamNote')}
            </span>
          </span>
        </label>
      {/each}
    </fieldset>
    {@render keyFields(provider)}
    <p class="note">
      {provider === 'beam'
        ? t('settings.cloud.setup.connect.keyNoteBeam')
        : t('settings.cloud.setup.connect.keyNoteModal')}
    </p>
    {#if planError}{@render problem(planError, planCode)}{/if}
  {:else if view === 'review'}
    <p class="lead">{t('settings.cloud.setup.review.lead', { providerKey: providerKey(provider) })}</p>
    {#if accountName}
      <div class="fixed">
        <span class="fixed-label">{t('settings.cloud.setup.review.workspace')}</span>
        <span class="mono">{accountName}</span>
      </div>
    {/if}
    <ul class="resources">
      {#each planResources as item, index (index)}
        <li>
          <span class="resource-label">{t(resourceKey(item.type))}</span>
          <code class="mono">{item.name}</code>
        </li>
      {/each}
    </ul>
    {#if modelOptions.length > 1}
      <Field label={t('settings.cloud.setup.review.model')} layout="row" controlId={ids.model}>
        {#snippet children()}
          <Select
            id={ids.model}
            options={modelOptions}
            value={model}
            fit
            disabled={planning}
            onchange={(/** @type {string} */ value) => {
              const selected = modelOptions.find((item) => item.value === value)
              replan({ ...planOptions, model_id: value, ...(selected?.requiredGpu ? { gpu: selected.requiredGpu } : {}) })
            }}
          />
        {/snippet}
      </Field>
    {:else if model}
      <div class="fixed">
        <span class="fixed-label">{t('settings.cloud.setup.review.model')}</span>
        <span class="mono">{model}</span>
      </div>
    {/if}
    {#if model}<p class="note">{t('settings.cloud.setup.review.modelNote')}</p>{/if}
    {#if modelLicense}<p class="note">{t('settings.cloud.setup.review.modelLicense', { license: modelLicense })}</p>{/if}
    {#if analysisOptions.length > 0}
      <fieldset class="analysis-models" disabled={planning}>
        <legend id={ids.analysis}>{t('settings.cloud.setup.review.analysisModels')}</legend>
        {#each analysisOptions as item (item.capability)}
          <label>
            <input type="checkbox" checked={analysisModels.includes(item.capability)}
              onchange={(event) => replan({ ...planOptions,
                analysis_models: event.currentTarget.checked
                  ? [...analysisModels, item.capability]
                  : analysisModels.filter((/** @type {string} */ capability) => capability !== item.capability),
              })} />
            <span>{item.label}</span>
          </label>
        {/each}
        <p class="note">{t('settings.cloud.setup.review.analysisNote')}</p>
      </fieldset>
    {/if}
    {#if gpuOptions.length > 0 || gpu || idleSeconds !== null}
      <div class="choices">
        {#if gpuOptions.length > 0}
          <Field label={t('settings.cloud.setup.review.gpu')} layout="row" controlId={ids.gpu}>
            {#snippet children()}
              <Select
                id={ids.gpu}
                options={gpuOptions}
                value={gpu ?? gpuOptions[0].value}
                fit
                disabled={planning}
                onchange={(/** @type {string} */ value) => replan({ ...planOptions, gpu: value })}
              />
            {/snippet}
          </Field>
        {:else if gpu}
          <div class="fixed">
            <span class="fixed-label">{t('settings.cloud.setup.review.gpu')}</span>
            <span class="mono">{gpu}</span>
          </div>
        {/if}
        {#if idleSeconds !== null}
          <Field label={t('settings.cloud.setup.review.idle')} layout="row" controlId={ids.idle}>
            {#snippet children()}
              <Select
                id={ids.idle}
                options={idleOptions}
                value={String(idleSeconds)}
                fit
                disabled={planning}
                onchange={(/** @type {string} */ value) => replan({ ...planOptions, idle_seconds: Number(value) })}
              />
            {/snippet}
          </Field>
        {/if}
      </div>
    {/if}
    <ul class="notes">
      <li>{t('settings.cloud.setup.review.weights', { size: weightsGb })}</li>
      {#if analysisGraphGb}<li>{t('settings.cloud.setup.review.analysisWeights', { size: analysisGraphGb })}</li>{/if}
      <li>{t('settings.cloud.setup.review.costGpu', { providerKey: providerKey(provider) })}</li>
      <li>{t('settings.cloud.setup.review.costIdle')}</li>
      <li>
        {provider === 'beam'
          ? t('settings.cloud.setup.review.tokenBeam')
          : t('settings.cloud.setup.review.tokenModal')}
      </li>
    </ul>
    <label class="approve">
      <input
        type="checkbox"
        checked={approved}
        disabled={planning}
        onchange={(event) => (approved = event.currentTarget.checked)}
      />
      <span>{t('settings.cloud.setup.review.approve', { providerKey: providerKey(provider) })}</span>
    </label>
    {#if planError}{@render problem(planError, planCode)}{/if}
    <Disclosure variant="plain" open={detailsOpen} ontoggle={(/** @type {boolean} */ open) => (detailsOpen = open)}>
      {#snippet summary()}{t('settings.cloud.setup.review.details')}{/snippet}
      <dl class="details">
        <dt>{t('settings.cloud.setup.review.hash')}</dt>
        <dd><code class="mono">{plan?.plan_hash}</code></dd>
        <dt>{t('settings.cloud.setup.review.installation')}</dt>
        <dd><code class="mono">{installationId}</code></dd>
      </dl>
    </Disclosure>
  {:else if view === 'running' || view === 'cleaning'}
    <p class="lead">
      {view === 'cleaning' ? t('settings.cloud.setup.running.cleanupLead') : t('settings.cloud.setup.running.lead')}
    </p>
    {@render checklist()}
    {#if run}
      <p class="elapsed">{t('settings.cloud.setup.running.elapsed', { time: clock(now - run.startedAt) })}</p>
    {/if}
  {:else if view === 'failed' || view === 'cleanupFailed'}
    {@render problem(
      view === 'cleanupFailed' ? 'settings.cloud.setup.error.cleanup' : setupErrorKey(run?.errorCode),
      run?.errorCode ?? null,
    )}
    {#if orphanedToken}
      <div class="recover">
        <svelte:element this={inline ? 'h5' : 'h4'} class="recover-heading" id="{uid}-recover">
          {t('settings.cloud.setup.orphaned.heading')}
        </svelte:element>
        <ol class="recover-steps" aria-labelledby="{uid}-recover">
          <li>{t('settings.cloud.setup.orphaned.dashboard')}</li>
          <li>{t('settings.cloud.setup.orphaned.cleanup')}</li>
          <li>{t('settings.cloud.setup.orphaned.again')}</li>
        </ol>
      </div>
    {/if}
    {@render checklist()}
    {#if view === 'failed' && resumable}
      <p class="note">{t('settings.cloud.setup.failed.kept')}</p>
    {/if}
    {#if target && resumable && askKeys}
      <p class="note">{t('settings.cloud.setup.failed.keys')}</p>
      {@render keyFields(target.provider)}
    {/if}
  {:else if view === 'done'}
    {@const saved = profileOf(run)}
    {#if saved && health?.ok}
      <p class="lead">{t('settings.cloud.setup.done.saved', { name: saved.name })}</p>
      <p class="health ok">
        <Icon name="check" size={12} />
        <span>{t(healthKey(health.status))}</span>
        {#if health.latency !== null}
          <span class="latency">{t('settings.inference.health.latency', { latency: health.latency })}</span>
        {/if}
      </p>
      <p class="note">{t('settings.cloud.setup.done.tryIt')}</p>
    {:else if saved}
      <!-- Saved and selected, but its first check did not pass: never
           "ready". No check at all reads as one that could not be made. -->
      <p class="health">
        <Icon name="warning-triangle" size={12} />
        <span>{t(healthKey(health?.status))}</span>
      </p>
      <p class="lead">{t('settings.cloud.setup.done.unchecked', { name: saved.name })}</p>
    {:else}
      <p class="lead">{t('settings.cloud.setup.done.nothing')}</p>
    {/if}
  {:else if view === 'resume' && target}
    <p class="lead">
      {task?.action === 'resume'
        ? t('settings.cloud.setup.resume.reissue', { providerKey: providerKey(target.provider), id: target.installationId })
        : t('settings.cloud.setup.resume.lead', { providerKey: providerKey(target.provider), id: target.installationId })}
    </p>
    {#if askKeys}{@render keyFields(target.provider)}{/if}
  {:else if view === 'cleanup' && target}
    {#if cleanupPlanning}
      <p class="lead" aria-live="polite">{t('settings.cloud.setup.cleanup.planning')}</p>
    {:else if cleanupCode}
      {@render problem(setupErrorKey(cleanupCode), cleanupCode)}
    {:else if cleanupPlan && cleanupResources.length === 0}
      <p class="lead">{t('settings.cloud.setup.cleanup.none', { providerKey: providerKey(target.provider) })}</p>
    {:else if cleanupPlan}
      <p class="lead">{t('settings.cloud.setup.cleanup.lead', { providerKey: providerKey(target.provider) })}</p>
      <ul class="resources">
        {#each cleanupResources as item, index (index)}
          <li>
            <span class="resource-label">{t(resourceKey(item.type))}</span>
            <code class="mono">{item.name}</code>
          </li>
        {/each}
      </ul>
      {#if ignoredCount > 0}
        <p class="note">{t('settings.cloud.setup.cleanup.ignored', { count: ignoredCount })}</p>
      {/if}
      {#if askKeys}{@render keyFields(target.provider)}{/if}
      <label class="approve">
        <input
          type="checkbox"
          checked={cleanupApproved}
          onchange={(event) => (cleanupApproved = event.currentTarget.checked)}
        />
        <span>{t('settings.cloud.setup.cleanup.approve')}</span>
      </label>
    {/if}
  {:else if view === 'cleaned'}
    <p class="lead">
      {cleanedEmpty
        ? t('settings.cloud.setup.cleanup.doneEmpty', { providerKey: providerKey(target?.provider ?? provider) })
        : t('settings.cloud.setup.cleanup.done', { providerKey: providerKey(target?.provider ?? provider) })}
    </p>
  {/if}

  {#if view === 'running' || view === 'cleaning'}
    <p class="vh" aria-live="polite">{currentStep}</p>
  {/if}
{/snippet}

{#snippet footer()}
  {#if view === 'connect'}
    <Button onclick={close} disabled={planning}>{t('shell.action.cancel')}</Button>
    <Button variant="primary" onclick={continueToReview} disabled={planning || !hasKeys(provider)}>
      {planning ? t('settings.cloud.setup.connect.checking') : t('settings.cloud.setup.connect.continue')}
    </Button>
  {:else if view === 'review'}
    <Button onclick={backToConnect} disabled={planning}>{t('settings.cloud.setup.review.back')}</Button>
    <Button
      variant="primary"
      onclick={startSetup}
      disabled={planning || !approved || !plan || !hasKeys(provider)}
    >
      {planning ? t('settings.cloud.setup.connect.checking') : t('settings.cloud.setup.review.start')}
    </Button>
  {:else if view === 'running' || view === 'cleaning'}
    <Button onclick={stop} disabled={stopping}>
      {stopping ? t('settings.cloud.setup.running.stopping') : t('settings.cloud.setup.running.stop')}
    </Button>
  {:else if orphanedToken}
    <!-- No Resume: the helper refuses it until the setup is cleaned up. -->
    <Button onclick={close}>{t('shell.action.close')}</Button>
    <Button variant="primary" onclick={startCleanup}>{t('settings.cloud.setup.failed.cleanup')}</Button>
  {:else if view === 'failed' && !resumable}
    <Button onclick={close}>{t('shell.action.close')}</Button>
    <Button variant="primary" onclick={startAgain}>{t('settings.cloud.setup.failed.again')}</Button>
  {:else if view === 'failed' || view === 'resume'}
    <Button onclick={close}>{t('shell.action.close')}</Button>
    <Button onclick={startCleanup}>{t('settings.cloud.setup.failed.cleanup')}</Button>
    <Button variant="primary" onclick={resume} disabled={!target || !hasKeys(target.provider)}>
      {t('settings.cloud.setup.failed.resume')}
    </Button>
  {:else if view === 'cleanupFailed'}
    <Button onclick={close}>{t('shell.action.close')}</Button>
    <Button variant="primary" onclick={startCleanup}>{t('settings.cloud.setup.failed.retry')}</Button>
  {:else if view === 'cleanup'}
    <Button onclick={backFromCleanup} disabled={cleanupPlanning}>{t('settings.cloud.setup.review.back')}</Button>
    {#if cleanupCode}
      <Button variant="primary" onclick={startCleanup}>{t('settings.cloud.setup.failed.retry')}</Button>
    {:else if cleanupPlan && cleanupResources.length === 0}
      <Button variant="primary" onclick={confirmCleanup}>{t('settings.cloud.setup.cleanup.forget')}</Button>
    {:else}
      <Button
        variant="primary"
        onclick={confirmCleanup}
        disabled={cleanupPlanning || !cleanupPlan || !cleanupApproved || !target || !hasKeys(target.provider)}
      >
        {t('settings.cloud.setup.cleanup.delete')}
      </Button>
    {/if}
  {:else}
    <Button variant="primary" onclick={close}>{t('shell.action.close')}</Button>
  {/if}
{/snippet}

{#if inline}
  <section class="prov" bind:this={root} aria-labelledby={ids.heading} aria-busy={busy}>
    {@render body()}
    <div class="foot">{@render footer()}</div>
  </section>
{:else}
  <Modal title={t('settings.cloud.setup.title')} width={480} onclose={busy ? undefined : close}>
    <div class="prov" bind:this={root} aria-busy={busy}>{@render body()}</div>
    {#snippet buttons()}{@render footer()}{/snippet}
  </Modal>
{/if}

<style>
  .prov {
    display: grid;
    gap: var(--s-4);
    min-width: 0;
  }

  .heading {
    margin: 0;
    font-size: 13px;
    font-weight: 600;
    color: var(--text);
    outline: none;
  }

  .lead,
  .note,
  .elapsed {
    margin: 0;
    line-height: 1.5;
  }
  .lead { color: var(--t2) }
  .note,
  .elapsed {
    font-size: 11.5px;
    color: var(--t3);
  }

  .mono {
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    font-size: 11.5px;
    color: var(--t2);
    overflow-wrap: anywhere;
  }

  /* Visually hidden, still read. */
  .vh {
    position: absolute;
    width: 1px;
    height: 1px;
    margin: -1px;
    padding: 0;
    overflow: hidden;
    clip: rect(0 0 0 0);
    white-space: nowrap;
    border: 0;
  }

  /* ---------- connect ---------- */

  .providers {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(150px, 1fr));
    gap: var(--s-3);
    margin: 0;
    padding: 0;
    border: 0;
    min-width: 0;
  }

  .provider {
    display: flex;
    gap: var(--s-3);
    align-items: flex-start;
    padding: 10px 12px;
    border: 1px solid var(--line);
    border-radius: var(--r-md);
    background: var(--panel2);
    cursor: pointer;
    transition: border-color var(--dur-fast) var(--ease);
  }
  .provider:hover { border-color: var(--line2) }
  .provider.chosen { border-color: var(--accent) }
  /* The radio itself carries focus; the card shows it too, without relying
     on :focus-visible, which the shipped WebKit applies differently. */
  .provider:focus-within { border-color: var(--accent); box-shadow: 0 0 0 3px var(--accent-soft) }
  .provider input {
    margin: 2px 0 0;
    accent-color: var(--accent);
  }
  .provider-text { display: grid; gap: 2px; min-width: 0 }
  .provider-name { font-weight: 600; color: var(--text) }
  .provider-note { font-size: 11.5px; color: var(--t3); line-height: 1.4 }

  .keys { display: grid; gap: var(--s-3) }
  .key { display: grid; gap: var(--s-1) }
  .key-label { font-size: 11.5px; color: var(--t2) }
  .field-error { margin: 0; font-size: 11.5px; color: var(--warn) }

  .help-toggle {
    display: inline-flex;
    gap: var(--s-2);
    align-items: center;
    justify-self: start;
    padding: 2px 0;
    border: 0;
    background: none;
    color: var(--t2);
    font-size: 11.5px;
    cursor: pointer;
  }
  .help-toggle:hover { color: var(--text) }
  .help-toggle:focus { color: var(--text) }

  .help {
    display: grid;
    gap: var(--s-2);
    padding: 10px 12px;
    border-radius: var(--r-md);
    background: var(--panel2);
  }
  .help p { margin: 0; line-height: 1.5; color: var(--t2) }
  .help-link {
    display: flex;
    gap: var(--s-3);
    align-items: center;
    justify-content: space-between;
    min-width: 0;
  }

  /* ---------- review ---------- */

  .resources {
    display: grid;
    gap: 6px;
    margin: 0;
    padding: 10px 12px;
    border: 1px solid var(--line);
    border-radius: var(--r-md);
    list-style: none;
  }
  .resources li {
    display: flex;
    flex-wrap: wrap;
    gap: 2px var(--s-3);
    align-items: baseline;
    justify-content: space-between;
    min-width: 0;
  }
  .resource-label { color: var(--text) }

  .choices { display: grid; gap: var(--s-2) }
  .analysis-models {
    display: grid;
    gap: var(--s-2);
    margin: var(--s-2) 0;
    padding: var(--s-3) 0 0;
    border: 0;
    border-top: 1px solid var(--line);
  }
  .analysis-models legend { padding: 0; color: var(--text) }
  .analysis-models label { display: flex; gap: var(--s-2); align-items: center; cursor: pointer }
  .analysis-models input { accent-color: var(--accent) }
  .fixed {
    display: flex;
    gap: var(--s-3);
    align-items: baseline;
    justify-content: space-between;
  }
  .fixed-label { color: var(--t2) }

  .notes {
    display: grid;
    gap: 5px;
    margin: 0;
    padding-left: 16px;
    color: var(--t2);
    line-height: 1.5;
  }

  .approve {
    display: flex;
    gap: var(--s-3);
    align-items: flex-start;
    line-height: 1.5;
    color: var(--text);
    cursor: pointer;
  }
  .approve input {
    flex: none;
    margin: 3px 0 0;
    accent-color: var(--accent);
  }

  .details {
    display: grid;
    gap: 3px;
    margin: 0;
  }
  .details dt { font-size: 11px; color: var(--t3) }
  .details dd { margin: 0 0 6px }

  /* ---------- progress ---------- */

  .steps {
    display: grid;
    gap: 2px;
    margin: 0;
    padding: 0;
    list-style: none;
  }
  .step {
    position: relative;
    display: grid;
    grid-template-columns: 16px 1fr auto;
    gap: var(--s-3);
    align-items: center;
    min-height: 26px;
    padding: 0 2px;
    color: var(--t2);
  }
  .step.running { color: var(--text) }
  .step.skip { color: var(--t3) }
  .step.fail { color: var(--warn) }
  .mark {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 16px;
  }
  .step.done .mark { color: var(--t2) }
  .pulse,
  .dot {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: currentColor;
  }
  .pulse { animation: mcBlink 1.2s var(--ease) infinite }
  .dot { width: 5px; height: 5px; opacity: .6 }
  .step-label { min-width: 0 }
  .step-time {
    font-size: 11.5px;
    font-variant-numeric: tabular-nums;
    color: var(--t3);
  }
  .bar {
    grid-column: 2 / 4;
    height: 2px;
    margin: -2px 0 4px;
    overflow: hidden;
    border-radius: 1px;
    background: var(--line);
  }
  .bar span {
    display: block;
    height: 100%;
    background: var(--accent);
    transform-origin: left center;
    transition: transform var(--dur) var(--ease);
  }

  /* ---------- outcomes ---------- */

  .problem {
    display: flex;
    gap: var(--s-3);
    align-items: flex-start;
    padding: 10px 12px;
    border-radius: var(--r-md);
    background: var(--panel2);
    color: var(--warn);
  }
  .problem p { margin: 0; line-height: 1.5; color: var(--text) }
  .problem .code {
    margin-top: 2px;
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    font-size: 11px;
    color: var(--t3);
  }
  .problem :global(svg) { flex: none; margin-top: 2px }

  /* The way out of a problem that has one, in order. Numbered because the
     order matters: the token goes before Clean up can finish the job. */
  .recover { display: grid; gap: var(--s-2) }
  .recover-heading {
    margin: 0;
    font-size: 12px;
    font-weight: 600;
    color: var(--text);
  }
  .recover-steps {
    display: grid;
    gap: 6px;
    margin: 0;
    padding-left: 18px;
    color: var(--t2);
    line-height: 1.5;
  }
  .recover-steps li::marker { color: var(--t3); font-variant-numeric: tabular-nums }

  .health {
    display: flex;
    gap: var(--s-2);
    align-items: center;
    margin: 0;
    color: var(--warn);
  }
  .health.ok { color: var(--t2) }
  .health span { color: var(--text) }
  .health .latency { color: var(--t3); font-variant-numeric: tabular-nums }

  /* ---------- footer (inline form; the dialog form uses the modal's) ---------- */

  .foot {
    display: flex;
    flex-wrap: wrap;
    gap: var(--s-2);
    justify-content: flex-end;
    padding-top: var(--s-3);
    border-top: 1px solid var(--line);
  }
</style>
