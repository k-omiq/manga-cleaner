<script>
  import { Button, IconButton, Menu } from '../ui/index.js'
  import { t } from '../i18n/index.js'
  import CoverArt from './CoverArt.svelte'
  import StatusMark from './StatusMark.svelte'
  import { projectMenuItems, runProjectAction } from './actions.js'
  import { interruptedChapter } from './library.svelte.js'
  import { openProject } from '../state/app.svelte.js'

  /**
   * One project in the library grid.
   *
   * The card is a single button - cover, name, numbers - with the two extra
   * controls (`Continue clean`, the context menu) in an overlay *beside* it,
   * never nested inside it. Both are tabbable only while this card is the
   * grid's active item, so a library of forty projects costs one Tab to reach
   * and one more to act on.
   *
   * @type {{
   *   project: import('../api/backend.js').ApiProject,
   *   progress: import('../model/progress.js').Progress,
   *   active: boolean,
   *   onresume: (project: import('../api/backend.js').ApiProject) => void,
   * }}
   */
  let { project, progress, active, onresume } = $props()

  const firstPage = $derived(project.chapters[0]?.pages[0] ?? null)
  const latest = $derived(project.chapters[0] ?? null)
  const interrupted = $derived(interruptedChapter(project))
  const inner = $derived(active ? 0 : -1)
  const menuLabel = $derived(t('home.menu.project', { name: project.name }))

  const resumeTitle = $derived(
    interrupted
      ? t('home.card.resumeAt', {
          chapter: interrupted.chapter.number,
          page: interrupted.pageIndex + 1,
        })
      : '',
  )
</script>

<li class="card">
  <button
    type="button"
    class="open"
    data-roving-item
    tabindex={inner}
    onclick={() => openProject(project.id)}
  >
    <span class="cover">
      <CoverArt page={firstPage} />
    </span>

    <span class="body">
      <span class="name" title={project.name}>{project.name}</span>
      {#if latest}
        <span class="line">
          {t('home.card.chapterLine', {
            count: project.chapters.length,
            chapter: latest.number,
            title: latest.name,
          })}
        </span>
      {/if}
      <span class="facts">
        <span>
          {t('home.card.cleaned', {
            cleaned: progress.pagesCleaned,
            total: progress.totalPages,
          })}
        </span>
        <span>{t(project.lastOpened.key, project.lastOpened.params)}</span>
      </span>
    </span>
  </button>

  <!-- Exactly the cover's box. Inert except for the controls inside it, so
       the card button underneath still takes every pointer event. -->
  <div class="overlay">
    <div class="chip">
      <StatusMark status={progress.status} statusKey={progress.statusKey} />
    </div>

    <div class="more">
      <Menu
        items={projectMenuItems(project)}
        onselect={(id) => runProjectAction(id, project)}
        label={menuLabel}
        align="end"
      >
        {#snippet trigger({ toggle, triggerProps })}
          <IconButton
            icon="more-horizontal"
            label={menuLabel}
            tabindex={inner}
            onclick={toggle}
            {...triggerProps}
          />
        {/snippet}
      </Menu>
    </div>

    {#if interrupted}
      <div class="resume">
        <Button
          variant="raised"
          block
          tabindex={inner}
          title={resumeTitle}
          onclick={() => onresume(project)}
        >
          {t('home.action.continueClean')}
        </Button>
      </div>
    {/if}
  </div>
</li>

<style>
  .card { position: relative; display: flex; flex-direction: column; min-width: 0 }

  .open {
    display: flex;
    flex-direction: column;
    min-width: 0;
    padding: 0;
    border: none;
    border-radius: var(--r-xs);
    background: transparent;
    text-align: start;
    cursor: pointer;
  }

  .cover {
    position: relative;
    display: block;
    aspect-ratio: 2 / 3;
    overflow: hidden;
    border-radius: var(--r-xs);
    background: var(--card);
    box-shadow: var(--edge);
    transition: transform var(--dur) var(--ease);
  }

  .body { display: block; min-width: 0; padding-top: 11px }

  .name,
  .line,
  .facts {
    display: block;
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
  }

  .name { font-size: 13px; font-weight: 500; letter-spacing: -.01em; color: var(--text) }
  .line { margin-top: 4px; font-size: 11px; color: var(--t3) }
  .facts { margin-top: 5px; font-size: 10.5px; color: var(--t3) }
  .facts span + span::before { content: '·'; margin: 0 5px }

  .overlay {
    position: absolute;
    top: 0;
    left: 0;
    width: 100%;
    aspect-ratio: 2 / 3;
    pointer-events: none;
    transition: transform var(--dur) var(--ease);
  }
  .overlay > * { pointer-events: auto }

  /* The cover and the overlay are the same box, and lift together. */
  .card:hover .cover,
  .card:hover .overlay,
  .open:focus-visible .cover,
  .open:focus-visible ~ .overlay { transform: translateY(-2px) }

  .chip { position: absolute; top: 8px; left: 8px; pointer-events: none }
  .resume { position: absolute; right: 8px; bottom: 8px; left: 8px }

  /* The global ring is --accent at offset 2px, which puts it on the cover art.
     The art tokens stay light in both themes, and in dark --accent is a near
     white - a near-white ring on near-white art. Both overlay controls carry a
     --surface fill of their own, so the ring is drawn inside their own box
     instead, where it contrasts in both themes. */
  .overlay :global(:focus-visible) { outline-offset: -2px }

  /* A transparent icon button would be invisible over the light cover art in
     the dark theme, so it carries the same --surface backing as the chip. */
  .more {
    position: absolute;
    top: 6px;
    right: 6px;
    border-radius: var(--r-lg);
    background: var(--surface);
    box-shadow: var(--edge-soft);
    opacity: 0;
    transition: opacity var(--dur-fast) var(--ease);
  }
  .card:hover .more,
  .card:focus-within .more { opacity: 1 }
</style>
