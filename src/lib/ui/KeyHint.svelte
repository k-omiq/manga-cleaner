<script>
  /**
   * A keycap, or a run of them. `keys` is a single key or a sequence
   * (`['Ctrl', 'E']`) - each is rendered as its own `<kbd>`, never concatenated
   * into a string, so no separator has to survive translation.
   *
   * @type {{ keys: string | string[] }}
   */
  let { keys } = $props()

  const list = $derived(Array.isArray(keys) ? keys : [keys])
</script>

<span class="keys">
  {#each list as k, i (i)}<kbd>{k}</kbd>{/each}
</span>

<style>
  .keys { display: inline-flex; align-items: center; gap: 3px }

  kbd {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    min-width: 17px;
    height: 17px;
    padding: 0 4px;
    border: 1px solid var(--line2);
    border-radius: var(--r-xs);
    background: var(--accent-soft);
    color: var(--t2);
    font-family: inherit;
    font-size: 10.5px;
    line-height: 1;
    white-space: nowrap;
  }
</style>
