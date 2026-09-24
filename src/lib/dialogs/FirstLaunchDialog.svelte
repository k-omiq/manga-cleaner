<script>
  /**
   * The setup a first launch opens, and Settings' "Run setup again".
   *
   * Six steps, one choice each (`FIRST_LAUNCH_STEPS`), every one of them
   * skippable. This file is the frame around them: the step heading, Back,
   * the one primary action, the quiet Skip, and the keyboard. What a step
   * shows is its own component in `onboarding/`, and everything that has to
   * outlive a mount - the plan, the download run, what the cloud setup did -
   * is the store in `firstlaunch.svelte.js`, because a dialog raised over
   * this one unmounts it (`App.svelte`).
   *
   * Every way out goes through `dismissFirstLaunch`, which records that the
   * offer was made. Escape follows the Modal's rule and the backdrop does not
   * close: a stray click must not end a setup halfway. While the provisioner
   * is working in the user's account there is no way out here at all, only
   * its own Cancel.
   */
  import { tick } from 'svelte'
  import { Button, Modal } from '../ui/index.js'
  import { t } from '../i18n/index.js'
  import { session } from '../state/session.svelte.js'
  import { openNewProject } from '../home/actions.js'
  import { FIRST_LAUNCH_STEPS } from './firstlaunch.js'
  import {
    closeFirstLaunchProvisioner,
    dismissFirstLaunch,
    firstLaunch,
    nextFirstLaunchStep,
    openFirstLaunchProvisioner,
    pendingBytes,
    pendingQueue,
    prevFirstLaunchStep,
    startFirstLaunchDownloads,
  } from './firstlaunch.svelte.js'
  import ModelsStep from './onboarding/ModelsStep.svelte'
  import DefaultsStep from './onboarding/DefaultsStep.svelte'
  import CloudStep from './onboarding/CloudStep.svelte'
  import BehaviorStep from './onboarding/BehaviorStep.svelte'
  import DoneStep from './onboarding/DoneStep.svelte'

  /** Whole keys chosen between, never built, so the catalogue test sees each one. */
  const HEADINGS = {
    welcome: 'onboarding.welcome.heading',
    models: 'onboarding.models.heading',
    defaults: 'onboarding.defaults.heading',
    cloud: 'onboarding.cloud.heading',
    behavior: 'onboarding.behavior.heading',
    done: 'onboarding.done.heading',
  }

  const headingId = $props.id()
  const step = $derived(firstLaunch.step)
  const position = $derived(FIRST_LAUNCH_STEPS.indexOf(/** @type {any} */ (step)) + 1)

  /** @type {HTMLElement|undefined} */
  let heading = $state()

  /**
   * Whether Download is the models step's primary action: nothing is
   * fetching, nothing is paused or failed (those have their own button on
   * the step), and something ticked is still missing.
   */
  const offerDownload = $derived(
    !firstLaunch.running && !firstLaunch.paused && !firstLaunch.failure && pendingQueue().length > 0,
  )

  /**
   * @typedef {{label: string, run: () => unknown, enter?: boolean}} Action
   *
   * What the footer offers on this step: Back or not, at most one button
   * beside the primary, and the primary itself. `enter` is whether Enter on
   * the heading may press the primary. It is withheld from Download, so a
   * key pressed twice on the step before cannot start a transfer of hundreds
   * of megabytes.
   */
  const actions = $derived.by(() => {
    /** @type {Action} */
    const next = { label: t('onboarding.action.next'), run: nextFirstLaunchStep, enter: true }
    if (step === 'welcome') {
      return {
        back: false,
        secondary: null,
        primary: { label: t('onboarding.action.start'), run: nextFirstLaunchStep, enter: true },
      }
    }
    if (step === 'models' && offerDownload) {
      return {
        back: true,
        secondary: { label: t('onboarding.action.notNow'), run: nextFirstLaunchStep },
        primary: {
          label: t('onboarding.models.download', { bytes: pendingBytes() }),
          run: startFirstLaunchDownloads,
          enter: false,
        },
      }
    }
    if (step === 'cloud' && firstLaunch.provisioning) {
      // The provisioner draws its own buttons. Once it has finished, Continue
      // is offered here too, so leaving does not depend on which button its
      // last screen happens to have.
      return {
        back: false,
        secondary: null,
        primary: firstLaunch.cloud ? { label: t('onboarding.action.next'), run: leaveProvisioner } : null,
      }
    }
    if (step === 'cloud' && !firstLaunch.cloud && !session.cloudAllowed) {
      return {
        back: true,
        secondary: { label: t('onboarding.cloud.setUp'), run: openFirstLaunchProvisioner },
        primary: { label: t('onboarding.action.notNow'), run: nextFirstLaunchStep, enter: true },
      }
    }
    if (step === 'done') {
      return {
        back: true,
        secondary: null,
        primary: { label: t('home.action.newProject'), run: startProject, enter: true },
      }
    }
    return { back: true, secondary: null, primary: next }
  })

  function leaveProvisioner() {
    closeFirstLaunchProvisioner()
    nextFirstLaunchStep()
  }

  /**
   * Close the setup, then raise New project over whatever it was opened on.
   * The tick lets the setup's Modal hand focus back before the next dialog
   * takes it, so closing that one returns focus to a real element.
   */
  async function startProject() {
    dismissFirstLaunch()
    await tick()
    openNewProject()
  }

  // A new step moves focus to its heading, so a screen reader starts where
  // the user now is and Enter can press the step's primary action. Not while
  // the provisioner is up: the cloud step puts focus inside it instead, and
  // closing it brings focus back here.
  $effect(() => {
    void step
    if (firstLaunch.provisioning) return
    tick().then(() => heading?.focus())
  })

  /**
   * Enter presses the primary action, but only from the heading or the
   * dialog itself. Anywhere else Enter already belongs to the focused
   * control, and taking it from a button or a select would change what that
   * control does.
   *
   * @param {KeyboardEvent} event
   */
  function onkeydown(event) {
    if (event.key !== 'Enter' || event.repeat || event.isComposing || event.defaultPrevented) return
    if (event.altKey || event.ctrlKey || event.metaKey || event.shiftKey) return
    const target = event.target
    if (!(target instanceof HTMLElement) || !heading) return
    const fromFrame = target === heading || (target.getAttribute('role') === 'dialog' && target.contains(heading))
    if (!fromFrame) return
    const primary = actions.primary
    if (!primary?.enter) return
    event.preventDefault()
    primary.run()
  }
</script>

<svelte:window {onkeydown} />

<Modal
  title={t('onboarding.title')}
  meta={t('onboarding.stepOf', { current: position, total: FIRST_LAUNCH_STEPS.length })}
  width={640}
  blocking
  onclose={firstLaunch.provisionerBusy
    ? undefined
    : firstLaunch.provisioning
      ? closeFirstLaunchProvisioner
      : dismissFirstLaunch}
>
  {#key step}
    <section class="step" aria-labelledby={headingId}>
      <h3 class="heading" id={headingId} tabindex="-1" bind:this={heading}>{t(HEADINGS[step])}</h3>
      {#if step === 'welcome'}
        <p class="lead">{t('onboarding.welcome.body')}</p>
        <p class="lead">{t('onboarding.welcome.local')}</p>
        <p class="note">{t('onboarding.welcome.steps')}</p>
      {:else if step === 'models'}
        <ModelsStep />
      {:else if step === 'defaults'}
        <DefaultsStep />
      {:else if step === 'cloud'}
        <CloudStep />
      {:else if step === 'behavior'}
        <BehaviorStep />
      {:else}
        <DoneStep />
      {/if}
    </section>
  {/key}

  {#snippet footnote()}
    <Button variant="plain" size="sm" disabled={firstLaunch.provisionerBusy} onclick={dismissFirstLaunch}>
      {step === 'done' ? t('onboarding.action.close') : t('onboarding.action.skip')}
    </Button>
  {/snippet}

  {#snippet buttons()}
    {#if actions.back}
      <Button onclick={prevFirstLaunchStep}>{t('onboarding.action.back')}</Button>
    {/if}
    {#if actions.secondary}
      <Button onclick={actions.secondary.run}>{actions.secondary.label}</Button>
    {/if}
    {#if actions.primary}
      <Button variant="primary" onclick={actions.primary.run}>{actions.primary.label}</Button>
    {/if}
  {/snippet}
</Modal>

<style>
  /* A floor rather than a fixed height: the dialog keeps roughly one size
     from step to step instead of jumping with each step's content, and a
     short screen still gets the Modal's own scrolling body. */
  .step {
    display: flex;
    flex-direction: column;
    gap: var(--s-4);
    min-height: 288px;
    padding: var(--s-2) 0 var(--s-2);
    animation: mcIn var(--dur-slow) var(--ease);
  }

  .heading {
    margin: 0 0 var(--s-1);
    font-size: 17px;
    font-weight: 600;
    line-height: 1.3;
    color: var(--text);
  }
  /* The heading takes focus only so a screen reader starts there. It is not a
     control, and a ring around it would read as one. */
  .heading:focus { outline: none }

  /* The steps' shared type, set here so the six read as one piece. Each step
     component uses these classes rather than restating them. */
  .step :global(.lead) {
    margin: 0;
    max-width: 58ch;
    font-size: 13px;
    line-height: 1.55;
    color: var(--t2);
  }
  .step :global(.note) {
    margin: 0;
    max-width: 62ch;
    font-size: 11.5px;
    line-height: 1.5;
    color: var(--t2);
  }
  .step :global(.alert) {
    margin: 0;
    font-size: 11.5px;
    line-height: 1.5;
    color: var(--warn);
  }
</style>
