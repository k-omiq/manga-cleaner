<script>
  /**
   * Setup step 5: how the app behaves day to day.
   *
   * Two of Settings > General's rows, with Settings' names: what closing the
   * window does, and which way new projects read. Language is not asked,
   * because there is one catalogue; the question comes back when there are
   * two.
   */
  import { Field, Segmented } from '../../ui/index.js'
  import { t } from '../../i18n/index.js'
  import { session, setCloseToTray, setReadingDirection } from '../../state/session.svelte.js'
  import { saveFirstLaunchSetting } from '../firstlaunch.svelte.js'

  const uid = $props.id()
  let saveFailed = $state(false)

  const directions = [
    { value: 'rtl', label: t('settings.direction.rtl') },
    { value: 'ltr', label: t('settings.direction.ltr') },
  ]

  /**
   * @template T
   * @param {(value: T) => void} apply
   * @param {T} next
   * @param {T} previous
   */
  async function save(apply, next, previous) {
    saveFailed = !(await saveFirstLaunchSetting(apply, next, previous))
  }

  /**
   * The box follows the session afterwards, as Settings' does. A refused
   * write puts the session back, but the click has already moved the box,
   * and a refusal that lands before Svelte next draws is a value that never
   * changed as far as Svelte can tell.
   *
   * @param {HTMLInputElement} input
   */
  async function changeTray(input) {
    await save(setCloseToTray, input.checked, session.closeToTray)
    input.checked = session.closeToTray
  }
</script>

<p class="lead">{t('onboarding.behavior.body')}</p>

<div class="rows">
  <Field
    label={t('settings.background.label')}
    description={t('onboarding.behavior.trayNote')}
    layout="row"
    controlId="{uid}-tray"
  >
    {#snippet children({ descriptionId })}
      <input
        id="{uid}-tray"
        class="check"
        type="checkbox"
        checked={session.closeToTray}
        aria-describedby={descriptionId}
        onchange={(event) => changeTray(/** @type {HTMLInputElement} */ (event.currentTarget))}
      />
    {/snippet}
  </Field>

  <Field
    label={t('settings.direction.label')}
    description={t('onboarding.behavior.directionNote')}
    layout="row"
  >
    {#snippet children({ labelId, descriptionId })}
      <Segmented
        options={directions}
        value={session.readingDirection}
        labelledBy={labelId}
        describedBy={descriptionId}
        onchange={(value) => save(setReadingDirection, value, session.readingDirection)}
      />
    {/snippet}
  </Field>
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

  .check {
    margin: 0;
    accent-color: var(--accent);
  }
</style>
