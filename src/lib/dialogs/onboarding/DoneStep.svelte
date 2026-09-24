<script>
  /**
   * Setup step 6: what was chosen, read back from where it is now kept, and
   * the one way to start (the dialog's New project button).
   *
   * Every value is read from the session and the store rather than
   * remembered from the steps, so a step that was skipped reads back as the
   * default it left in place, and a download still running says how far it
   * has got.
   */
  import { t } from '../../i18n/index.js'
  import { capabilities } from '../../state/capabilities.svelte.js'
  import { session } from '../../state/session.svelte.js'
  import { runProgress } from '../firstlaunch.js'
  import { firstLaunch, loadFirstLaunchSidecarModels } from '../firstlaunch.svelte.js'

  const plan = $derived(firstLaunch.plan)
  // No plan is a catalogue that could not be read, which is not evidence that
  // anything is installed.
  const requiredHere = $derived(
    plan !== null &&
      plan.required.every((row) => row.installed || firstLaunch.finished[row.id] === true),
  )
  const progress = $derived(runProgress(plan, firstLaunch.selection, firstLaunch.finished, firstLaunch.progress))
  const percent = $derived(
    progress.total > 0 ? Math.min(100, Math.floor((progress.done / progress.total) * 100)) : 0,
  )

  const lead = $derived(
    requiredHere
      ? t('onboarding.done.body')
      : firstLaunch.running
        ? t('onboarding.done.bodyDownloading')
        : t('onboarding.done.bodyMissing'),
  )

  const models = $derived.by(() => {
    if (firstLaunch.running) return t('onboarding.done.value.downloading', { percent })
    if (firstLaunch.failure) return t('onboarding.done.value.failed')
    if (firstLaunch.paused) return t('onboarding.done.value.paused', { percent })
    return requiredHere ? t('onboarding.done.value.installed') : t('onboarding.done.value.notDownloaded')
  })

  // The same reading the picker on step 3 shows: once the runtime has
  // answered, its `selected` provider is the stored preference as the backend
  // kept it, and no `selected` provider means Automatic.
  const accel = $derived.by(() => {
    const providers = firstLaunch.accelerators?.providers
    const id = providers
      ? (providers.find((row) => row.selected)?.id ?? 'auto')
      : session.accelerator
    if (id === 'auto') return t('settings.accel.auto')
    const provider = providers?.find((row) => row.id === id)
    return provider ? t(provider.labelKey) : t('onboarding.done.value.accelCustom')
  })

  // Read here too, so a skipped step 3 still shows the model's name rather
  // than its id. Once only: a helper with no models answers an empty list,
  // which must not ask again.
  let askedModels = false
  $effect(() => {
    if (!capabilities.sidecar || askedModels || firstLaunch.sidecarModels.length > 0) return
    askedModels = true
    loadFirstLaunchSidecarModels()
  })

  const flux = $derived(
    firstLaunch.sidecarModels.find((model) => model.id === session.fluxModel)?.label ?? session.fluxModel,
  )

  // Off can still have an endpoint behind it: one set up here that did not
  // answer its first check, or whose permission could not be saved.
  const cloud = $derived.by(() => {
    const name = firstLaunch.cloud?.name
    if (!session.cloudAllowed) {
      return name ? t('onboarding.done.value.cloudOffSaved', { name }) : t('onboarding.done.value.cloudOff')
    }
    return name ? t('onboarding.done.value.cloudOnNamed', { name }) : t('onboarding.done.value.cloudOn')
  })
</script>

<p class="lead">{lead}</p>

<dl class="summary">
  <div class="row">
    <dt>{t('onboarding.done.summary.models')}</dt>
    <dd>{models}</dd>
  </div>
  <div class="row">
    <dt>{t('settings.accel.label')}</dt>
    <dd>{accel}</dd>
  </div>
  {#if capabilities.sidecar && flux}
    <div class="row">
      <dt>{t('settings.sidecarModel.label')}</dt>
      <dd>{flux}</dd>
    </div>
  {/if}
  <div class="row">
    <dt>{t('onboarding.done.summary.cloud')}</dt>
    <dd>{cloud}</dd>
  </div>
  <div class="row">
    <dt>{t('onboarding.done.summary.tray')}</dt>
    <dd>{session.closeToTray ? t('onboarding.done.value.trayKeep') : t('onboarding.done.value.trayQuit')}</dd>
  </div>
  <div class="row">
    <dt>{t('settings.direction.label')}</dt>
    <dd>{session.readingDirection === 'ltr' ? t('settings.direction.ltr') : t('settings.direction.rtl')}</dd>
  </div>
</dl>

<style>
  .summary {
    margin: 0;
    border-top: 1px solid var(--line);
  }

  .row {
    display: grid;
    grid-template-columns: minmax(0, 1fr) minmax(0, 1.4fr);
    gap: var(--s-4);
    padding: var(--s-3) 0;
    border-bottom: 1px solid var(--line);
    font-size: 12px;
    line-height: 1.45;
  }

  dt { color: var(--t2) }
  dd {
    margin: 0;
    color: var(--text);
    overflow-wrap: anywhere;
  }
</style>
