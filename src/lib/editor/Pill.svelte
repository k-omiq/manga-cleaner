<script>
  /**
   * The editor's one recurring surface: a rounded `--panel` bar floating over
   * the canvas with the `--edge` shadow. Every cluster in the chrome is one of
   * these - the identity line, the panel toggles, the view controls, the two
   * bottom bars.
   *
   * `height`, `gap` and `pad` differ per cluster and come from the design file,
   * so they are props rather than variants: five slightly different numbers are
   * not five kinds of thing.
   *
   * @type {{
   *   height?: number,
   *   gap?: number,
   *   pad?: string,
   *   label?: string,
   *   role?: string,
   *   children: import('svelte').Snippet,
   * }}
   */
  let { height = 34, gap = 8, pad = '0 13px', label, role, children } = $props()
</script>

<div
  class="pill"
  style:height="{height}px"
  style:gap="{gap}px"
  style:padding={pad}
  role={role ?? (label ? 'group' : undefined)}
  aria-label={label}
>
  {@render children()}
</div>

<style>
  .pill {
    display: flex;
    align-items: center;
    flex: none;
    min-width: 0;
    border-radius: var(--r-lg);
    background: var(--panel);
    box-shadow: var(--edge);
  }
</style>
