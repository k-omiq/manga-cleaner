<script>
  /**
   * Setup step 4: the optional cloud GPU.
   *
   * Two lines, and the dialog's two buttons: Not now leaves cloud cleaning
   * off, Set up now puts the provisioner in place of the lines (IC-5). The
   * provisioner owns everything about the setup itself, secrets included;
   * what comes back here is only that it finished and whether its endpoint
   * answered the first check, the permission that a passed check turns on
   * (`configureFirstLaunchCloud`), and whether it is busy in the user's
   * account, which the dialog reads to stay open. An endpoint that did not
   * answer is said back too, with where to test it, and is not offered for
   * setup again.
   */
  import { tick } from 'svelte'
  import { t } from '../../i18n/index.js'
  import { session } from '../../state/session.svelte.js'
  import { focusable } from '../../ui/focus.js'
  import CloudProvisioner from '../CloudProvisioner.svelte'
  import {
    closeFirstLaunchProvisioner,
    configureFirstLaunchCloud,
    firstLaunch,
    setFirstLaunchProvisionerBusy,
  } from '../firstlaunch.svelte.js'

  /** @type {HTMLElement|undefined} */
  let panel = $state()

  // The provisioner replaces the step's content, so focus goes into it rather
  // than staying on a button that is no longer there.
  $effect(() => {
    if (!firstLaunch.provisioning) return
    tick().then(() => {
      if (panel) (focusable(panel)[0] ?? panel).focus()
    })
  })

  const status = $derived.by(() => {
    const saved = firstLaunch.cloud
    if (saved?.healthy) {
      return saved.name ? t('onboarding.cloud.ready', { name: saved.name }) : t('onboarding.cloud.readyUnnamed')
    }
    if (saved) {
      return saved.name
        ? t('onboarding.cloud.unchecked', { name: saved.name })
        : t('onboarding.cloud.uncheckedUnnamed')
    }
    return session.cloudAllowed ? t('onboarding.cloud.on') : ''
  })
</script>

{#if firstLaunch.provisioning}
  <div class="provisioner" bind:this={panel} tabindex="-1">
    <CloudProvisioner
      inline
      initialProvider="modal"
      onclose={closeFirstLaunchProvisioner}
      onconfigured={configureFirstLaunchCloud}
      onbusychange={setFirstLaunchProvisionerBusy}
    />
  </div>
{:else}
  <p class="lead">{t('onboarding.cloud.body')}</p>
  <p class="lead">{t('onboarding.cloud.consent')}</p>
  {#if firstLaunch.cloudSaveFailed}
    <p class="alert" role="alert">{t('onboarding.cloud.saveFailed')}</p>
  {:else if status}
    <p class="note" role="status">{status}</p>
  {/if}
{/if}

<style>
  .provisioner { outline: none }
</style>
