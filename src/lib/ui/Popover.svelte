<script>
  import { anchoredOverlay } from './overlay.svelte.js'
  import { focusable } from './focus.js'

  /**
   * An anchored panel of ordinary controls - `Menu`'s shape without `Menu`'s
   * list. The tool bar's *Adjustments* is the one that exists: a handful of
   * sliders and a colour field that have no room on a bar an inch tall and no
   * business in a menu, which is a choice between items rather than a place to
   * put controls.
   *
   * The trigger arrives the same way a menu's does - a `trigger` snippet given
   * `{ open, toggle, close, triggerProps }`, and `triggerProps` spread onto
   * whatever button the caller renders - so the two are learned once.
   *
   * It is a `dialog` by role and **not modal**: nothing traps focus, `Tab`
   * walks out of the panel and off the end of it, and the page underneath
   * stays live. Focus moves to the first control inside on open, because a
   * panel opened from the keyboard that left focus on its trigger would need a
   * second key to reach anything.
   *
   * Dismissal - the outside press, `Escape` - is `ui/overlay.svelte.js`'s,
   * shared with `Menu`. What is **not** shared is `Tab`: a menu is one control
   * and Tab leaves it, but this holds several, so Tab moves between them and
   * the panel closes when focus actually leaves the anchor (`focusOut`). Tab
   * off the last control produces exactly that, so the two readings agree at
   * the edge and differ only in the middle.
   *
   * @type {{
   *   label: string,
   *   align?: 'start' | 'end',
   *   width?: string|number,
   *   trigger: import('svelte').Snippet<[{
   *     open: boolean,
   *     toggle: () => void,
   *     close: () => void,
   *     triggerProps: Record<string, unknown>,
   *   }]>,
   *   children: import('svelte').Snippet,
   * }}
   */
  let { label, align = 'start', width = '260px', trigger, children } = $props()

  /** @type {HTMLElement | undefined} */
  let root = $state()
  /** @type {HTMLElement | undefined} */
  let panel = $state()
  let flippedY = $state(false)

  // The panel's own id, so the trigger can point at it while it exists.
  const panelId = $props.id()

  const overlay = anchoredOverlay(() => root)
  const open = $derived(overlay.open)
  const styleWidth = $derived(typeof width === 'number' ? `${width}px` : width)

  // The first control inside, once it exists in the DOM. A panel with nothing
  // focusable in it is left alone rather than forced onto the panel itself:
  // the caller has drawn something that is not a control, and moving focus to
  // a `div` would only strand it.
  $effect(() => {
    if (!open) {
      flippedY = false
      return
    }
    const node = panel
    if (!node) return
    focusable(node)[0]?.focus()

    const box = node.getBoundingClientRect()
    const vh = globalThis.innerHeight ?? 0
    if (vh && box.bottom > vh - 8 && box.top - box.height > 0) {
      flippedY = true
    } else {
      flippedY = false
    }
  })
</script>

<!-- Keyboard and focus handling sit on the anchor so they cover both the
     trigger and the open panel. The div itself is never a tab stop. -->
<!-- svelte-ignore a11y_no_static_element_interactions -->
<div
  class="anchor"
  bind:this={root}
  onkeydown={(e) => overlay.dismissKey(e)}
  onfocusout={(e) => overlay.focusOut(e)}
>
  {@render trigger({
    open,
    toggle: overlay.toggle,
    close: overlay.close,
    triggerProps: {
      'aria-haspopup': 'dialog',
      'aria-expanded': open,
      'aria-controls': open ? panelId : undefined,
    },
  })}

  {#if open}
    <div
      bind:this={panel}
      id={panelId}
      class="panel"
      class:end={align === 'end'}
      class:flipped-y={flippedY}
      role="dialog"
      aria-label={label}
      style:width={styleWidth}
    >
      {@render children()}
    </div>
  {/if}
</div>

<style>
  .anchor { position: relative; display: inline-flex }

  /* `Menu`'s own metrics, because it is the same object seen from the side:
     the same offset below the anchor, the same surface, the same edge and the
     same entrance. */
  .panel {
    position: absolute;
    top: calc(100% + var(--s-2));
    left: 0;
    z-index: 50;
    max-width: 90vw;
    padding: var(--s-3);
    border-radius: var(--r-lg);
    background: var(--surface);
    box-shadow: var(--edge);
    animation: mcIn var(--dur) var(--ease);
    cursor: default;
  }
  .panel.end { left: auto; right: 0 }
  .panel.flipped-y {
    top: auto;
    bottom: calc(100% + var(--s-2));
  }
</style>
