<script>
  import Icon from '../icons/Icon.svelte'
  import { anchoredOverlay } from './overlay.svelte.js'

  /**
   * Anchored dropdown. The component owns the open state and the anchor box;
   * the caller supplies the trigger through the `trigger` snippet, which
   * receives `{ open, toggle, close, triggerProps }` - spread `triggerProps`
   * onto whatever button you render so it carries `aria-haspopup` and
   * `aria-expanded`.
   *
   * Items are `{ id, label, icon?, shortcut?, disabled?, separator?,
   * selected? }`. A `separator: true` entry draws a hairline and is not
   * focusable. If any item carries `selected` the menu is a choice list: every
   * item becomes a `menuitemradio` with `aria-checked`, and a 14px mark column
   * is reserved on all of them so the labels stay aligned.
   *
   * `note` is a muted line under the items, and it exists for one thing: why
   * an item in this menu is disabled. A `title` on a disabled button reaches
   * nobody - browsers fire no hover events on one - so a menu that withholds
   * an option owes the reason as text inside itself. It is absent unless
   * something is actually blocked. It sits **beside** the `role="menu"` list
   * rather than inside it: a `menu` may hold menu items and separators and
   * nothing else, and a paragraph in there is both invalid and unreachable by
   * the arrow keys. Every disabled item is `aria-describedby` it instead, so
   * the reason arrives with the item it is about.
   *
   * @type {{
   *   items: Array<{
   *     id: string, label?: string, icon?: string, shortcut?: string,
   *     disabled?: boolean, separator?: boolean, selected?: boolean,
   *   }>,
   *   onselect: (id: string) => void,
   *   label: string,
   *   note?: string | null,
   *   align?: 'start' | 'end',
   *   trigger: import('svelte').Snippet<[{
   *     open: boolean,
   *     toggle: () => void,
   *     close: () => void,
   *     triggerProps: Record<string, unknown>,
   *   }]>,
   * }}
   */
  let { items, onselect, label, note = null, align = 'start', trigger } = $props()

  /** @type {HTMLElement | undefined} */
  let root = $state()
  /** @type {HTMLElement | undefined} */
  let menuEl = $state()
  /** @type {HTMLButtonElement[]} */
  let itemEls = $state([])
  let pending = $state(-1)
  let flippedY = $state(false)
  let shiftedX = $state(0)

  // The list's own id, so the trigger can point at it while it is open, and
  // the note's, so the items it explains can point at it.
  const uid = $props.id()
  const listId = `${uid}-list`
  const noteId = `${uid}-note`

  // Opening, closing, the outside press and the focus that comes back are
  // `ui/overlay.svelte.js`'s, shared with `Popover`. What stays here is the
  // part that is a *menu*: which item the keyboard is on.
  const overlay = anchoredOverlay(() => root)
  const open = $derived(overlay.open)

  const entries = $derived(items.map((it, i) => ({ ...it, index: i })))
  const isChoice = $derived(items.some((it) => it.selected !== undefined))
  // A mixed menu - the project card's is Open, New chapter, Rename, Copy
  // source path, Remove - put an icon in front of three labels and nothing in
  // front of the other two, so the labels started at two different x. One item
  // with an icon reserves the column for all of them, exactly as `selected`
  // reserves the mark column.
  const hasIcons = $derived(items.some((it) => !!it.icon))
  const enabled = $derived(
    entries.filter((it) => !it.separator && !it.disabled).map((it) => it.index),
  )

  function show() {
    if (open) return
    overlay.show()
    pending = enabled[0] ?? -1
  }

  /** @param {{refocus?: boolean}} [options] */
  function close(options) {
    if (!open) return
    pending = -1
    overlay.close(options)
  }

  function toggle() {
    open ? close() : show()
  }

  // Avoid holes in itemEls when items shrink or when the menu is closed.
  $effect(() => {
    if (!open) {
      itemEls = []
    } else if (itemEls.length > items.length) {
      itemEls = itemEls.slice(0, items.length)
    }
  })

  // Focus the item the keyboard has landed on, after it exists in the DOM.
  // Re-synchronize pending when opening to eliminate racing with overlay.show().
  $effect(() => {
    if (!open) {
      pending = -1
      return
    }
    if (pending < 0 && enabled.length) {
      pending = enabled[0]
    }
    if (pending >= 0) {
      itemEls[pending]?.focus()
    }
  })

  /**
   * Where the panel goes, measured once per opening.
   *
   * **Separate from the focus effect above, and it must stay separate.** That
   * one reads `pending`, which every arrow key changes, so anything sharing it
   * re-runs on every keystroke - and a re-measurement is not idempotent here:
   * the flip is what moves the panel, so a flipped panel measures as one that
   * has room below and un-flips itself, and the next key flips it back. The
   * placement question is only ever asked of an opening panel in its natural
   * position, which is what this effect's dependencies say.
   */
  $effect(() => {
    if (!open) {
      flippedY = false
      shiftedX = 0
      return
    }
    if (!menuEl) return
    const box = menuEl.getBoundingClientRect()
    const vh = globalThis.innerHeight ?? 0
    const vw = globalThis.innerWidth ?? 0
    flippedY = Boolean(vh && box.bottom > vh - 8 && box.top - box.height > 0)
    shiftedX = vw && box.right > vw - 8 ? Math.round(box.right - (vw - 8)) : 0
  })

  const firstItem = $derived(enabled[0] ?? -1)
  const lastItem = $derived(enabled[enabled.length - 1] ?? -1)

  /**
   * @param {number} from index of the current item
   * @param {number} delta -1 or 1
   */
  function stepTo(from, delta) {
    if (!enabled.length) return
    const at = enabled.indexOf(from)
    const next = at === -1 ? (delta > 0 ? 0 : enabled.length - 1) : (at + delta + enabled.length) % enabled.length
    pending = enabled[next]
  }

  /** @param {KeyboardEvent} e */
  function onkeydown(e) {
    // `Escape` closes every anchored overlay, and `Tab` closes this one -
    // a menu is a single control, so tabbing is leaving it. (A popover holds
    // several and reads Tab differently; see `ui/overlay.svelte.js`.) The
    // pending item is cleared here because only a menu has one.
    if (e.key === 'Escape') {
      if (open) pending = -1
      overlay.dismissKey(e)
      return
    }
    if (e.key === 'Tab') {
      if (open) pending = -1
      overlay.tabAway(e)
      return
    }
    switch (e.key) {
      case 'ArrowDown':
        e.preventDefault()
        if (open) stepTo(pending, 1)
        else show()
        return
      case 'ArrowUp':
        e.preventDefault()
        if (open) stepTo(pending, -1)
        else { show(); pending = lastItem }
        return
      case 'Home':
        if (!open) return
        e.preventDefault()
        pending = firstItem
        return
      case 'End':
        if (!open) return
        e.preventDefault()
        pending = lastItem
        return
      default:
    }
  }

  /** @param {string} id */
  function pick(id) {
    close()
    onselect(id)
  }
</script>

{#snippet menuItems()}
  {#each entries as it (it.id)}
    {#if it.separator}
      <div class="sep" role="separator"></div>
    {:else}
      <button
        bind:this={itemEls[it.index]}
        type="button"
        role={isChoice ? 'menuitemradio' : 'menuitem'}
        aria-checked={isChoice ? !!it.selected : undefined}
        aria-describedby={it.disabled && note ? noteId : undefined}
        class="item"
        tabindex="-1"
        disabled={it.disabled}
        onclick={() => pick(it.id)}
      >
        {#if isChoice}
          <span class="mark">
            {#if it.selected}<Icon name="check" size={13} />{/if}
          </span>
        {/if}
        {#if hasIcons}
          <span class="gutter">
            {#if it.icon}<Icon name={it.icon} size={14} />{/if}
          </span>
        {/if}
        <span class="item-label">{it.label}</span>
        {#if it.shortcut}<span class="item-key">{it.shortcut}</span>{/if}
      </button>
    {/if}
  {/each}
{/snippet}

<!-- Keyboard handling sits on the anchor so it covers both the trigger and the
     open menu. The div itself is never a tab stop. -->
<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="anchor" bind:this={root} onkeydown={onkeydown}>
  {@render trigger({
    open,
    toggle,
    close,
    triggerProps: {
      'aria-haspopup': 'menu',
      'aria-expanded': open,
      'aria-controls': open ? listId : undefined,
    },
  })}

  {#if open}
    {#if note}
      <div
        bind:this={menuEl}
        class="menu"
        class:end={align === 'end'}
        class:flipped-y={flippedY}
        style:transform={shiftedX ? `translateX(-${shiftedX}px)` : undefined}
      >
        <!-- The list is described by the note as well as each disabled item
             being: the reason a rung is *absent* - a sidecar this machine will
             not carry - disables nothing, so the per-item association alone
             would leave that sentence reaching nobody again. -->
        <div
          class="list"
          id={listId}
          role="menu"
          aria-label={label}
          aria-describedby={noteId}
        >
          {@render menuItems()}
        </div>
        <p class="note" id={noteId}>{note}</p>
      </div>
    {:else}
      <div
        bind:this={menuEl}
        class="menu"
        class:end={align === 'end'}
        class:flipped-y={flippedY}
        style:transform={shiftedX ? `translateX(-${shiftedX}px)` : undefined}
        id={listId}
        role="menu"
        aria-label={label}
      >
        {@render menuItems()}
      </div>
    {/if}
  {/if}
</div>

<style>
  .anchor { position: relative; display: inline-flex }

  .menu {
    position: absolute;
    top: calc(100% + var(--s-2));
    left: 0;
    z-index: 50;
    min-width: 184px;
    /* Project names are user data and can be arbitrarily long; without a cap
       the panel grows past the window. The label ellipsises instead. */
    max-width: 288px;
    padding: var(--s-1);
    border-radius: var(--r-lg);
    background: var(--surface);
    box-shadow: var(--edge);
    animation: mcIn var(--dur) var(--ease);
    cursor: default;
  }
  .menu.end { left: auto; right: 0 }
  .menu.flipped-y {
    top: auto;
    bottom: calc(100% + var(--s-2));
  }

  .item {
    display: flex;
    align-items: center;
    gap: var(--s-3);
    width: 100%;
    height: 26px;
    padding: 0 var(--s-3);
    border: none;
    border-radius: var(--r-sm);
    background: transparent;
    color: var(--t2);
    font-size: 12px;
    text-align: start;
    white-space: nowrap;
    cursor: pointer;
    transition:
      background var(--dur-fast) var(--ease),
      color var(--dur-fast) var(--ease);
  }
  .item:hover:not(:disabled),
  .item:focus-visible { background: var(--accent-soft); color: var(--text) }
  .item:disabled { opacity: .4; cursor: default }

  .item-label { flex: 1; overflow: hidden; text-overflow: ellipsis }
  .mark,
  .gutter {
    display: flex;
    flex: none;
    align-items: center;
    justify-content: center;
    width: 14px;
  }
  /* The check reads as state and takes the item's full ink; an item's own icon
     is decoration beside its label and keeps the label's colour. */
  .mark { color: var(--text) }
  .item-key { color: var(--t3); font-size: 10.5px }

  .sep {
    height: 1px;
    margin: var(--s-1) var(--s-2);
    background: var(--line);
  }

  /* Under the items, over a hairline, and outside the `role="menu"` element
     as well as outside the list visually: a sentence *about* the list rather
     than one more thing in it. `--t2` rather than `--t3` - this is the only channel
     for a reason the user needs, and `--t3` is under 4.5:1 against
     `--surface` in dark. */
  .note {
    margin: var(--s-1) 0 0;
    padding: var(--s-2) var(--s-3) var(--s-1);
    border-top: 1px solid var(--line);
    font-size: 10px;
    line-height: 1.45;
    color: var(--t2);
    white-space: normal;
  }
</style>
