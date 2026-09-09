<script>
  import { session, raiseWindow, foldWindow } from '../state/session.svelte.js'
  import { windowGesture, closeWindowFocusing } from './windowgesture.js'
  import { IconButton } from '../ui/index.js'
  import Icon from '../icons/Icon.svelte'
  import { t } from '../i18n/index.js'

  /**
   * A floating window: draggable, resizable, foldable, closable, and raised by
   * a touch anywhere inside it. Pages and Layers are both this component with
   * a different body. The tool bar is **not** - it is content-sized, has
   * no fold and no resize corner, and draws its own shape - but it moves by the
   * same gesture, which is why that gesture lives in `windowgesture.js` rather
   * than here.
   *
   * Geometry lives in `session.windows[id]` and is persisted; this component
   * owns no position of its own. Pointer moves write live and persist once, on
   * release - `commitWindows` serialises the whole record, and a drag would
   * otherwise do that a few hundred times.
   *
   * **Keyboard.** The prototype is pointer-only, which is not allowed.
   * The grip is a real button rather than
   * a `tabindex`-ed header, so it carries a role and a name: with focus on it
   * the arrow keys move the window by 8px (1px with Shift), Escape returns it
   * to its default position, and Enter or Space folds it. The resize corner is
   * a second button that sizes by the same increments. The grip's ring is drawn
   * around the title bar, which is what it moves; the corner draws its own,
   * around itself, which is what it sizes from.
   *
   * The grip has no `onclick`: a pointer drag that starts on it ends in one,
   * and a window that folded every time it was dragged by its handle would be
   * unusable. Folding by pointer is the chevron's job; Enter and Space on the
   * grip are handled in `onGestureKey`, where no drag can be mistaken for
   * them.
   *
   * Windows are **not** modal: nothing here traps focus, and Tab leaves as
   * usual.
   *
   * @type {{
   *   id: string,
   *   title: string,
   *   meta?: string,
   *   pad?: string,
   *   headerExtra?: import('svelte').Snippet,
   *   footer?: import('svelte').Snippet,
   *   children: import('svelte').Snippet,
   * }}
   */
  let { id, title, meta, pad = '5px 7px 9px', headerExtra, footer, children } = $props()

  const win = $derived(session.windows[id])
  const rank = $derived(session.stacking[id] ?? 1)
  const folded = $derived(win.fold)

  const bodyId = $derived(`mc-window-${id}`)
  const titleId = $derived(`mc-window-title-${id}`)

  /**
   * Moving and sizing, which `windowgesture.js` owns for this component and
   * for the tool bar alike. `foldable` is this component's alone: `Enter` and
   * `Space` on the grip fold, as the note above says.
   */
  const { onGestureStart, onGestureMove, onGestureEnd, onGestureKey } = windowGesture({
    id: () => id,
    measuredHeight: () => root?.offsetHeight ?? 260,
    foldable: true,
  })

  /**
   * The header is the drag surface, and its buttons sit on top of it. A
   * pointerdown that reaches the header starts a gesture, captures the pointer
   * and calls `preventDefault`, which between them drag the window while the
   * button is held, leave focus on `<body>`, and - because Pointer Events L3
   * retargets a click to the capture target - can stop the button's `onclick`
   * firing at all. The two buttons opt out the way `.corner` already does.
   *
   * @param {PointerEvent} event
   */
  function keepFromHeader(event) {
    event.stopPropagation()
  }

  /** @type {HTMLElement|undefined} */
  let root = $state()
</script>

<!-- A `<section>` with an accessible name is already a labelled region; the
     pointer handlers on it do nothing but raise it, which the keyboard gets
     from `focusin`. -->
<section
  bind:this={root}
  class="window"
  aria-labelledby={titleId}
  style:left="{win.x}px"
  style:top="{win.y}px"
  style:width="{win.w}px"
  style:height={folded || win.h === null ? null : `${win.h}px`}
  style:max-height={folded || win.h !== null ? null : '64vh'}
  style:z-index={20 + rank}
  onpointerdown={() => raiseWindow(id)}
  onfocusin={() => raiseWindow(id)}
>
  <!-- The title bar is a group of controls, and it is also the drag surface;
       the grip inside it is the keyboard equivalent. -->
  <div
    class="header"
    role="group"
    aria-label={t('editor.window.titleBar', { windowName: title })}
    onpointerdown={(e) => onGestureStart(e, 'move')}
    onpointermove={onGestureMove}
    onpointerup={onGestureEnd}
    onpointercancel={onGestureEnd}
  >
    <button
      type="button"
      class="grip"
      title={t('editor.window.move', { windowName: title })}
      aria-label={t('editor.window.move', { windowName: title })}
      aria-expanded={!folded}
      aria-controls={bodyId}
      onkeydown={(e) => onGestureKey(e, 'move')}
    >
      <Icon name="drag-handle" size={16} />
    </button>

    <h2 class="title" id={titleId}>{title}</h2>
    {#if meta}<div class="meta">{meta}</div>{:else}<div class="spacer"></div>{/if}
    {@render headerExtra?.()}

    <span class="chev" class:up={folded}>
      <IconButton
        icon="chevron-down"
        label={t(folded ? 'editor.window.expand' : 'editor.window.collapse', {
          windowName: title,
        })}
        size={21}
        iconSize={14}
        onclick={() => foldWindow(id)}
        onpointerdown={keepFromHeader}
        aria-expanded={!folded}
        aria-controls={bodyId}
      />
    </span>
    <IconButton
      icon="close"
      label={t('editor.window.close', { windowName: title })}
      size={21}
      iconSize={14}
      onclick={() => closeWindowFocusing(id)}
      onpointerdown={keepFromHeader}
    />
  </div>

  {#if !folded}
    <div class="stack" id={bodyId}>
      <div class="body" style:padding={pad}>{@render children()}</div>
      {@render footer?.()}
    </div>

    <button
      type="button"
      class="corner"
      title={t('editor.window.resize', { windowName: title })}
      aria-label={t('editor.window.resize', { windowName: title })}
      onpointerdown={(e) => onGestureStart(e, 'size')}
      onpointermove={onGestureMove}
      onpointerup={onGestureEnd}
      onpointercancel={onGestureEnd}
      onkeydown={(e) => onGestureKey(e, 'size')}
    >
      <Icon name="resize-corner" size={13} />
    </button>
  {/if}
</section>

<style>
  .window {
    position: absolute;
    display: flex;
    flex-direction: column;
    /* Rounds the body's corners and nothing else - this box is not meant to
       scroll, and `hidden` would leave it a scroll container the way `.editor`
       was one. The body inside keeps its own `overflow-y: auto`. */
    overflow: clip;
    border-radius: var(--r-lg);
    background: var(--panel);
    box-shadow: var(--edge);
    animation: mcIn 150ms ease-out;
  }

  .header {
    display: flex;
    align-items: center;
    flex: none;
    gap: 7px;
    height: 29px;
    padding: 0 5px 0 7px;
    border-bottom: 1px solid var(--line);
    cursor: grab;
    touch-action: none;
  }
  .header:active { cursor: grabbing }
  /* The grip moves the whole window, so its ring is drawn around the title
     bar - the thing the user is holding. The corner is a different control in
     a different place and draws its own ring there: a ring at the top of the
     window for a corner at the bottom of it points at the wrong element, and
     makes grip-focus and corner-focus look identical (WCAG 2.4.7, 2.4.11). */
  .header:has(.grip:focus-visible) {
    outline: 2px solid var(--accent);
    outline-offset: -2px;
  }

  .grip {
    display: flex;
    align-items: center;
    flex: none;
    padding: 0;
    border: none;
    background: transparent;
    color: var(--t3);
    cursor: grab;
  }
  .grip:hover { color: var(--t2) }
  .grip:focus-visible { outline: none }

  .title {
    flex: 0 0 auto;
    margin: 0;
    font-size: 10px;
    letter-spacing: .1em;
    font-weight: 600;
    text-transform: uppercase;
    color: var(--t2);
    white-space: nowrap;
  }

  .meta {
    flex: 1 1 auto;
    min-width: 0;
    text-align: right;
    font-size: 10.5px;
    color: var(--t3);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .spacer { flex: 1 1 auto }

  .chev { display: flex; transition: transform var(--dur) var(--ease) }
  .chev.up { transform: rotate(180deg) }

  /* `flex-basis: auto`, not 0: a window with no explicit height sizes to its
     content, and a basis of 0 collapses it to nothing to grow from. With a
     height set, `flex-grow` still fills the space. */
  .stack {
    display: flex;
    flex-direction: column;
    flex: 1 1 auto;
    min-height: 0;
  }

  .body {
    flex: 1 1 auto;
    min-height: 0;
    overflow-y: auto;
  }

  .corner {
    position: absolute;
    right: 0;
    bottom: 0;
    display: flex;
    align-items: flex-end;
    justify-content: flex-end;
    width: 17px;
    height: 17px;
    padding: 1px;
    border: none;
    background: transparent;
    color: var(--t3);
    cursor: nwse-resize;
    touch-action: none;
  }
  .corner:hover { color: var(--t2) }
  /* Inset, because the corner sits flush against two edges of a window that
     clips its overflow: an outset ring would be half hidden. */
  .corner:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: -2px;
    border-radius: var(--r-sm);
  }
</style>
