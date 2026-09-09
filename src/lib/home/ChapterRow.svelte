<script>
  import { t } from '../i18n/index.js'
  import { IconButton, Menu } from '../ui/index.js'
  import { chapterMenuItems, runChapterAction } from './actions.js'
  import StatusMark from './StatusMark.svelte'

  /**
   * One chapter. Everything a scanlator needs before deciding to open it:
   * where it stands, how big it is, what is waiting in it, and when they last
   * touched it. The row is one button - the whole thing opens the chapter.
   *
   * The menu sits **beside** that button rather than inside it, the way
   * `ProjectCard`'s does, because a button inside a button is not a control the
   * keyboard or a screen reader can reach. It is tabbable only while this row
   * is the list's active item, so a chapter list costs one Tab to leave.
   *
   * @type {{
   *   chapter: import('../api/backend.js').ApiChapter,
   *   progress: import('../model/progress.js').Progress,
   *   mode: 'single'|'longstrip',
   *   interruptedAt: number|null,
   *   active: boolean,
   *   project: import('../api/backend.js').ApiProject,
   *   onopen: (chapter: import('../api/backend.js').ApiChapter) => void,
   * }}
   */
  let { chapter, progress, mode, interruptedAt, active, project, onopen } = $props()

  const menuLabel = $derived(t('home.action.chapterMenu', { name: chapter.name }))

  const skipped = $derived(chapter.pages.filter((page) => page.status === 'skipped').length)

  /** The sub-line, as independent facts - never one concatenated sentence. */
  const facts = $derived.by(() => {
    const list = [
      mode === 'longstrip'
        ? t('home.chapter.positions', { count: progress.totalPages })
        : t('home.chapter.pages', { count: progress.totalPages }),
    ]
    if (progress.regionsNeedingReview > 0) {
      list.push(t('home.chapter.needReview', { count: progress.regionsNeedingReview }))
    }
    if (skipped > 0) list.push(t('home.chapter.skipped', { count: skipped }))
    if (interruptedAt !== null) {
      list.push(t('home.chapter.interrupted', { page: interruptedAt + 1 }))
    }
    return list
  })
</script>

<li class="row">
  <button
    type="button"
    class="hit"
    data-roving-item
    tabindex={active ? 0 : -1}
    onclick={() => onopen(chapter)}
  >
    <StatusMark status={progress.status} statusKey={progress.statusKey} variant="inline" />

    <span class="number">{t('home.chapter.number', { number: chapter.number })}</span>

    <span class="main">
      <span class="title" title={chapter.name}>{chapter.name}</span>
      <span class="facts">
        {#each facts as fact (fact)}<span>{fact}</span>{/each}
      </span>
    </span>

    <!-- The word is already in the row once, in the mark's visually-hidden
         span, which is the copy that survives below 760px where this column is
         dropped. This one is the sighted reader's, and repeats it. -->
    <span class="status" aria-hidden="true">{t(progress.statusKey)}</span>
    <span class="cleaned">
      {t('home.chapter.cleaned', {
        cleaned: progress.pagesCleaned,
        total: progress.totalPages,
      })}
    </span>
    <span class="when">{t(chapter.lastOpened.key, chapter.lastOpened.params)}</span>
  </button>

  <div class="more">
    <Menu
      items={chapterMenuItems()}
      onselect={(id) => runChapterAction(id, project, chapter)}
      label={menuLabel}
      align="end"
    >
      {#snippet trigger({ toggle, triggerProps })}
        <IconButton
          icon="more-horizontal"
          label={menuLabel}
          size={26}
          tabindex={active ? 0 : -1}
          onclick={toggle}
          {...triggerProps}
        />
      {/snippet}
    </Menu>
  </div>
</li>

<style>
  /* The row is a button and the menu is beside it, so the `li` is the thing
     that lays them out - and the menu only appears on hover or focus, which is
     what keeps a destructive control out of a list the user is reading. */
  .row {
    display: flex;
    align-items: center;
    gap: var(--s-2);
    border-bottom: 1px solid var(--line);
  }

  .more { flex: none; opacity: 0; transition: opacity var(--dur-fast) var(--ease) }
  .row:hover .more,
  .more:focus-within { opacity: 1 }

  .hit {
    display: flex;
    align-items: center;
    gap: var(--s-6);
    width: 100%;
    padding: 18px 6px;
    border: none;
    border-radius: var(--r-sm);
    background: transparent;
    text-align: start;
    cursor: pointer;
    transition: background var(--dur-fast) var(--ease);
  }
  .hit:hover { background: var(--accent-soft) }

  .number {
    width: 72px;
    flex: none;
    overflow: hidden;
    font-size: 13px;
    white-space: nowrap;
    text-overflow: ellipsis;
    color: var(--text);
  }

  .main { flex: 1; min-width: 0 }
  .title {
    display: block;
    overflow: hidden;
    font-size: 13px;
    white-space: nowrap;
    text-overflow: ellipsis;
    color: var(--text);
  }
  .facts {
    display: block;
    margin-top: 4px;
    overflow: hidden;
    font-size: 11px;
    white-space: nowrap;
    text-overflow: ellipsis;
    color: var(--t3);
  }
  .facts span + span::before { content: '·'; margin: 0 5px }

  .status,
  .cleaned,
  .when {
    flex: none;
    font-size: 11.5px;
    text-align: end;
    white-space: nowrap;
  }
  .status { width: 120px; color: var(--t2) }
  .cleaned { width: 130px; color: var(--t3) }
  .when { width: 104px; color: var(--t3) }

  /* Narrow windows drop the columns that repeat information the row already
     carries - the mark keeps the status, the sub-line keeps the size. */
  @media (max-width: 900px) { .when { display: none } }
  @media (max-width: 760px) { .status { display: none } }
</style>
