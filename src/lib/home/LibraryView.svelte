<script>
  import { Button, Empty, SectionLabel } from '../ui/index.js'
  import { t } from '../i18n/index.js'
  import ProjectGrid from './ProjectGrid.svelte'
  import { library, loadLibrary, resumeProject } from './library.svelte.js'
  import { openNewChapter, openNewProject } from './actions.js'
  import { projectProgress } from '../model/progress.js'

  /**
   * Level one: every project. The two centred actions, the PROJECTS label, the
   * card grid, and the two things that can be true instead of a grid - still
   * loading, and nothing here at all.
   *
   * There is no search field and no sort control. The design file has neither,
   * and a library that can look empty because of a filter is the most alarming
   * false state a project manager can show.
   */

  const rows = $derived(
    library.projects.map((project) => ({ project, progress: projectProgress(project) })),
  )
  const noProjects = $derived(library.projects.length === 0)

  const uid = $props.id()
  const reasonId = `${uid}-reason`
</script>

<section class="library">
  <div class="actions">
    <div class="slot">
      <Button
        variant="primary"
        size="hero"
        block
        disabled={noProjects}
        aria-describedby={noProjects ? reasonId : undefined}
        onclick={() => openNewChapter(null)}
      >
        {t('home.action.newChapter')}
      </Button>
    </div>
    <div class="slot">
      <Button variant="soft" size="hero" block onclick={openNewProject}>
        {t('home.action.newProject')}
      </Button>
    </div>

    <!-- A disabled control has to say why. A disabled button is neither
         focusable nor hoverable, so a `title` would never be read or seen:
         the reason is on the screen, beside the control it is about. -->
    {#if noProjects}
      <p class="reason" id={reasonId}>{t('home.hint.newChapterNeedsProject')}</p>
    {/if}
  </div>

  <SectionLabel text={t('home.section.projects')} as="h2" />

  <div class="body">
    {#if library.status === 'loading'}
      <Empty title={t('home.state.loading')} />
    {:else if library.status === 'failed'}
      <Empty title={t('home.state.failedTitle')} body={t('home.state.failedBody')}>
        <Button onclick={() => loadLibrary()}>{t('home.action.retry')}</Button>
      </Empty>
    {:else if noProjects}
      <Empty title={t('home.empty.libraryTitle')} />
    {:else}
      <ProjectGrid {rows} onresume={(project) => resumeProject(project.id)} />
    {/if}
  </div>
</section>

<style>
  .library { display: block }

  .actions {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 10px;
    padding: 168px 0 152px;
  }
  .slot { width: 212px; max-width: 100% }

  .reason {
    max-width: 260px;
    margin: 2px 0 0;
    font-size: 10.5px;
    line-height: 1.5;
    text-align: center;
    color: var(--t3);
  }

  .body { margin-top: 18px }

  /* The design file's 168/152 drop is a first-impression, not a rule; on a
     short window it would put the grid entirely below the fold. */
  @media (max-height: 700px) {
    .actions { padding: 88px 0 80px }
  }
</style>
