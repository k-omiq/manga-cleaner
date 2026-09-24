<script>
  /**
   * Setup step 3: the model settings that exist today, with the recommended
   * value already in place.
   *
   * These are Settings' own rows - acceleration from the Acceleration tab,
   * the AI redraw helper from the Models tab - written through the same
   * session setters and `writeSettings`. Nothing here is a setting of its
   * own: a row that only this step had would be a promise nothing keeps.
   *
   * The AI redraw rows depend on whether a helper is installed: without one
   * there is only the folder to point at, with one there is its model and
   * engine.
   */
  import { Button, Field, Segmented, Select } from '../../ui/index.js'
  import { t } from '../../i18n/index.js'
  import { chooseFolder } from '../../api/folder.js'
  import { capabilities } from '../../state/capabilities.svelte.js'
  import {
    session,
    setAccelerator,
    setFluxBackend,
    setFluxModel,
    setSidecarPath,
  } from '../../state/session.svelte.js'
  import { runtimeReady } from '../firstlaunch.js'
  import {
    firstLaunch,
    loadFirstLaunchAccelerators,
    loadFirstLaunchSidecarModels,
    saveFirstLaunchSetting,
  } from '../firstlaunch.svelte.js'

  const uid = $props.id()
  let saveFailed = $state(false)
  let choosing = $state(false)

  const ready = $derived(runtimeReady(firstLaunch.plan, firstLaunch.finished, firstLaunch.current))

  // Asked once the runtime is here, which can happen while this step is
  // open: the run started on the step before may finish underneath it.
  let asked = false
  $effect(() => {
    if (!ready || asked || firstLaunch.accelerators) return
    asked = true
    loadFirstLaunchAccelerators()
  })

  $effect(() => {
    if (capabilities.sidecar) loadFirstLaunchSidecarModels()
  })

  /**
   * What the picker shows as chosen. Once the runtime has answered, its
   * `selected` provider is the better reading of the stored preference,
   * which is how Settings decides it too.
   */
  const accelValue = $derived(
    firstLaunch.accelerators
      ? (firstLaunch.accelerators.providers.find((provider) => provider.selected)?.id ?? 'auto')
      : session.accelerator,
  )

  /**
   * Automatic, then every provider the runtime reports. One it cannot use is
   * listed disabled with its reason, as Settings lists it, so a user looking
   * for their graphics card finds why rather than nothing.
   */
  const accelOptions = $derived.by(() => {
    const options = [
      { value: 'auto', label: t('onboarding.defaults.accel.auto'), disabled: false },
      ...(firstLaunch.accelerators?.providers ?? []).map((provider) => ({
        value: provider.id,
        label: !provider.available && provider.reasonKey
          ? `${t(provider.labelKey)}: ${t(provider.reasonKey)}`
          : t(provider.labelKey),
        disabled: !provider.available,
      })),
    ]
    // A choice made in Settings before the runtime could be asked still has
    // to be shown as the choice, not as Automatic.
    if (!options.some((option) => option.value === accelValue)) {
      options.push({ value: accelValue, label: t('onboarding.done.value.accelCustom'), disabled: false })
    }
    return options
  })

  const accelNote = $derived(
    firstLaunch.acceleratorsFailed
      ? t('onboarding.defaults.accel.unreadable')
      : ready
        ? t('onboarding.defaults.accel.note')
        : t('onboarding.defaults.accel.later'),
  )

  const fluxBackends = [
    { value: 'auto', label: t('settings.fluxBackend.auto') },
    { value: 'mflux', label: t('settings.fluxBackend.mflux') },
    { value: 'sdnq', label: t('settings.fluxBackend.sdnq') },
  ]

  const fluxModels = $derived(
    firstLaunch.sidecarModels.length > 0
      ? firstLaunch.sidecarModels.map((model) => ({ value: model.id, label: model.label }))
      : [{ value: '', label: t('settings.sidecarModel.noneFound') }],
  )

  /**
   * @template T
   * @param {(value: T) => void} apply
   * @param {T} next
   * @param {T} previous
   */
  async function save(apply, next, previous) {
    const kept = await saveFirstLaunchSetting(apply, next, previous)
    saveFailed = !kept
    return kept
  }

  /**
   * Put a picker back on the value in force after a refused write. The
   * native select moved when it was changed, and Svelte writes `value` only
   * when the value it holds changes, which a refused write leaves as it was.
   *
   * @param {string} id - the select's id
   * @param {string} value
   */
  function resync(id, value) {
    const select = document.getElementById(id)
    if (select instanceof HTMLSelectElement) select.value = value
  }

  /** @param {string} id */
  async function chooseAccelerator(id) {
    if (await save(setAccelerator, id, session.accelerator)) await loadFirstLaunchAccelerators()
    else resync(`${uid}-accel`, accelValue)
  }

  /** @param {string} id */
  async function chooseFluxModel(id) {
    if (!(await save(setFluxModel, id, session.fluxModel))) resync(`${uid}-flux-model`, session.fluxModel)
  }

  async function browse() {
    if (choosing) return
    choosing = true
    try {
      /** @type {string|null} */
      let chosen = null
      try {
        chosen = await chooseFolder({
          title: t('settings.sidecar.chooserTitle'),
          defaultPath: session.sidecarPath || undefined,
        })
      } catch {
        // A chooser that could not open chose nothing, which is also what
        // closing it does.
        chosen = null
      }
      if (chosen !== null) await save(setSidecarPath, chosen, session.sidecarPath)
    } finally {
      choosing = false
    }
  }
</script>

<p class="lead">{t('onboarding.defaults.body')}</p>

<div class="rows">
  <Field
    label={t('settings.accel.label')}
    description={accelNote}
    layout="row"
    controlId="{uid}-accel"
  >
    {#snippet children()}
      <Select
        id="{uid}-accel"
        fit
        options={accelOptions}
        value={accelValue}
        disabled={accelOptions.length < 2}
        onchange={chooseAccelerator}
      />
    {/snippet}
  </Field>

  {#if !capabilities.sidecar || session.sidecarPath}
    <Field
      label={t('settings.sidecar.label')}
      description={t('onboarding.defaults.flux.folderNote')}
      layout="row"
    >
      {#snippet children({ descriptionId })}
        <Button size="sm" disabled={choosing} onclick={browse} aria-describedby={descriptionId}>
          {t('shell.action.chooseFolder')}
        </Button>
      {/snippet}
    </Field>
    {#if session.sidecarPath}
      <p class="note path">{session.sidecarPath}</p>
      {#if !capabilities.sidecar}
        <p class="alert" role="alert">{t('settings.sidecar.notFound')}</p>
      {/if}
    {/if}
  {/if}

  {#if capabilities.sidecar}
    <Field
      label={t('settings.sidecarModel.label')}
      description={t('onboarding.defaults.flux.modelNote')}
      layout="row"
      controlId="{uid}-flux-model"
    >
      {#snippet children()}
        <Select
          id="{uid}-flux-model"
          fit
          options={fluxModels}
          value={session.fluxModel}
          disabled={firstLaunch.sidecarModels.length === 0}
          onchange={chooseFluxModel}
        />
      {/snippet}
    </Field>

    <Field
      label={t('settings.fluxBackend.label')}
      description={t('onboarding.defaults.flux.engineNote')}
      layout="row"
    >
      {#snippet children({ labelId, descriptionId })}
        <Segmented
          options={fluxBackends}
          value={session.fluxBackend}
          labelledBy={labelId}
          describedBy={descriptionId}
          onchange={(value) => save(setFluxBackend, value, session.fluxBackend)}
        />
      {/snippet}
    </Field>
  {/if}
</div>

{#if saveFailed}
  <p class="alert" role="alert">{t('onboarding.saveFailed')}</p>
{/if}

<style>
  .rows {
    display: flex;
    flex-direction: column;
    gap: var(--s-2);
  }

  /* A folder path is long and has no spaces to wrap at. */
  .path { overflow-wrap: anywhere }
</style>
