<script>
  import Icon from '../icons/Icon.svelte'

  /**
   * Header row plus animated body. Two chromes ship from one component:
   *
   * - `variant="panel"` - the Tools panel parameter dropdown. Chevron trails
   *   the summary; the row fills with `--panel2` behind a `--line2` hairline
   *   while `selected`.
   * - `variant="plain"` - the Masks panel provenance expander. Chevron leads,
   *   no chrome, body indented under the summary.
   *
   * The summary is a button; anything else interactive in the header (a delete
   * control, say) goes in the `trailing` snippet, which renders as a sibling so
   * no interactive element nests inside another.
   *
   * @type {{
   *   open: boolean,
   *   ontoggle: (open: boolean) => void,
   *   label?: string,
   *   variant?: 'panel' | 'plain',
   *   selected?: boolean,
   *   disabled?: boolean,
   *   summary: import('svelte').Snippet,
   *   trailing?: import('svelte').Snippet,
   *   children: import('svelte').Snippet,
   * }}
   */
  let {
    open,
    ontoggle,
    label,
    variant = 'panel',
    selected = false,
    disabled = false,
    summary,
    trailing,
    children,
  } = $props()

  // The body is not rendered while collapsed, so aria-controls is only emitted
  // when it points at an element that actually exists.
  const bodyId = $props.id()
</script>

<div class="dsc {variant}" class:selected class:open>
  <div class="head">
    <button
      type="button"
      class="summary"
      class:lead={variant === 'plain'}
      {disabled}
      aria-expanded={open}
      aria-controls={open ? bodyId : undefined}
      aria-label={label}
      onclick={() => ontoggle(!open)}
    >
      <span class="chev" class:spun={open}>
        <Icon name="chevron-down" size={13} />
      </span>
      <span class="summary-body">{@render summary()}</span>
    </button>
    {@render trailing?.()}
  </div>

  {#if open}
    <div class="body" id={bodyId}>
      {@render children()}
    </div>
  {/if}
</div>

<style>
  .head {
    display: flex;
    align-items: center;
    gap: var(--s-1);
  }

  .summary {
    flex: 1;
    min-width: 0;
    display: flex;
    align-items: center;
    gap: var(--s-3);
    padding: 0;
    border: none;
    background: transparent;
    color: inherit;
    text-align: start;
    cursor: pointer;
    font-size: 11.5px;
  }
  .summary:disabled { opacity: .45; cursor: default }

  /* panel: chevron trails the summary; plain: it leads. */
  .summary { flex-direction: row-reverse }
  .summary.lead { flex-direction: row }

  .summary-body { flex: 1; min-width: 0 }

  .chev {
    display: flex;
    flex: none;
    color: var(--t3);
    transition: transform var(--dur) var(--ease);
  }
  .chev.spun { transform: rotate(180deg) }

  .body { animation: mcFade var(--dur) var(--ease) }

  /* ---- panel ------------------------------------------------------------ */
  .panel {
    border: 1px solid transparent;
    border-radius: var(--r-chip);
    padding: 0 var(--s-3) 0 var(--s-2);
    margin-bottom: 2px;
    transition: background var(--dur-fast) var(--ease);
  }
  .panel.selected {
    background: var(--panel2);
    border-color: var(--line2);
  }
  .panel .head { min-height: 26px }
  .panel .body { padding: 2px 0 8px }

  /* ---- plain ------------------------------------------------------------ */
  .plain .head { min-height: 26px }
  .plain .body { padding: 6px 0 4px 20px }
</style>
