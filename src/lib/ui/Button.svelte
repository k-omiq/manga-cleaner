<script>
  /**
   * Text button. `primary` is the accent-filled action in dialogs; `ghost` is
   * everything else; `plain` carries no fill at all - the Home screen's
   * `← Projects` back link, which the design file draws as bare text rather
   * than as a filled control. Metrics from the prototype's btn() kit: h 26 /
   * pad 0 10 / radius 5 / font 12, and font 11 once the height drops below 24.
   *
   * `soft` is `ghost`'s fill at full text strength - the design file's second
   * home action (`New project`), which is `--accent-soft` on `--text` rather
   * than on `--t2` because it sits beside the primary and is its equal in
   * standing, not a secondary control in a toolbar.
   *
   * `raised` is the one that floats over content - the `Continue clean`
   * control sitting on a project's cover art. It is the only variant with a
   * fill of its own (`--surface`) and a shadow, because a translucent
   * `--accent-soft` over artwork reads as a smudge rather than as a control,
   * and it carries the design file's own metrics for that one control
   * (h 28 / radius 7 / font 11) rather than a size from the table.
   *
   * Sizes are the design file's home metrics: `sm` 22, `md` 26, `lg` 28 (the
   * header's `Settings`), `xl` 32 (a chapter list's `New chapter`) and `hero`
   * 40 (the two centred library actions).
   *
   * @type {{
   *   variant?: 'primary' | 'ghost' | 'soft' | 'plain' | 'raised',
   *   size?: 'hero' | 'xl' | 'lg' | 'md' | 'sm',
   *   disabled?: boolean,
   *   block?: boolean,
   *   type?: 'button' | 'submit' | 'reset',
   *   title?: string,
   *   onclick?: (e: MouseEvent) => void,
   *   children?: import('svelte').Snippet,
   *   [key: string]: unknown,
   * }}
   */
  let {
    variant = 'ghost',
    size = 'md',
    disabled = false,
    block = false,
    type = 'button',
    onclick,
    children,
    ...rest
  } = $props()
</script>

<button
  {type}
  {disabled}
  {onclick}
  class="btn {variant} {size}"
  class:block
  {...rest}
>
  {@render children?.()}
</button>

<style>
  .btn {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    gap: var(--s-2);
    border: 1px solid transparent;
    border-radius: var(--r-sm);
    white-space: nowrap;
    cursor: pointer;
    transition:
      background var(--dur-fast) var(--ease),
      color var(--dur-fast) var(--ease),
      box-shadow var(--dur-fast) var(--ease);
  }
  .btn.block { width: 100% }

  .md { height: 26px; padding: 0 10px; font-size: 12px }
  .sm { height: 22px; padding: 0 9px; font-size: 11px }
  .lg { height: 28px; padding: 0 12px; border-radius: var(--r-md); font-size: 11.5px }
  .xl { height: 32px; padding: 0 15px; border-radius: var(--r-lg); font-size: 12px }
  /* The only size that lives in a fixed-width slot (212px, the design file's
     centred home actions), so it is the only one that must survive a label
     wider than its slot: it wraps and grows downwards instead of overflowing
     into a parent that clips its overflow. A one-line label still measures
     exactly 40. */
  .hero {
    min-height: 40px;
    padding: 6px 16px;
    border-radius: var(--r-lg);
    font-size: 12.5px;
    letter-spacing: .02em;
    line-height: 1.35;
    white-space: normal;
    text-align: center;
    text-wrap: balance;
    overflow-wrap: anywhere;
  }

  .ghost { background: var(--accent-soft); color: var(--t2) }
  .ghost:hover:not(:disabled) { color: var(--text) }

  .soft { background: var(--accent-soft); color: var(--text) }
  .soft:hover:not(:disabled) { box-shadow: 0 0 0 3px var(--accent-soft) }

  /* No fill, no box - a text affordance. Padding stays so the hit target is
     the same size as every other button of its height. */
  .plain {
    background: transparent;
    color: var(--t3);
    padding-left: 2px;
    padding-right: 2px;
    letter-spacing: .04em;
  }
  .plain:hover:not(:disabled) { color: var(--text) }

  /* One control, one set of numbers - the design file's cover button. */
  .btn.raised {
    height: 28px;
    border-radius: var(--r-md);
    background: var(--surface);
    color: var(--t2);
    box-shadow: var(--edge-soft);
    font-size: 11px;
  }
  .raised:hover:not(:disabled) { color: var(--text) }

  .primary {
    background: var(--accent);
    color: var(--accent-fg);
    font-weight: 600;
  }
  /* --accent and --text are the same ink in dark, so a colour shift would be
     invisible there. A halo in --accent-soft reads in both themes. */
  .primary:hover:not(:disabled) { box-shadow: 0 0 0 3px var(--accent-soft) }

  .btn:disabled { opacity: .38; cursor: default }
</style>
