<script>
  /**
   * Setup step: which detector each language uses, or none.
   *
   * A skipped language asks for no detection files. The table below says what
   * each engine is made of and what it costs, including the ones that are
   * not ready yet, so the choice reads as a roadmap.
   */
  import { Select } from '../../ui/index.js'
  import { t } from '../../i18n/index.js'
  import EngineTable from '../EngineTable.svelte'
  import { DETECTORS, LANGUAGES, detectorsFor, engineBytes } from '../../model/pipelines.js'
  import { chooseDetector, finishedFiles, firstLaunch } from '../firstlaunch.svelte.js'

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
</script>

<p class="lead">{t('onboarding.detection.body')}</p>
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
<EngineTable engines={DETECTORS} label={t('pipelines.detection')} {stateOf} />

<style>
  .languages { display: grid; gap: var(--s-2); margin-bottom: var(--s-7) }
  .language {
    display: grid;
    grid-template-columns: 110px minmax(0, 280px);
    align-items: center;
    gap: var(--s-5);
  }
  .name { font-weight: 600 }
  .skipped .name { color: var(--t3) }
</style>
