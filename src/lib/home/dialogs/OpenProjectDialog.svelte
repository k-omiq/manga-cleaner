<script>
  import { Button, Modal, TextInput } from '../../ui/index.js'
  import Icon from '../../icons/Icon.svelte'
  import { t } from '../../i18n/index.js'
  import { closeModal, modalWidth, openProject } from '../../state/app.svelte.js'
  import { library } from '../library.svelte.js'
  import { focusItem, nextIndex } from '../roving.js'
  import { projectProgress } from '../../model/progress.js'

  /**
   * Open a project by typing its name.
   *
   * The design file opens a `.mcproj` file from disk; the adapter exposes no
   * file dialog and no "open this path" method (see the task report), so what
   * this does instead is the half that is actually reachable - and the half a
   * keyboard user wants anyway: type three letters, press Enter, and be in the
   * project without touching the grid.
   *
   * This dialog is the only place Home filters a project list. The library
   * itself has no search field - the design file has none, and a grid that can
   * look empty because of a filter is a false state worth avoiding. Here the
   * filter is bounded by the dialog and cannot outlive it.
   *
   * @type {{ spec: import('../../state/app.svelte.js').ModalSpec }}
   */
  let { spec } = $props()

  let query = $state('')
  let active = $state(0)
  /** @type {HTMLElement | undefined} */
  let list = $state()

  /**
   * @param {import('../../api/backend.js').ApiProject} project
   * @param {string} needle - already lower-cased and trimmed
   * @returns {boolean}
   */
  function matches(project, needle) {
    return needle === '' || project.name.toLowerCase().includes(needle)
  }

  const results = $derived.by(() => {
    const needle = query.trim().toLowerCase()
    return library.projects
      .filter((project) => matches(project, needle))
      .map((project) => ({ project, progress: projectProgress(project) }))
  })

  $effect(() => {
    if (active > results.length - 1) active = Math.max(0, results.length - 1)
  })

  /** @param {string} id */
  function open(id) {
    openProject(id)
    closeModal('open')
  }

  /** Enter from the filter opens the highlighted result. */
  function openActive() {
    const row = results[active]
    if (row) open(row.project.id)
  }

  /** @param {KeyboardEvent} event */
  function onfilterkeydown(event) {
    if (event.key === 'Enter') {
      event.preventDefault()
      openActive()
      return
    }
    const next = nextIndex(event.key, {
      index: active,
      count: results.length,
      orientation: 'list',
    })
    if (next === null) return
    event.preventDefault()
    active = next
    focusItem(list, next)
  }

  /** @param {KeyboardEvent} event */
  function onlistkeydown(event) {
    const next = nextIndex(event.key, {
      index: active,
      count: results.length,
      orientation: 'list',
    })
    if (next === null) return
    event.preventDefault()
    active = next
    focusItem(list, next)
  }
</script>

<Modal
  title={t(spec.titleKey)}
  width={modalWidth(spec.kind)}
  onclose={() => closeModal(null)}
>

  <div class="filter">
    <TextInput
      value={query}
      onchange={(value) => {
        query = value
        active = 0
      }}
      icon="search"
      label={t('home.openProject.filter')}
      placeholder={t('home.openProject.filterPlaceholder')}
      onkeydown={onfilterkeydown}
    />
  </div>

  {#if results.length === 0}
    <p class="none">{t('home.openProject.noMatches')}</p>
  {:else}
    <!-- Same shape as the library grid: the buttons take the keys, the list
         only routes them. It is never focused itself. -->
    <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
    <ul bind:this={list} class="list" onkeydown={onlistkeydown}>
      {#each results as row, i (row.project.id)}
        <li>
          <button
            type="button"
            class="row"
            class:active={i === active}
            data-roving-item
            aria-current={i === active ? 'true' : undefined}
            tabindex={i === active ? 0 : -1}
            onclick={() => open(row.project.id)}
            onfocus={() => (active = i)}
          >
            <!-- The active row is the one Open acts on, and `--accent-soft` is
                 also the hover fill, so the fill alone says neither which row
                 that is nor anything at all to a screen reader. The gutter is
                 reserved on every row so the names stay in one column. -->
            <span class="mark" aria-hidden="true">
              {#if i === active}<Icon name="chevron-right" size={12} />{/if}
            </span>
            <span class="name">{row.project.name}</span>
            <span class="meta">
              {t('home.openProject.meta', {
                chapters: row.project.chapters.length,
                pages: row.progress.totalPages,
              })}
            </span>
          </button>
        </li>
      {/each}
    </ul>
  {/if}

  {#snippet buttons()}
    <Button onclick={() => closeModal(null)}>{t('shell.action.cancel')}</Button>
    <Button variant="primary" disabled={results.length === 0} onclick={openActive}>
      {t('home.action.open')}
    </Button>
  {/snippet}
</Modal>

<style>
  .filter { margin-top: var(--s-5) }

  .list {
    max-height: 216px;
    overflow-y: auto;
    scrollbar-gutter: stable;
    margin: var(--s-4) 0 0;
    padding: 0;
    list-style: none;
  }

  .row {
    display: flex;
    align-items: baseline;
    gap: var(--s-4);
    width: 100%;
    height: 30px;
    padding: 0 var(--s-3);
    border: none;
    border-radius: var(--r-sm);
    background: transparent;
    text-align: start;
    cursor: pointer;
    transition: background var(--dur-fast) var(--ease);
  }
  .row:hover, .row.active { background: var(--accent-soft) }
  .mark {
    display: flex;
    flex: none;
    align-items: center;
    justify-content: center;
    align-self: center;
    width: 14px;
    color: var(--t2);
  }

  .name {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    font-size: 12px;
    white-space: nowrap;
    text-overflow: ellipsis;
    color: var(--text);
  }
  .meta { flex: none; font-size: 10.5px; color: var(--t3); white-space: nowrap }

  .none { margin: var(--s-5) 0 0; font-size: 11.5px; color: var(--t3) }
</style>
