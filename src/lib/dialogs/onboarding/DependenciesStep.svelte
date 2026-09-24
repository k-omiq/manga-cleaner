<script>
  /**
   * Setup step: what this computer needs for the choices so far, and what of
   * it is already here.
   *
   * Everything is read from the backend rather than guessed: the platform and
   * the runtime build from `listModels`, the graphics acceleration from the
   * runtime itself once it is installed (`runtimeReady`), and the FLUX helper
   * from the capabilities. Each platform gets its own runtime build, and some
   * builds need a toolkit installed by hand, which is said here.
   */
  import { t } from '../../i18n/index.js'
  import { capabilities } from '../../state/capabilities.svelte.js'
  import { CLEANERS } from '../../model/pipelines.js'
  import { RUNTIME_ID, runtimeReady } from '../firstlaunch.js'
  import {
    chosenBytes,
    chosenFiles,
    finishedFiles,
    firstLaunch,
    loadFirstLaunchAccelerators,
  } from '../firstlaunch.svelte.js'

  /** Each platform id's name, chosen between rather than built. */
  const PLATFORM_KEYS = {
    'macos-arm64': 'onboarding.dependencies.platform.macArm',
    'macos-x64': 'onboarding.dependencies.platform.macIntel',
    'windows-x64': 'onboarding.dependencies.platform.windows',
    'windows-arm64': 'onboarding.dependencies.platform.windowsArm',
    'linux-x64': 'onboarding.dependencies.platform.linux',
    'linux-arm64': 'onboarding.dependencies.platform.linuxArm',
  }

  /** Build names worth saying. `stock` is the plain build and says nothing. */
  const FLAVOUR_NAMES = { directml: 'DirectML', cuda12: 'CUDA 12', cuda13: 'CUDA 13' }

  const plan = $derived(firstLaunch.plan)
  const done = $derived(finishedFiles())
  const files = $derived(chosenFiles())
  const pending = $derived(files.filter((id) => !plan?.files[id]?.installed && !done[id]))
  const cleaningIds = $derived(new Set(CLEANERS.flatMap((engine) => engine.files)))
  const groups = $derived([
    { key: 'pipelines.detection', ids: files.filter((id) => id !== RUNTIME_ID && !cleaningIds.has(id)) },
    { key: 'pipelines.cleaning', ids: files.filter((id) => cleaningIds.has(id)) },
  ])
  const platformName = $derived(
    t(PLATFORM_KEYS[/** @type {keyof typeof PLATFORM_KEYS} */ (plan?.platform ?? '')] ?? 'onboarding.dependencies.platform.unknown'),
  )
  const build = $derived(
    [plan?.runtime.version, FLAVOUR_NAMES[/** @type {keyof typeof FLAVOUR_NAMES} */ (plan?.runtime.flavour ?? '')]]
      .filter(Boolean)
      .join(' · '),
  )
  const ready = $derived(runtimeReady(plan, done, firstLaunch.current))

  // Asked once the runtime is here, which can happen while this step is
  // open: a run started earlier may finish underneath it.
  let asked = false
  $effect(() => {
    if (!ready || asked || firstLaunch.accelerators) return
    asked = true
    loadFirstLaunchAccelerators()
  })

  const acceleration = $derived.by(() => {
    if (!ready) return t('onboarding.dependencies.afterRuntime')
    if (firstLaunch.acceleratorsFailed) return t('onboarding.dependencies.unreadable')
    if (!firstLaunch.accelerators) return '…'
    const names = firstLaunch.accelerators.providers
      .filter((provider) => provider.available && provider.id !== 'cpu')
      .map((provider) => t(provider.labelKey))
    return names.length ? names.join(', ') : t('onboarding.dependencies.cpuOnly')
  })

  /** @param {string} id */
  function stateOf(id) {
    const row = plan?.files[id]
    if (!row || row.installed || done[id]) return t('onboarding.dependencies.installed')
    return t('onboarding.dependencies.toDownload', { bytes: row.bytes })
  }
</script>

<p class="lead">{t('onboarding.dependencies.body', { platform: platformName })}</p>

<dl class="rows">
  <div class="row">
    <dt>{t('onboarding.dependencies.runtime')}{#if build}<span class="sub">{build}</span>{/if}</dt>
    <dd class:warn={plan?.runtimeUnavailable}>
      {plan?.runtimeUnavailable ? t('onboarding.dependencies.unavailable') : stateOf(RUNTIME_ID)}
      {#if plan?.runtime.needs.length}
        <span class="need">{t('onboarding.dependencies.needs', { items: plan.runtime.needs.join(', ') })}</span>
      {/if}
    </dd>
  </div>
  <div class="row">
    <dt>{t('onboarding.dependencies.acceleration')}</dt>
    <dd>{acceleration}</dd>
  </div>
  <div class="row">
    <dt>{t('onboarding.dependencies.helper')}</dt>
    <dd>{capabilities.sidecar ? t('onboarding.dependencies.helperFound') : t('onboarding.dependencies.helperMissing')}</dd>
  </div>
</dl>

{#each groups as group (group.key)}
  {#if group.ids.length}
    <h2 class="group">{t(group.key)}</h2>
    <dl class="rows">
      {#each group.ids as id (id)}
        <div class="row"><dt>{t(plan?.files[id]?.labelKey ?? '')}</dt><dd>{stateOf(id)}</dd></div>
      {/each}
    </dl>
  {/if}
{/each}

<p class="total">
  {pending.length
    ? t('onboarding.dependencies.total', { count: pending.length, bytes: chosenBytes() })
    : t('onboarding.dependencies.nothing')}
</p>

<style>
  .rows { margin: 0 0 var(--s-6) }
  .row {
    display: flex;
    justify-content: space-between;
    gap: var(--s-6);
    padding: var(--s-4) 0;
    border-top: 1px solid var(--line);
  }
  dt { font-weight: 500 }
  .sub { margin-left: var(--s-3); color: var(--t3); font-weight: 400 }
  dd { margin: 0; color: var(--t2); text-align: right }
  dd.warn { max-width: 40ch; color: var(--warn) }
  .need { display: block; margin-top: var(--s-1); max-width: 40ch; color: var(--warn) }
  .group { margin: var(--s-6) 0 var(--s-2); font-size: 11px; font-weight: 600; color: var(--t3) }
  .total { margin: 0; color: var(--t2); font-size: 12px }
</style>
