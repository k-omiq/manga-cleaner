<script>
  import { isHighlighted, hover, select, undo, undoAvailable, undoLabelKey } from '../state/editor.svelte.js'
  import { rowEngines, engineChoiceLabel } from '../model/masks.js'
  import { capabilities } from '../state/capabilities.svelte.js'
  import { cloudUsable } from '../state/cloud.svelte.js'
  import { actionHint } from './maskrows.js'
  import { menuPoint } from './gesture.js'
  import { deleteRow, keepDependencyResult, rerunMask, runMaskAction } from './maskactions.svelte.js'
  import { Button, Disclosure, IconButton, Select } from '../ui/index.js'
  import { t } from '../i18n/index.js'
  import MaskFacts from './MaskFacts.svelte'
  import RegionMenu from './RegionMenu.svelte'

  /**
   * One region in the Layers panel. A mask from the automatic pass and a mask
   * drawn by hand are the same row with the same controls and the same
   * provenance; a region with no mask - declined, or
   * skipped by the script gate - is the same row with what happened to it in
   * place of the engine.
   *
   * Collapsed: the status glyph, the engine, the sub-line, and the row's three
   * controls - the engine picker, Try again, and Delete. Expanded: the
   * provenance facts, then whatever actions the row still carries (a
   * gate-skipped region's Clean anyway; nothing, for a mask).
   *
   * **Three controls, not seven.** The row used to offer Stronger, Simpler,
   * Fill mode and Reopen: four buttons that asked the reader to hold an engine
   * ladder in their head to press any of them. The picker says the same thing
   * - *what should this layer be cleaned with* - as a list of names, and Try
   * again covers the case where the answer is "the same thing, again".
   *
   * **Try again replaces; a new stroke refines.** Both Try again and the
   * picker redo this layer from the layers below it and swap its result; a new
   * stroke reads the page as shown. The expanded row says so in one line, and
   * a paint or clone row keeps Try again visible but off, with the reason.
   *
   * **Delete is on every row, warnings included.** For a mask it deletes the
   * mask and the original text comes back; for a region with no mask it is the
   * region itself that goes, which is how a warning the user has read and
   * decided about leaves the list. Both are undoable and both go through the
   * backend, so the panel and the file cannot disagree.
   *
   * Hovering or focusing the row highlights the region on the canvas, and
   * activating it selects: the row writes `hover()` and `select()`, the canvas
   * reads `editor.hoverId` / `editor.selectionId`, and neither knows the other
   * exists (see the highlight contract in `state/editor.svelte.js`).
   *
   * @type {{
   *   row: import('./maskrows.js').MaskRow,
   *   region: import('../api/backend.js').ApiRegion,
   *   open: boolean,
   *   ontoggle: (open: boolean) => void,
   * }}
   */
  let { row, region, open, ontoggle } = $props()

  const pickerId = $props.id()

  const highlighted = $derived(isHighlighted(row.id))

  // Try again replaces this layer from the layers below it; a paint or clone
  // stroke has no cleaning to redo, so its row keeps the control in place,
  // dimmed, and says why rather than dropping it without a word. Other masks
  // `reRunnable` refuses (an approved text-shape component) keep Delete alone.
  const retryBlocked = $derived(!row.reRunnable && (row.engine === 'paint' || row.engine === 'clone'))

  // Undo here is the editor's global undo, not an undo of the change that put
  // this layer in review, so its tooltip names what it would reverse.
  const undoKey = $derived(undoLabelKey())
  const undoHint = $derived(undoKey ? t('editor.action.undoCommand', { commandKey: undoKey }) : undefined)
  const title = $derived(t(row.titleKey))
  const sub = $derived(row.sub.map((part) => t(part.key, part.params)).join(' · '))

  // What this machine can clean with now, and Cloud last while a cloud
  // endpoint is ready and allowed: left out otherwise, like a rung whose
  // weights are missing. The engine the mask actually used leads the list
  // when it is not among them - Cloud while the cloud is not ready, a rung
  // this machine no longer has - because a picker that silently showed the
  // wrong entry as selected would be worse than one that is not shown at
  // all, which is what `row.reRunnable` decides; and a mask cleaned with FLUX
  // on another machine still has to read as FLUX here.
  const offered = $derived(rowEngines(capabilities.engines, { cloud: cloudUsable() }))
  const engineOptions = $derived(
    (row.engine && !offered.includes(row.engine) ? [row.engine, ...offered] : offered).map(
      (rung) => ({ value: rung, label: t(engineChoiceLabel(rung)) }),
    ),
  )

  // The entry last picked, while its re-run is under way. The picker shows it
  // until that re-run has answered, then the engine the mask has: the new
  // one, or the old one again when nothing changed - a cloud consent
  // cancelled, a re-run refused. Without this a native select goes on
  // showing the pick, since the value it is given never moved.
  /** @type {string|null} */
  let picking = $state(null)
  let pickSeq = 0

  // Pointer and focus are two ways into the same highlight, and either can
  // outlast the other: tabbing into an expanded row's buttons with the pointer
  // still over it used to clear the highlight, and so did moving the pointer
  // away from a row whose action button was focused. The highlight goes only
  // when neither is left.
  let pointerIn = false
  let focusIn = false

  /**
   * @param {'pointer'|'focus'} source
   * @param {boolean} inside
   */
  function track(source, inside) {
    if (source === 'pointer') pointerIn = inside
    else focusIn = inside
    hover(pointerIn || focusIn ? row.id : null)
  }

  /** @param {string} next */
  async function onEngineChange(next) {
    if (!next || next === (picking ?? row.engine)) return
    const mine = ++pickSeq
    picking = next
    try {
      await rerunMask(region, 'engine', next)
    } finally {
      // A later pick owns the picker until its own answer.
      if (mine === pickSeq) picking = null
    }
  }

  /**
   * The open context menu: `{x, y, region}` in client pixels, or null.
   *
   * @type {{x: number, y: number, region: import('../api/backend.js').ApiRegion} | null}
   */
  let menu = $state(null)

  /**
   * The row's own three controls, again, on the secondary press - the same
   * menu the canvas raises over the same region, so a user who found the
   * gesture on the page finds it on the row and the other way round. The
   * engine picker is behind a disclosure; this is the route to it that does
   * not ask for the row to be expanded first.
   *
   * `Shift`+`F10` and the context-menu key on the row's summary button raise
   * this event too, with no coordinates; `menuPoint` anchors those to the row.
   *
   * @param {MouseEvent} event
   */
  function oncontextmenu(event) {
    if (!region) return
    event.preventDefault()
    select(row.id)
    const rowEl = /** @type {HTMLElement} */ (event.currentTarget)
    menu = { ...menuPoint(event, rowEl.getBoundingClientRect()), region }
  }
</script>

<!-- Pointer-down rather than click: the row is a container, and the selection
     has to follow a press anywhere in it - the summary, the picker, either
     icon - not only the one button that happens to carry the toggle. -->
<div
  class="row"
  class:highlighted
  role="listitem"
  data-mask-row={row.id}
  onpointerdown={() => select(row.id)}
  onpointerenter={() => track('pointer', true)}
  onpointerleave={() => track('pointer', false)}
  onfocusin={() => track('focus', true)}
  onfocusout={() => track('focus', false)}
  {oncontextmenu}
>
  <Disclosure {open} {ontoggle} variant="plain">
    {#snippet summary()}
      <span class="line">
        <span class="dot {row.status}" aria-hidden="true">{row.glyph}</span>
        <span class="text">
          <span class="title">{title}</span>
          <span class="sub">{sub}</span>
        </span>
        <span class="sr">{t(row.statusKey)}</span>
      </span>
    {/snippet}

    <!-- Two icons on the collapsed line and no more. The panel is a narrow
         column, and a third control here took its width out of the layer's own
         name - which is the one thing on the line the reader is scanning for.
         The engine picker is a decision, not a reflex, so it goes below with
         the facts that inform it. -->
    {#snippet trailing()}
      {#if row.reRunnable}
        <IconButton
          icon="refresh"
          label={t('masks.action.retry')}
          title={t('masks.action.retryHint')}
          size={21}
          iconSize={13}
          onclick={() => rerunMask(region, 'retry')}
        />
      {:else if retryBlocked}
        <!-- aria-disabled rather than disabled: a disabled button leaves the
             tab order and WebKit shows no tooltip over it, and then nothing
             says why Try again is off. -->
        <IconButton
          icon="refresh"
          label={t('masks.action.retry')}
          title={t('masks.action.retryBlocked')}
          aria-disabled="true"
          data-retry-blocked
          size={21}
          iconSize={13}
        />
      {/if}
      <IconButton
        icon="trash"
        label={t(row.engine ? 'masks.action.delete' : 'masks.action.deleteRegion')}
        title={t(row.engine ? 'masks.action.deleteHint' : 'masks.action.deleteRegionHint')}
        size={21}
        iconSize={13}
        onclick={() => deleteRow(region)}
      />
    {/snippet}

    <MaskFacts facts={row.facts} />

    {#if region.mask?.dependencyReview}
      <!-- The facts above end with why ("Flagged"); these are the three
           answers to it. Rebuild is Try again, so it says what Try again says. -->
      <div class="acts" role="group" aria-label={t('masks.status.needsReview')} data-dependency-review>
        <Button size="sm" title={t('masks.dependency.keepHint')} onclick={() => keepDependencyResult(region)}>
          {t('masks.dependency.keep')}
        </Button>
        {#if row.reRunnable}
          <Button size="sm" title={t('masks.action.retryHint')} onclick={() => rerunMask(region, 'retry')}>
            {t('masks.dependency.rebuild')}
          </Button>
        {/if}
        {#if undoAvailable()}
          <Button size="sm" title={undoHint} onclick={() => undo()}>{t('masks.dependency.undoLast')}</Button>
        {/if}
      </div>
    {/if}

    {#if row.reRunnable}
      <!-- The kit's picker at the row's own size, not a `<select>` of this
           file's own: the tool window asks the same question with the same
           control, and two appearances of it was not worth keeping. -->
      <div class="pick">
        <label class="pick-label" for={pickerId}>{t('masks.action.engine')}</label>
        <div class="pick-control">
          <Select
            id={pickerId}
            size="row"
            options={engineOptions}
            value={picking ?? row.engine}
            title={t('masks.action.engineHint')}
            onchange={onEngineChange}
          />
        </div>
      </div>
      <p class="note">{t('masks.action.rerunNote')}</p>
    {:else if retryBlocked}
      <p class="note">{t('masks.action.retryBlocked')}</p>
    {/if}

    {#if row.actions.length > 0}
      <div class="acts" data-acts>
        {#each row.actions as action (action.id)}
          {@const hint = actionHint(action.id)}
          <Button
            size="sm"
            title={t(hint.key, hint.params)}
            onclick={() => runMaskAction(action.id, region)}
          >
            {t(action.labelKey)}
          </Button>
        {/each}
      </div>
    {/if}
  </Disclosure>

  <RegionMenu at={menu} onclose={() => (menu = null)} />
</div>

<style>
  .row {
    border-radius: var(--r-chip);
    padding: 2px var(--s-2);
    margin-bottom: 2px;
    transition: background var(--dur-fast) var(--ease);
  }
  /* Hover *or* selection: one rule, so the canvas lighting a region and the
     pointer lighting a row look the same. */
  .highlighted { background: var(--panel2) }

  .line {
    display: flex;
    align-items: center;
    gap: var(--s-3);
    min-height: 26px;
  }

  .dot {
    flex: none;
    width: 14px;
    text-align: center;
    font-size: 11px;
    color: var(--t3);
  }
  .dot.review { color: var(--t2) }
  .dot.declined { color: var(--warn) }

  .text { flex: 1; min-width: 0 }

  .title,
  .sub {
    display: block;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .title { font-size: 11.5px }
  .sub { font-size: 10.5px; color: var(--t3) }

  /* The picker sits under the facts, in the same 70px key column they use, so
     "Clean with" reads as one more line of the same table - which is what it
     is: the only line of it the reader can change. */
  .pick {
    display: flex;
    align-items: center;
    gap: var(--s-3);
    margin-top: 7px;
    font-size: 10.5px;
  }

  .pick-label {
    flex: none;
    width: 70px;
    color: var(--t3);
  }

  /* The picker draws its own metrics; this is only the cell it fills. */
  .pick-control {
    flex: 1;
    min-width: 0;
  }

  /* What Try again means next to a new stroke, or why it is off. Muted and
     small: it is a reminder under the controls, not another fact. */
  .note {
    margin: 6px 0 0;
    font-size: 10.5px;
    line-height: 1.45;
    color: var(--t3);
  }

  .acts {
    display: flex;
    flex-wrap: wrap;
    gap: 5px;
    margin-top: 9px;
  }

  /* The kit's IconButton dims only a native `disabled`; this one stays
     focusable so its reason can be read. The glyph is dimmed rather than the
     button, so the focus ring keeps its full contrast. */
  .row :global(.ibtn[aria-disabled='true']) { cursor: default }
  .row :global(.ibtn[aria-disabled='true'] > *) { opacity: .35 }
  .row :global(.ibtn[aria-disabled='true']:hover) {
    background: transparent;
    color: var(--t2);
  }

  /* Visually hidden, still read aloud: the glyph's meaning in words. */
  .sr {
    position: absolute;
    width: 1px;
    height: 1px;
    margin: -1px;
    padding: 0;
    overflow: hidden;
    clip-path: inset(50%);
    white-space: nowrap;
  }
</style>
