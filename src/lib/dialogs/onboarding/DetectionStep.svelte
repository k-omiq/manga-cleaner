<script>
  /**
   * Setup step: which detector each language uses, or none, and whether the
   * optional Japanese OCR rescue is wanted.
   *
   * A skipped language asks for no detection files, and the rescue is off
   * until it is ticked, so a first setup fetches neither OCR file. The table
   * below says what each engine is made of and what it costs, including the
   * ones that are not ready yet, so the choice reads as a roadmap. The
   * text-shaped review is not offered here: its graphs are imported later, in
   * Settings, and one line says so.
   */
  import { Select } from '../../ui/index.js'
  import { t } from '../../i18n/index.js'
  import { session } from '../../state/session.svelte.js'
  import EngineTable from '../EngineTable.svelte'
  import { ALL_TEXT_POLICY, DETECTORS, LANGUAGES, OCR_FILES, detectorsFor, engineBytes } from '../../model/pipelines.js'
  import { chooseDetector, chooseOcrRescue, finishedFiles, firstLaunch } from '../firstlaunch.svelte.js'

  const uid = $props.id()

  /** The Select's value for "skip this language"; stored as `null`. */
  const SKIP = ''

  /** @param {string} language */
  function options(language) {
    return [
      ...detectorsFor(language).map((engine) => ({ value: engine.id, label: engine.name })),
      { value: SKIP, label: t('pipelines.skip') },
    ]
  }

  /** @param {import('../../model/pipelines.js').Engine} engine */
  function stateOf(engine) {
    if (!engine.ready) return t('pipelines.status.soon')
    const bytes = firstLaunch.plan ? engineBytes(engine, firstLaunch.plan.files, finishedFiles()) : 0
    return bytes > 0 ? t('models.value.size', { bytes }) : t('pipelines.status.installed')
  }

  /** Setup keeps the policy it finds; a replay after choosing all-text says so. */
  const allText = $derived(session.textPolicy === ALL_TEXT_POLICY)

  /**
   * What ticking the rescue means with these choices: nothing to read when
   * Japanese is skipped, nothing fetched under all-text, otherwise its cost.
   */
  const rescueLine = $derived.by(() => {
    if (allText) return ''
    if (!firstLaunch.detection.ja) return session.ocrRescue ? t('settings.detection.rescue.skipped') : ''
    const plan = firstLaunch.plan
    const bytes = plan ? engineBytes({ files: OCR_FILES }, plan.files, finishedFiles()) : 0
    return bytes > 0 ? t('settings.detection.rescue.size', { bytes }) : ''
  })
</script>

<p class="lead">{t('onboarding.detection.body')}</p>
{#if allText}<p class="note policy">{t('settings.detection.setupAllText')}</p>{/if}
<div class="languages">
  {#each LANGUAGES as language (language.id)}
    <div class="language" class:skipped={!firstLaunch.detection[language.id]}>
      <span class="name">{t(language.labelKey)}</span>
      <Select
        options={options(language.id)}
        value={firstLaunch.detection[language.id] ?? SKIP}
        label={t('pipelines.detectorFor', { language: t(language.labelKey) })}
        onchange={(value) => chooseDetector(language.id, value || null)}
      />
    </div>
  {/each}
</div>
<!-- Opt-in, and the same switch Settings > Detection shows. The description
     and the cost are read with the box. -->
<div class="rescue">
  <input
    id="{uid}-rescue"
    type="checkbox"
    checked={session.ocrRescue}
    aria-describedby="{uid}-rescue-description {uid}-rescue-line"
    onchange={(event) => chooseOcrRescue(event.currentTarget.checked)}
  />
  <div class="rescue-text">
    <label for="{uid}-rescue">{t('pipelines.workflow.ocrRescue')}</label>
    <p id="{uid}-rescue-description">{t('pipelines.workflow.ocrRescueDescription')}</p>
    <p id="{uid}-rescue-line" class="rescue-line" role="status">{rescueLine}</p>
  </div>
</div>
<EngineTable engines={DETECTORS} label={t('pipelines.detection')} {stateOf} />
<p class="note">{t('settings.detection.setupReview')}</p>

<style>
  .policy { margin: 0 0 var(--s-5) }
  .languages { display: grid; gap: var(--s-2); margin-bottom: var(--s-5) }
  .language {
    display: grid;
    grid-template-columns: 110px minmax(0, 280px);
    align-items: center;
    gap: var(--s-5);
  }
  .name { font-weight: 600 }
  .skipped .name { color: var(--t3) }

  /* The engine table's checkbox shape: box left, title over its note. */
  .rescue {
    display: flex;
    align-items: flex-start;
    gap: var(--s-4);
    max-width: 62ch;
    margin-bottom: var(--s-7);
  }
  .rescue input { flex: none; margin: 2px 0 0; accent-color: var(--accent); cursor: pointer }
  .rescue-text { display: grid; gap: 2px; min-width: 0 }
  .rescue-text label { font-weight: 600; cursor: pointer }
  .rescue-text p { margin: 0; color: var(--t2); font-size: 11.5px; line-height: 1.45 }
</style>
