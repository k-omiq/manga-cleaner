<script>
  /**
   * The tool bar: whichever tool the rail has selected, its parameters as
   * controls, and - for Auto clean - the button that starts and cancels the
   * run.
   *
   * **It is as long as the tool needs it to be.** `width: max-content`, one
   * row, forty-four pixels tall, and it grows and shrinks as the tool changes.
   * Nothing stores its width: the bar measures what the browser made of it and
   * writes that back through `setWindowBox`, because `clampPosition` keeps a
   * grabbable strip on screen and needs a truthful number to do it with. A
   * geometry stored before it was a bar may still carry an `h` and a `fold`
   * for this window; `sanitizeSession` drops both on read, so nothing here has
   * to ignore them.
   *
   * **Everything that can be a picture is one.** A choice whose options all
   * carry an icon is a group of icon cells - four shapes, aligned against
   * non-aligned, clone against heal - and a choice whose options are names
   * is a dropdown showing the current one. What is left is Size, which is
   * inline because it is the control a hand reaches for constantly, the
   * colour's swatch, and *Adjustments*: every other slider and the colour's hex
   * field, one press away in a popover. A bar with six sliders on it would not
   * be a bar.
   *
   * **A `<section>` with a name, not a `role="toolbar"`.** A toolbar promises a
   * single tab stop with the arrow keys moving between its controls, and the
   * arrows here already belong to the radio groups and the sliders inside it -
   * claiming the role would describe a keyboard model this does not implement.
   * A labelled region is what it actually is, and it is named by the tool's own
   * name on screen (`aria-labelledby`) rather than by a second copy of it.
   *
   * **Labels and controls, no prose.** The two sentences here are not
   * explanations of a tool but reasons a control is unavailable - a blocked
   * option's gate note and the run's own - and each appears only while
   * something is blocked. Every cloud option is gated by `session.cloudAllowed`:
   * a blocked rung is shown disabled and says why, rather than being hidden, so
   * the user can see what the setting is costing them.
   *
   * The run lives here because the cancel belongs where the run was started.
   * The Pages window shows the progress and grows no second cancel.
   */
  import { untrack } from 'svelte'
  import { session, raiseWindow, setWindowBox } from '../state/session.svelte.js'
  import { capabilities } from '../state/capabilities.svelte.js'
  import {
    editor,
    setToolParam,
    startRun,
    cancelRun,
    currentPage,
    runningPage,
  } from '../state/editor.svelte.js'
  import {
    TOOL_ICONS,
    effectiveChoice,
    hexInvalid,
    hexOnCommit,
    hexOnInput,
    paramGroups,
    toolSpec,
  } from './tools.js'
  import { windowGesture, closeWindowFocusing } from './windowgesture.js'
  import {
    Button,
    IconButton,
    Menu,
    Popover,
    Segmented,
    Slider,
    TextInput,
  } from '../ui/index.js'
  import Icon from '../icons/Icon.svelte'
  import { t } from '../i18n/index.js'

  const spec = $derived(toolSpec(editor.tool))
  const values = $derived(editor.toolParams[spec.id] ?? {})
  const win = $derived(session.windows.tool)
  const rank = $derived(session.stacking.tool ?? 1)

  const uid = $props.id()
  const titleId = `${uid}-name`
  const hexNoteId = `${uid}-hex`
  const blockedId = `${uid}-blocked`

  /**
   * The controls this tool is showing **now**, in their groups.
   *
   * A parameter may be live only under another parameter's value - the Shape's
   * colour and opacity while it is a solid fill rather than an engine -
   * and those controls are absent rather than disabled: nothing is blocked, so
   * there is nothing to explain (`tools.js#activeParams`). The groups are the
   * spec's own (`tools.js#section`); this component draws whatever it is handed
   * and decides no grouping of its own.
   *
   * What it *does* decide is which side of the Adjustments button each
   * parameter falls on, and that is the one rule the bar owns: `size` and the
   * colour's swatch stay out on the bar, and every other slider goes behind the
   * button with the hex field. A group left with nothing on the bar draws no
   * hairline of its own.
   */
  const layout = $derived.by(() => {
    /** @type {Array<{key: string|null, params: Array<any>}>} */
    const groups = []
    /** @type {Array<import('./tools.js').RangeParam>} */
    const behind = []
    /** @type {import('./tools.js').ColorParam|null} */
    let colorParam = null

    for (const group of paramGroups(spec, values)) {
      /** @type {Array<any>} */
      const onBar = []
      for (const param of group.params) {
        if (param.kind === 'range' && param.key !== 'size') behind.push(param)
        else {
          if (param.kind === 'color') colorParam = param
          onBar.push(param)
        }
      }
      if (onBar.length) groups.push({ key: group.key, params: onBar })
    }
    return { groups, behind, colorParam }
  })

  /** Whether the Adjustments button has anything to open. */
  const adjustable = $derived(layout.behind.length > 0 || layout.colorParam !== null)

  /**
   * A choice is a group of icon cells when **every** option has a glyph, and a
   * dropdown otherwise. Never a mixture: a row of cells where some are pictures
   * and some are words has no rhythm, and the spec is written so the question
   * never arises (`tools.js#ChoiceOption`).
   *
   * @param {import('./tools.js').ChoiceParam} param
   */
  function iconChoice(param) {
    return param.options.every((option) => !!option.icon)
  }

  /**
   * @param {import('./tools.js').ChoiceParam} param
   */
  function optionsFor(param) {
    return param.options
      // A cloud option is shown disabled and says why - the user should be able
      // to see what the setting is costing them. A **missing engine** is the
      // opposite case and §4a's own rule: rung 3a needs a sidecar the
      // application ships no Python for, and rungs 2 and 3 need weights that
      // are downloaded after install. On a machine
      // without them there is no control worth drawing - the remedy is one
      // press in Settings › Models, and a disabled option here would be an
      // explanation in the wrong window. Hidden rather than disabled.
      .filter((option) => !option.engine || capabilities.engines[option.engine] !== false)
      .map((option) => ({
        value: option.value,
        label: t(option.labelKey),
        icon: option.icon,
        disabled: option.cloud && !session.cloudAllowed,
        title: option.cloud && !session.cloudAllowed ? t('editor.state.cloudBlocked') : undefined,
      }))
  }

  /**
   * Why a control has an option nobody can choose, as text.
   *
   * The reason used to live only in a `title` on the disabled option, where it
   * reached no one: browsers fire no hover events on a disabled form control,
   * so that tooltip never appears. §7.6's point is that the user should be able
   * to see what the setting is costing them, and a note that is only there
   * while something is blocked is how they see it. A dropdown carries it inside
   * itself, under the items (`ui/Menu.svelte`'s `note`); an icon group has no
   * inside, so it carries it beside itself on the bar.
   *
   * @param {import('./tools.js').ChoiceParam} param
   * @returns {string|null}
   */
  function gateNote(param) {
    const gated = param.options.some((option) => option.cloud) && !session.cloudAllowed
    if (gated) return t('editor.state.cloudBlocked')
    // Rung 3a's half of the same duty. `sidecarReasonKey` is null for the
    // ordinary machine that installed nothing - absence is silent - and carries
    // a `decline.reason.sidecar*` sentence only where something *is* installed
    // and this machine still will not carry it. That sentence had no surface at
    // all until the tool window rendered it.
    const withheld =
      param.options.some((option) => option.sidecar) &&
      !capabilities.sidecar &&
      capabilities.sidecarReasonKey
    return withheld ? t(capabilities.sidecarReasonKey) : null
  }

  // When an engine option is filtered out (missing sidecar or weights), sync
  // the effective value back into editor.toolParams so the store and the
  // control agree rather than sending a stale rejected rung across the seam.
  // Reading values and calling set are untracked so the effect does not read
  // and write the same state or re-trigger on unrelated parameter changes.
  $effect(() => {
    const currentValues = untrack(() => values)
    for (const group of paramGroups(spec, currentValues)) {
      for (const param of group.params) {
        if (param.kind === 'choice') {
          const opts = optionsFor(param)
          if (opts.length === 0) continue
          const raw = currentValues[param.key]
          const effective = effectiveChoice(param, opts, raw)
          if (raw !== undefined ? raw !== effective : param.options[0]?.value !== effective) {
            untrack(() => set(param.key, effective))
          }
        }
      }
    }
  })

  /**
   * What the user has typed into the hex field since it was last committed, or
   * null while they are not editing it.
   *
   * The field is a second route into a value the swatch owns, so it shows the
   * committed colour whenever nothing is being typed; while something is, it
   * shows the text as typed - otherwise a value on its way to being a colour
   * would be overwritten by the colour it has not become yet. It is one
   * variable because a spec has at most one colour parameter.
   *
   * @type {string|null}
   */
  let hexTyped = $state(null)
  /** @type {string|null} */
  let hexTypedTool = $state(null)

  // Switching tool abandons whatever was half-typed: the next tool's colour is
  // a different value, and carrying the text across would show it under the
  // wrong swatch.
  $effect(() => {
    void spec.id
    void layout.colorParam?.key
    hexTyped = null
    hexTypedTool = spec.id
  })

  /**
   * The eyedropper is Chromium's alone. Where it is missing the button is
   * **absent** rather than disabled: the swatch and the hex
   * field are two working routes to the same value on every platform, so there
   * is nothing being withheld and nothing to explain.
   */
  const hasEyeDropper = typeof (/** @type {any} */ (globalThis).EyeDropper) === 'function'

  /**
   * @param {string} key - the colour parameter this writes
   */
  async function pickFromScreen(key) {
    try {
      const picked = await new (/** @type {any} */ (globalThis).EyeDropper)().open()
      const hex = hexOnCommit(String(picked?.sRGBHex ?? ''))
      if (hex) {
        hexTyped = null
        hexTypedTool = null
        set(key, hex)
      }
    } catch {
      /* the user pressed Escape; the colour is unchanged, which is the answer */
    }
  }

  /**
   * @param {string} key
   * @param {string} text
   */
  function commitHex(key, text) {
    const hex = hexOnCommit(text)
    if (hex) set(key, hex)
    hexTyped = null
    hexTypedTool = null
  }

  const running = $derived(editor.run.active)
  const page = $derived(currentPage())

  const actionLabel = $derived(
    running
      ? t('editor.action.cancelRun')
      : values.scope === 'project'
        ? t('editor.action.runOnProject')
        : t('editor.action.runOnPage'),
  )

  /**
   * The line beside the run button while it is running, in a live region.
   *
   * It names the page the **run** is on, not the page on screen. Reading
   * `currentPage()` here froze the line at "Cleaning page 8" for the whole of a
   * twenty-page run, because the user stays on page 8 while the queue moves.
   * `runningPage()` is null for the moment between starting and the first
   * `page-started`, and the viewed page is the honest answer then.
   *
   * There is no idle counterpart. The tool window carried one - "Click a region
   * on the page to apply Brush. Autosave on." - and on a bar it would be a
   * sentence explaining the tool the user had just chosen, next to the controls
   * that tool brought with it.
   */
  const status = $derived.by(() => {
    if (!running) return ''
    const onIt = runningPage() ?? page
    return t('editor.status.cleaning', { page: onIt ? onIt.number : editor.pageIndex + 1 })
  })

  /**
   * Auto clean's own precondition, which is not an engine choice.
   *
   * The detector, the balloon detector and the script gate are three files the
   * run opens before it looks at a page, and without them `runClean` cannot
   * start at all - `src-tauri/src/run.rs` answers a `notice.run.modelsMissing`
   * and queues nothing. So the button is **disabled with a note** rather than
   * hidden, which is the opposite of what an engine option gets and is right
   * for the same reason: an engine has four alternatives beside it and Auto
   * clean has none, so a tool that quietly lost its only action would read as a
   * broken bar rather than as a machine missing a download.
   */
  const blockedKey = $derived.by(() => {
    if (!spec.runnable) return null
    if (!capabilities.runtime) return 'editor.state.runtimeMissing'
    return capabilities.autoClean ? null : 'editor.state.modelsMissing'
  })

  /**
   * @param {string} key
   * @param {unknown} value
   */
  function set(key, value) {
    setToolParam(spec.id, key, value)
  }

  function run() {
    if (running) cancelRun()
    else startRun(values.scope === 'project' ? 'project' : 'page')
  }

  /** @type {HTMLElement|undefined} */
  let root = $state()

  const { onGestureStart, onGestureMove, onGestureEnd, onGestureKey } = windowGesture({
    id: () => 'tool',
    measuredHeight: () => root?.offsetHeight ?? 44,
  })

  /**
   * The whole bar is the drag surface, and its controls sit on it. A
   * pointerdown that reaches a control is that control's - a gesture would
   * capture the pointer and `preventDefault`, which between them can stop a
   * button's `onclick` firing at all, because Pointer Events L3 retargets the
   * click to the capture target. The grip is the exception: it starts the
   * gesture itself, before this ever sees the event.
   *
   * A control's `label` and its `output` are the control for this purpose:
   * pressing a slider's label is how a pointer focuses that slider, and a
   * gesture started on the word would swallow the transfer along with the
   * click.
   *
   * @param {PointerEvent & {currentTarget: HTMLElement}} event
   */
  function onBarPointerDown(event) {
    raiseWindow('tool')
    const target = /** @type {Element|null} */ (event.target)
    if (
      target?.closest?.(
        'button, input, select, textarea, label, output, [role="radiogroup"], [role="dialog"], .menu',
      )
    ) {
      return
    }
    onGestureStart(event, 'move')
  }

  /**
   * Measure the bar rather than compute it.
   *
   * Its width is the sum of whatever the selected tool put on it, in the
   * user's own font, at the user's own zoom - a number this component could
   * only ever guess at and the element already knows. `setWindowBox` is how it
   * reaches `clampPosition`, which is the one thing that needs it: a bar
   * dragged off the right edge must keep `KEEP_ON_SCREEN` pixels of itself
   * where a hand can reach them, and it cannot do that against a stale width.
   */
  $effect(() => {
    const node = root
    if (!node || typeof ResizeObserver !== 'function') return
    const observer = new ResizeObserver((entries) => {
      // The **border box**, not the content box: `contentRect` stops inside the
      // bar's own horizontal padding, so a width taken from it is a dozen
      // pixels short of the thing `clampPosition` is placing.
      // Older WebKit hands `borderBoxSize` over as one object rather than as a
      // one-element array, and the same engine is under the shipped app.
      const box = entries[0]?.borderBoxSize
      const size = Array.isArray(box) ? box[0] : box
      const width = size?.inlineSize ?? node.offsetWidth
      // A bar being torn down measures 0, and that is not a width to place
      // anything by: keep the last honest answer.
      if (width > 0) setWindowBox('tool', { w: Math.round(width) })
    })
    observer.observe(node)
    return () => observer.disconnect()
  })
</script>

<section
  bind:this={root}
  class="bar"
  aria-labelledby={titleId}
  style:left="{win.x}px"
  style:top="{win.y}px"
  style:z-index={20 + rank}
  onpointerdown={onBarPointerDown}
  onpointermove={onGestureMove}
  onpointerup={onGestureEnd}
  onpointercancel={onGestureEnd}
  onfocusin={() => raiseWindow('tool')}
>
  <button
    type="button"
    class="grip"
    title={t('editor.window.move', { windowName: t(spec.nameKey) })}
    aria-label={t('editor.window.move', { windowName: t(spec.nameKey) })}
    onpointerdown={(e) => onGestureStart(e, 'move')}
    onpointermove={onGestureMove}
    onpointerup={onGestureEnd}
    onpointercancel={onGestureEnd}
    onkeydown={(e) => onGestureKey(e, 'move')}
  >
    <Icon name="drag-handle" size={16} />
  </button>

  <!-- The tool's own icon and name, which is also the bar's accessible name.
       The hint is a tooltip on it: a bar has no room for a second line of text
       beside the name, and a reminder of the gesture is the one thing here
       that a tooltip is honestly enough for. -->
  <span class="ident" title={t(spec.hintKey)}>
    <Icon name={TOOL_ICONS[spec.id]} size={16} />
    <h2 class="name" id={titleId}>{t(spec.nameKey)}</h2>
  </span>

  {#key spec.id}
    {#each layout.groups as group, index (group.key ?? index)}
      <span class="rule" role="separator" aria-orientation="vertical"></span>
      <div class="group">
        {#each group.params as param (param.key)}
          {#if param.kind === 'range'}
            <Slider
              compact
              label={t(param.labelKey)}
              value={Number(values[param.key] ?? param.min)}
              min={param.min}
              max={param.max}
              step={param.step}
              unit={param.unit ?? ''}
              onchange={(value) => set(param.key, value)}
            />
          {:else if param.kind === 'color'}
            <input
              type="color"
              class="swatch"
              value={String(values[param.key] ?? param.default ?? '#000000')}
              aria-label={t(param.labelKey)}
              oninput={(e) => {
                hexTyped = null
                hexTypedTool = null
                set(param.key, e.currentTarget.value)
              }}
            />
          {:else}
            {@const opts = optionsFor(param)}
            {@const value = effectiveChoice(param, opts, values[param.key])}
            {@const note = gateNote(param)}
            {#if iconChoice(param)}
              <!-- The note beside the group is the group's description, not a
                   loose sentence on the bar: without the association a screen
                   reader reaches the disabled cell and is told nothing about
                   why. -->
              {@const gateId = note ? `${uid}-gate-${param.key}` : undefined}
              <Segmented
                options={opts}
                {value}
                label={t(param.labelKey)}
                describedBy={gateId}
                onchange={(next) => set(param.key, next)}
              />
              {#if note}<p class="gate" id={gateId}>{note}</p>{/if}
            {:else}
              {@const current = opts.find((option) => option.value === value)?.label ?? value}
              <Menu
                label={t(param.labelKey)}
                {note}
                items={opts.map((option) => ({
                  id: option.value,
                  label: option.label,
                  disabled: option.disabled,
                  selected: option.value === value,
                }))}
                onselect={(next) => set(param.key, next)}
              >
                {#snippet trigger({ toggle, triggerProps })}
                  <button
                    type="button"
                    class="drop"
                    title={note ?? undefined}
                    aria-label={t('tools.label.choice', {
                      labelKey: param.labelKey,
                      value: current,
                    })}
                    onclick={toggle}
                    {...triggerProps}
                  >
                    <span class="drop-key">{t(param.shortKey ?? param.labelKey)}</span>
                    <span class="drop-value">{current}</span>
                    <Icon name="chevron-down" size={12} />
                  </button>
                {/snippet}
              </Menu>
            {/if}
          {/if}
        {/each}
      </div>
    {/each}

    {#if adjustable}
      <span class="rule" role="separator" aria-orientation="vertical"></span>
      <Popover label={t('tools.action.adjustments')} align="end">
        {#snippet trigger({ toggle, triggerProps })}
          <IconButton
            icon="sliders"
            label={t('tools.action.adjustments')}
            size={28}
            iconSize={15}
            onclick={toggle}
            {...triggerProps}
          />
        {/snippet}

        <div class="adjust">
          {#each layout.behind as param (param.key)}
            <Slider
              label={t(param.labelKey)}
              value={Number(values[param.key] ?? param.min)}
              min={param.min}
              max={param.max}
              step={param.step}
              unit={param.unit ?? ''}
              onchange={(value) => set(param.key, value)}
            />
          {/each}

          {#if layout.colorParam}
            {@const key = layout.colorParam.key}
            {@const shown =
              (hexTypedTool === spec.id ? hexTyped : null) ??
              String(values[key] ?? layout.colorParam.default ?? '#000000')}
            {@const invalid = hexInvalid(shown)}
            <div class="hex">
              <span class="hex-label">{t('tools.param.colorHex')}</span>
              <TextInput
                size="sm"
                value={shown}
                label={t('tools.param.colorHex')}
                spellcheck="false"
                autocapitalize="off"
                autocomplete="off"
                aria-invalid={invalid ? 'true' : undefined}
                aria-describedby={invalid ? hexNoteId : undefined}
                onchange={(text) => {
                  hexTyped = text
                  hexTypedTool = spec.id
                  const hex = hexOnInput(text)
                  if (hex) set(key, hex)
                }}
                onblur={(e) => commitHex(key, e.currentTarget.value)}
                onkeydown={(e) => {
                  if (e.key === 'Enter') commitHex(key, e.currentTarget.value)
                }}
              />
              {#if hasEyeDropper}
                <IconButton
                  icon="eyedropper"
                  label={t('tools.action.eyedropper')}
                  size={26}
                  iconSize={14}
                  onclick={() => pickFromScreen(key)}
                />
              {/if}
            </div>
            {#if invalid}
              <p class="gate hex-note" id={hexNoteId} data-hex-invalid>
                {t('tools.param.colorHexInvalid')}
              </p>
            {/if}
          {/if}
        </div>
      </Popover>
    {/if}

    {#if spec.runnable}
      <span class="rule" role="separator" aria-orientation="vertical"></span>
      <div class="action">
        <!-- The blocked note describes the button it blocks. A disabled control
             announced without it is an action the user is refused for no stated
             reason, which is the whole failure the note exists to prevent. -->
        <Button
          variant="primary"
          size="md"
          disabled={Boolean(blockedKey) && !running}
          aria-describedby={blockedKey ? blockedId : undefined}
          onclick={run}
        >
          <Icon name={running ? 'stop' : 'play'} size={13} />
          {actionLabel}
        </Button>
        <!-- Rendered whether or not there is anything in it: a live region has to
             be on the page *before* the text arrives, or nothing is announced. -->
        <span class="live" aria-live="polite">{status}</span>
        {#if blockedKey}<p class="gate" id={blockedId}>{t(blockedKey)}</p>{/if}
      </div>
    {/if}
  {/key}

  <span class="rule" role="separator" aria-orientation="vertical"></span>
  <IconButton
    icon="close"
    label={t('editor.window.close', { windowName: t(spec.nameKey) })}
    size={26}
    iconSize={14}
    onclick={() => closeWindowFocusing('tool')}
  />
</section>

<style>
  /* One row, as long as its contents and no longer - no width is stored and
     none is set here. The radius is the tool rail's, which is the other thing
     in this screen that is a floating strip of controls rather than a panel.
     Deliberately **no `overflow`**: a dropdown and the Adjustments popover open
     below the bar, and a bar that clipped its overflow would cut them off at
     its own rounded edge. */
  .bar {
    position: absolute;
    display: flex;
    align-items: center;
    gap: var(--s-2);
    width: max-content;
    height: 44px;
    padding: 0 var(--s-2);
    border-radius: 19px;
    background: var(--panel);
    box-shadow: var(--edge);
    animation: mcIn 150ms ease-out;
    cursor: grab;
  }
  .bar:active { cursor: grabbing }
  /* The grip moves the whole bar, so its ring is drawn around the bar - the
     thing the user is holding - exactly as a window's is drawn around the title
     bar it moves (WCAG 2.4.7, 2.4.11). */
  .bar:has(.grip:focus-visible) {
    outline: 2px solid var(--accent);
    outline-offset: -2px;
  }

  .grip {
    display: flex;
    align-items: center;
    justify-content: center;
    flex: none;
    /* 24 rather than the glyph's 16: the smallest target WCAG 2.5.8 will call
       a target, and the one control on the bar a hand goes for without
       looking. */
    width: 24px;
    height: 28px;
    padding: 0;
    border: none;
    background: transparent;
    color: var(--t3);
    cursor: grab;
    /* On the grip alone, and deliberately not on the bar: the bar's background
       is a drag surface too, and `touch-action: none` across the whole of it
       would take the touch gestures away from the slider and the swatch
       sitting on it. */
    touch-action: none;
  }
  .grip:hover { color: var(--t2) }
  .grip:focus-visible { outline: none }

  .ident {
    display: flex;
    align-items: center;
    flex: none;
    gap: var(--s-2);
    padding-right: var(--s-1);
    color: var(--t2);
  }
  /* The window title's own type: the smallest thing on screen that is still a
     name rather than a label. */
  .name {
    margin: 0;
    font-size: 10px;
    letter-spacing: .1em;
    font-weight: 600;
    text-transform: uppercase;
    color: var(--t2);
    white-space: nowrap;
  }

  /* What separates one group of controls from the next. The tool window drew a
     small uppercase heading over each group; a bar has no second line to put
     one on, so a hairline carries it - and the group is structural from there
     on, named by nothing. */
  .rule {
    flex: none;
    width: 1px;
    height: 22px;
    background: var(--line);
  }

  .group {
    display: flex;
    align-items: center;
    gap: var(--s-2);
  }

  /* A dropdown's trigger: what the parameter is, in the muted ink a label is
     drawn in, then the value it holds in the ink of a control. The full label
     is the button's accessible name; this is the short form, because
     "Speech bubble text" and "Text outside bubbles" cannot both sit on a bar in
     front of a model name. */
  .drop {
    display: inline-flex;
    align-items: center;
    flex: none;
    gap: var(--s-2);
    height: 28px;
    padding: 0 var(--s-2) 0 var(--s-3);
    border: 1px solid var(--line2);
    border-radius: var(--r-chip);
    background: var(--accent-soft);
    color: var(--text);
    font-size: 11px;
    white-space: nowrap;
    cursor: pointer;
    transition:
      border-color var(--dur-fast) var(--ease),
      color var(--dur-fast) var(--ease);
  }
  .drop:hover { border-color: var(--tint) }
  .drop-key { color: var(--t3) }

  .swatch {
    -webkit-appearance: none;
    -moz-appearance: none;
    appearance: none;
    flex: none;
    width: 26px;
    height: 26px;
    padding: 0;
    border: 1px solid var(--line2);
    border-radius: var(--r-chip);
    background: transparent;
    cursor: pointer;
  }
  .swatch::-webkit-color-swatch-wrapper { padding: 2px }
  .swatch::-webkit-color-swatch {
    border: none;
    border-radius: calc(var(--r-chip) - 2px);
  }
  .swatch::-moz-color-swatch {
    border: none;
    border-radius: calc(var(--r-chip) - 2px);
  }
  .swatch:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: 1px;
  }

  /* The popover's own body, which is the panel grid the tool window used to
     be: a fixed label column, the control filling what is left, and a readout
     slot at the right, so a column of sliders starts and ends its tracks at one
     x. `ui/Slider.svelte` reads both widths. */
  .adjust {
    display: grid;
    gap: var(--s-2);
    --field-label: 84px;
    --field-readout: 40px;
  }
  .hex {
    display: grid;
    grid-template-columns: var(--field-label) minmax(0, 1fr) auto;
    align-items: center;
    column-gap: var(--s-3);
  }
  .hex-label {
    min-width: 0;
    font-size: 11px;
    line-height: 1.3;
    color: var(--t2);
  }
  .hex-note { margin-left: calc(var(--field-label) + var(--s-3)) }

  .action {
    display: flex;
    align-items: center;
    flex: none;
    gap: var(--s-2);
  }

  /* The run's own line, and it takes no room at all while there is nothing to
     say - an empty live region that still reserved a gap would put a hole in
     the bar for the whole time nothing is running.

     Collapsed rather than `display: none`, and that is the whole point: a
     `display: none` element is out of the accessibility tree, so the region
     would not exist until the moment its first text arrived - which is exactly
     the announcement that would then be missed. This takes no space and stays
     in the tree. */
  .live {
    font-size: 10px;
    line-height: 1.3;
    color: var(--t2);
    max-width: 190px;
  }
  .live:empty {
    position: absolute;
    width: 1px;
    height: 1px;
    overflow: hidden;
    clip-path: inset(50%);
  }

  /* A reason something is unavailable: beside the control it is about, in the
     muted ink the rest of the bar's small text uses. `--t2` rather than `--t3`
     - this is the only channel for a reason the user needs, and `--t3` is under
     4.5:1 against `--panel` in dark. */
  .gate {
    margin: 0;
    max-width: 190px;
    font-size: 10px;
    line-height: 1.3;
    color: var(--t2);
  }
</style>
