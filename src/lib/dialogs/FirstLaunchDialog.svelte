<script>
  /**
   * The first-launch download offer.
   *
   * The weights and the ONNX Runtime are downloaded after install rather than
   * bundled, and until this dialog existed the only
   * route to them was a Settings section the user had to know about: a fresh
   * install opened an editor whose Auto clean button was disabled with a
   * sentence naming Settings, and whose engine pickers showed two rungs instead
   * of five. Correct, and not the same as being asked.
   *
   * **It adds no seam method.** Everything goes through the seven the model
   * manager already uses, and progress arrives on the same process-wide
   * `model-progress` channel Settings listens to. Two dialogs hearing one
   * stream is the ordinary case that channel was built for.
   *
   * **This component holds nothing.** The plan, the ticks, the progress and
   * the sequence all live in `firstlaunch.svelte.js`, because the offer can
   * lose the screen to a dialog raised over it while a 200 MB transfer is in
   * flight - and a run whose state was in the component would
   * come back with the ticks reset and the bytes quoted again. What is left
   * here is the drawing of it.
   */
  import { Button, Modal, SectionLabel } from '../ui/index.js'
  import { t } from '../i18n/index.js'
  import { labelKeyFor } from './firstlaunch.js'
  import {
    cancelFirstLaunchDownload,
    dismissFirstLaunch,
    firstLaunch,
    pendingBytes,
    pendingQueue,
    setFirstLaunchTick,
    startFirstLaunchDownloads,
  } from './firstlaunch.svelte.js'

  const plan = $derived(firstLaunch.plan)

  /** What the primary button promises, and whether it has anything to fetch. */
  const total = $derived(pendingBytes())
  const queue = $derived(pendingQueue())

  /**
   * One row's line, as a key and its parameters. The same five states Settings
   * draws, in the same words - this is the same list of files.
   *
   * @param {import('./firstlaunch.js').PlanRow} row
   * @returns {{key: string, params?: Object}}
   */
  function statusOf(row) {
    const inFlight = firstLaunch.progress[row.id]
    if (inFlight) {
      const percent = inFlight.total
        ? Math.floor((inFlight.downloaded / inFlight.total) * 100)
        : null
      return percent === null
        ? { key: 'settings.models.status.downloading' }
        : { key: 'settings.models.status.downloadingPercent', params: { percent } }
    }
    if (row.installed || firstLaunch.finished[row.id]) {
      return { key: 'settings.models.status.installed' }
    }
    if (firstLaunch.failure?.id === row.id) return { key: 'settings.models.status.failed' }
    return { key: 'settings.models.status.missing' }
  }

  /** @param {import('./firstlaunch.js').PlanRow} row */
  function metaOf(row) {
    const status = statusOf(row)
    const size = row.bytes > 0 ? `${t('models.value.size', { bytes: row.bytes })} · ` : ''
    return `${size}${t(status.key, status.params)}`
  }
</script>

{#if plan}
  <Modal title={t('models.firstLaunch.title')} width={460} onclose={dismissFirstLaunch}>
    <p class="lede">{t('models.firstLaunch.description')}</p>

    <section class="group">
      <SectionLabel as="h3" text={t('models.firstLaunch.requiredLabel')} />
      <p class="note">{t('models.firstLaunch.requiredNote', { bytes: plan.requiredBytes })}</p>
      <!-- A platform with no published runtime keeps the weights on offer and
           says why they are not enough on their own: they are exactly what an
           offline install would need beside a hand-placed library, and hiding
           them would leave the user with nothing to press and nothing to read. -->
      {#if plan.runtimeUnavailable}
        <p class="note">{t('models.firstLaunch.runtimeUnavailable')}</p>
      {/if}
      <!-- No ticks on this group: it is what Auto clean is made of, and a
           checkbox beside four files that are useless apart would be a choice
           with no second answer. -->
      <ul class="rows">
        {#each plan.required as row (row.id)}
          <li class="row">
            <span class="row-name">{t(row.labelKey)}</span>
            <span class="row-meta">{metaOf(row)}</span>
          </li>
        {/each}
      </ul>
    </section>

    {#if plan.optional.length > 0}
      <section class="group">
        <SectionLabel as="h3" text={t('models.firstLaunch.optionalLabel')} />
        <p class="note">{t('models.firstLaunch.optionalNote')}</p>
        <ul class="rows">
          {#each plan.optional as row (row.id)}
            <li class="row tickable">
              <label class="tick">
                <input
                  type="checkbox"
                  checked={firstLaunch.selection[row.id] === true}
                  disabled={firstLaunch.running || firstLaunch.finished[row.id] === true}
                  onchange={(e) =>
                    setFirstLaunchTick(
                      row.id,
                      /** @type {HTMLInputElement} */ (e.currentTarget).checked,
                    )}
                />
                <span class="row-name">{t(row.labelKey)}</span>
              </label>
              <span class="row-meta">{metaOf(row)}</span>
            </li>
          {/each}
        </ul>
      </section>
    {/if}

    {#if firstLaunch.failure}
      <!-- `nameKey` is the row's own key, taken from the plan the queue was
           built from; a null - an id this plan never drew - interpolates to
           nothing rather than to the word `null`. -->
      <p class="report warn">
        {t('models.firstLaunch.failed', { nameKey: labelKeyFor(plan, firstLaunch.failure.id) })}
      </p>
      <p class="report detail">{firstLaunch.failure.message}</p>
    {:else if firstLaunch.stopped}
      <p class="report">{t('models.firstLaunch.stopped')}</p>
    {:else if firstLaunch.sequenceDone}
      <p class="report">{t('models.firstLaunch.done')}</p>
    {/if}

    {#snippet buttons()}
      {#if firstLaunch.running}
        <Button onclick={cancelFirstLaunchDownload}>{t('settings.models.action.cancel')}</Button>
      {:else if firstLaunch.sequenceDone}
        <Button variant="primary" onclick={dismissFirstLaunch}>{t('shell.action.done')}</Button>
      {:else}
        <Button onclick={dismissFirstLaunch}>{t('models.firstLaunch.action.notNow')}</Button>
        <!-- Disabled on an empty **queue**, not on a total of zero: a row whose
             size the view could not state counts as 0 bytes and is still a
             download worth starting. -->
        <Button
          variant="primary"
          disabled={queue.length === 0}
          onclick={startFirstLaunchDownloads}
        >
          {t('models.firstLaunch.action.download', { bytes: total })}
        </Button>
      {/if}
    {/snippet}
  </Modal>
{/if}

<style>
  .lede {
    margin: 0 0 var(--s-4);
    font-size: 11.5px;
    color: var(--t2);
    line-height: 1.5;
  }
  .group { margin-top: var(--s-4) }
  .group :global(h3) { margin-bottom: 4px }
  .note {
    margin: 0;
    font-size: 10.5px;
    color: var(--t3);
    line-height: 1.45;
  }
  /* The same table Settings › Models draws, so the two read as one list of
     files rather than as two lists of different things. */
  .rows {
    margin: var(--s-2) 0 0;
    padding: 0;
    list-style: none;
  }
  .row {
    display: flex;
    align-items: center;
    gap: var(--s-3);
    padding: var(--s-2) 0;
    border-bottom: 1px solid var(--line);
  }
  .row-name {
    flex: 1;
    min-width: 0;
    font-size: 12px;
    color: var(--text);
  }
  .row-meta {
    flex: none;
    font-size: 10.5px;
    color: var(--t3);
    line-height: 1.4;
  }
  .tick {
    display: flex;
    align-items: center;
    gap: var(--s-2);
    flex: 1;
    min-width: 0;
    cursor: pointer;
  }
  .tickable .row-name { flex: 1 }
  .report {
    margin: var(--s-3) 0 0;
    font-size: 10.5px;
    color: var(--t2);
    line-height: 1.45;
  }
  /* A failure is the one line here the reader has to act on. */
  .report.warn { color: var(--warn) }
  .report.detail { word-break: break-word }
</style>
