<script>
  /**
   * The Home screen's wide-tracked section label (PROJECTS, CHAPTERS).
   * Renders as a `<div>` by default; pass `as="h2"` where it genuinely heads a
   * region, so the outline is real rather than decorative.
   *
   * @type {{ text: string, as?: string }}
   */
  let { text, as = 'div' } = $props()
</script>

<svelte:element this={as} class="section-label">{text}</svelte:element>

<style>
  .section-label {
    margin: 0;
    font-size: 10.5px;
    font-weight: inherit;
    letter-spacing: .22em;
    text-transform: uppercase;
    color: var(--t3);
  }
</style>
