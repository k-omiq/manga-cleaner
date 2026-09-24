<script>
  /**
   * Setup step 2: the files cleaning needs, as three choices rather than eight
   * rows, and the one run that fetches them.
   *
   * The Download button itself is the dialog's primary action. What is here
   * is what it will fetch and, once it has been pressed, how far it has got,
   * with Pause, Resume or Try again beside the bar. Everything is read from
   * the store, because the run outlives this step: a user who presses
   * Download and moves on finds the same bar when they come back.
   */
  import Icon from '../../icons/Icon.svelte'
  import { Button } from '../../ui/index.js'
  import { hasKey, t } from '../../i18n/index.js'
  import {
    RUNTIME_ID,
    groupBytes,
    groupState,
    labelKeyFor,
    planGroups,
    runProgress,
    runtimeNotice,
  } from '../firstlaunch.js'
  import {
    firstLaunch,
    pauseFirstLaunchDownloads,
    pendingQueue,
    setFirstLaunchTick,
    startFirstLaunchDownloads,
  } from '../firstlaunch.svelte.js'

  /** Each group's name, chosen between rather than built. */
  const LABELS = {
    required: 'onboarding.models.required.label',
    redraw: 'models.kind.inpainter',
    japanese: 'models.kind.ocr',
  }

  /** The right-hand column's word for a group in the run. */
  const STATUS = {
    installed: 'onboarding.models.status.installed',
    failed: 'onboarding.models.status.failed',
    downloading: 'onboarding.models.status.downloading',
    waiting: 'onboarding.models.status.waiting',
    paused: 'onboarding.models.status.paused',
  }

  const uid = $props.id()
  const plan = $derived(firstLaunch.plan)
  const groups = $derived(planGroups(plan))
  const started = $derived(
    firstLaunch.running || firstLaunch.paused || firstLaunch.failure !== null || firstLaunch.sequenceDone,
  )
  const progress = $derived(runProgress(plan, firstLaunch.selection, firstLaunch.finished, firstLaunch.progress))
  const percent = $derived(
    progress.total > 0 ? Math.min(100, Math.floor((progress.done / progress.total) * 100)) : 0,
  )

  /** @param {import('../firstlaunch.js').PlanGroup} group */
  function noteKey(group) {
    if (group.id === 'redraw') return 'onboarding.models.redraw.note'
    if (group.id === 'japanese') return 'onboarding.models.japanese.note'
    return group.rows.some((row) => row.id === RUNTIME_ID)
      ? 'onboarding.models.required.note'
      : 'onboarding.models.required.noteNoRuntime'
  }

  /** @param {import('../firstlaunch.js').PlanGroup} group */
  function ticked(group) {
    return !group.optional || group.rows.some((row) => firstLaunch.selection[row.id] === true)
  }

  /**
   * An optional group is one tick for all of its files: the Japanese reader
   * is three files and none of them reads anything alone.
   *
   * @param {import('../firstlaunch.js').PlanGroup} group
   * @param {boolean} wanted
   */
  function choose(group, wanted) {
    for (const row of group.rows) setFirstLaunchTick(row.id, wanted)
  }

  /** What the run is doing, in one sentence. */
  const runText = $derived.by(() => {
    if (firstLaunch.failure) {
      const nameKey = labelKeyFor(plan, firstLaunch.failure.id)
      return nameKey
        ? t('onboarding.models.run.failed', { nameKey })
        : t('onboarding.models.run.failedAny')
    }
    if (firstLaunch.running) {
      const nameKey = firstLaunch.current ? labelKeyFor(plan, firstLaunch.current) : null
      return nameKey
        ? t('onboarding.models.run.downloading', { nameKey })
        : t('onboarding.models.run.working')
    }
    if (firstLaunch.paused) return t('onboarding.models.run.paused')
    return t('onboarding.models.run.done')
  })

  /**
   * The failure's own sentence, when the backend has one (the runtime's
   * refusals, such as too little disk space), else what it said, as Settings
   * > Models shows it.
   */
  const detail = $derived.by(() => {
    const message = firstLaunch.failure?.message
    if (!message) return ''
    const notice = runtimeNotice(message)
    return notice && hasKey(notice.key) ? t(notice.key, notice.params) : message
  })
</script>

<p class="lead">{t('onboarding.models.body')}</p>

{#if plan?.runtimeUnavailable}
  <p class="alert">{t('onboarding.models.runtimeUnavailable')}</p>
{/if}

<ul class="groups">
  {#each groups as group (group.id)}
    {@const status = groupState(group, firstLaunch)}
    {@const here = status === 'installed'}
    <li class="group">
      <input
        id="{uid}-{group.id}"
        class="check"
        type="checkbox"
        checked={here || ticked(group)}
        disabled={!group.optional || here || firstLaunch.running}
        aria-describedby="{uid}-{group.id}-note"
        onchange={(event) => choose(group, /** @type {HTMLInputElement} */ (event.currentTarget).checked)}
      />
      <div class="text">
        <label class="name" for="{uid}-{group.id}">{t(LABELS[group.id])}</label>
        <span class="about" id="{uid}-{group.id}-note">{t(noteKey(group))}</span>
      </div>
      <span class="value" class:done={here}>
        {#if here}<Icon name="check" size={13} />{/if}
        {status ? t(STATUS[status]) : t('models.value.size', { bytes: groupBytes(group, firstLaunch.finished) })}
      </span>
    </li>
  {/each}
</ul>

{#if started}
  <div class="run">
    <div class="line">
      <p class="state" role="status">{runText}</p>
      <span class="amount">{t('onboarding.models.amount', { done: progress.done, total: progress.total })}</span>
      {#if firstLaunch.running}
        <Button size="sm" onclick={pauseFirstLaunchDownloads}>{t('onboarding.models.pause')}</Button>
      {:else if firstLaunch.failure}
        <Button size="sm" onclick={startFirstLaunchDownloads}>{t('onboarding.models.retry')}</Button>
      {:else if firstLaunch.paused}
        <Button size="sm" onclick={startFirstLaunchDownloads}>{t('onboarding.models.resume')}</Button>
      {/if}
    </div>
    <div
      class="bar"
      role="progressbar"
      aria-label={t('onboarding.models.progressLabel')}
      aria-valuemin="0"
      aria-valuemax="100"
      aria-valuenow={percent}
    >
      <div class="fill" style:transform="scaleX({percent / 100})"></div>
    </div>
    {#if detail}
      <p class="alert" role="alert">{detail}</p>
    {/if}
    {#if firstLaunch.running}
      <p class="note">{t('onboarding.models.background')}</p>
    {/if}
  </div>
{:else if pendingQueue().length === 0 && !plan?.runtimeUnavailable}
  <p class="note ready"><Icon name="check" size={13} />{t('onboarding.models.ready')}</p>
{/if}

<style>
  .groups {
    margin: 0;
    padding: 0;
    list-style: none;
    border-top: 1px solid var(--line);
  }

  .group {
    display: grid;
    grid-template-columns: auto 1fr auto;
    align-items: start;
    gap: var(--s-4);
    padding: var(--s-4) 0;
    border-bottom: 1px solid var(--line);
  }

  .check {
    margin: 2px 0 0;
    accent-color: var(--accent);
  }

  .text {
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
  }
  .name {
    font-size: 12.5px;
    color: var(--text);
  }
  .about {
    font-size: 11.5px;
    line-height: 1.45;
    color: var(--t2);
  }

  .value {
    display: inline-flex;
    align-items: center;
    gap: var(--s-1);
    font-size: 11.5px;
    color: var(--t2);
    white-space: nowrap;
  }
  .value.done { color: var(--text) }

  .run {
    display: flex;
    flex-direction: column;
    gap: var(--s-3);
  }
  .line {
    display: flex;
    align-items: center;
    gap: var(--s-4);
    min-height: 26px;
  }
  .state {
    flex: 1;
    min-width: 0;
    margin: 0;
    font-size: 12px;
    color: var(--text);
  }
  .amount {
    font-size: 11.5px;
    color: var(--t2);
    white-space: nowrap;
  }

  .bar {
    height: 4px;
    overflow: hidden;
    border-radius: var(--r-pill);
    background: var(--accent-soft);
  }
  .fill {
    height: 100%;
    background: var(--accent);
    transform-origin: left center;
    transition: transform var(--dur) var(--ease);
  }

  .ready {
    display: flex;
    align-items: center;
    gap: var(--s-2);
  }
</style>
