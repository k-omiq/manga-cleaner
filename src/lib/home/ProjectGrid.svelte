<script>
  import ProjectCard from './ProjectCard.svelte'
  import { columnCount, focusItem, nextIndex } from './roving.js'
  import { t } from '../i18n/index.js'

  /**
   * The library grid: `auto-fill, minmax(168px, 1fr)`, 32/20 gutters, and one
   * tab stop. Arrows move by one card or one row - the row step is read from
   * the resolved `grid-template-columns`, so it follows the real layout at any
   * window width - and Enter opens the focused project.
   *
   * @type {{
   *   rows: Array<{
   *     project: import('../api/backend.js').ApiProject,
   *     progress: import('../model/progress.js').Progress,
   *   }>,
   *   onresume: (project: import('../api/backend.js').ApiProject) => void,
   * }}
   */
  let { rows, onresume } = $props()

  /** @type {HTMLElement | undefined} */
  let grid = $state()
  let active = $state(0)

  // A removed or filtered-out project must not leave the tab stop off the end.
  $effect(() => {
    if (active > rows.length - 1) active = Math.max(0, rows.length - 1)
  })

  /** @param {KeyboardEvent} event */
  function onkeydown(event) {
    if (event.metaKey || event.ctrlKey || event.altKey) return
    const next = nextIndex(event.key, {
      index: active,
      count: rows.length,
      columns: columnCount(grid),
      orientation: 'grid',
    })
    if (next === null) return
    event.preventDefault()
    active = next
    focusItem(grid, next)
  }

  /** Keep the roving stop where the user actually is, however they got there. */
  function onfocusin(/** @type {FocusEvent} */ event) {
    const item = /** @type {HTMLElement} */ (event.target)?.closest?.('[data-roving-item]')
    if (!item) return
    const items = [...(grid?.querySelectorAll('[data-roving-item]') ?? [])]
    const index = items.indexOf(item)
    if (index >= 0) active = index
  }
</script>

<!-- The list is never a tab stop and never focused: the handlers are here
     because arrow keys have to be answered wherever inside a card they are
     pressed, and the thing that receives them is always one of the buttons. -->
<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
<ul
  bind:this={grid}
  class="grid"
  aria-label={t('home.section.projects')}
  onkeydown={onkeydown}
  onfocusin={onfocusin}
>
  {#each rows as row, i (row.project.id)}
    <ProjectCard
      project={row.project}
      progress={row.progress}
      active={i === active}
      {onresume}
    />
  {/each}
</ul>

<style>
  .grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(168px, 1fr));
    gap: 32px 20px;
    margin: 0;
    padding: 0;
    list-style: none;
  }
</style>
