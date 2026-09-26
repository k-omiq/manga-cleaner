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
  import { session, setDetectorModels } from '../../state/session.svelte.js'
  import { ALL_TEXT_POLICY, DETECTOR_MODEL_IDS, LANGUAGES, OCR_FILES, engineBytes, model } from '../../model/pipelines.js'
  import { chooseDetector, chooseOcrRescue, finishedFiles, firstLaunch } from '../firstlaunch.svelte.js'

  const uid = $props.id()

  /** The Select's value for "skip this language"; stored as `null`. */
  const SKIP = ''

  /** @param {string} language */
  function options(language) {
    return [
      { value: 'ctd-rtdetr', label: t('pipelines.clean') },
      { value: SKIP, label: t('pipelines.skip') },
    ]
  }

  /** Setup keeps the policy it finds; a replay after choosing all-text says so. */
  const allText = $derived(session.textPolicy === ALL_TEXT_POLICY)

  function chooseModel(id, checked) {
    let models = session.detectorModels.filter((selected) => selected !== id)
    if (checked) {
      if (id === 'rtFull') models = models.filter((selected) => selected !== 'rtSmall')
      if (id === 'rtSmall') models = models.filter((selected) => selected !== 'rtFull')
      models.push(id)
    }
    if (models.length) setDetectorModels(models)
  }

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
<fieldset class="detection-models">
  <legend>Detection models</legend>
  {#each DETECTOR_MODEL_IDS as id (id)}
    <label>
      <input type="checkbox" checked={session.detectorModels.includes(id)}
        disabled={session.detectorModels.length === 1 && session.detectorModels.includes(id)}
        onchange={(event) => chooseModel(id, event.currentTarget.checked)} />
      {model(id)?.product}
    </label>
  {/each}
</fieldset>
<p class="note">{t('settings.detection.selectedModels', { models: session.detectorModels.map((id) => model(id)?.product).filter(Boolean).join(' + ') })}</p>
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
<p class="note">{t('settings.detection.setupReview')}</p>

<style>
  .detection-models { display: flex; flex-wrap: wrap; gap: var(--s-4); margin-bottom: var(--s-5); border: 0; padding: 0 }
  .detection-models legend { font-weight: 600; margin-bottom: var(--s-2) }
  .detection-models label { display: flex; align-items: center; gap: var(--s-2) }
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
