<script>
  /**
   * Label + control + description. Two shapes:
   *
   * - `layout="stack"` - label above the control, description under it. The
   *   New project / New chapter dialogs.
   * - `layout="row"` - label left, control right, hairline below. The Settings
   *   and Export rows.
   *
   * The `children` snippet receives `{ labelId, descriptionId }`. Point a
   * grouped control at them (`labelledBy={labelId}`); a single form control
   * should instead be given `controlId` so the label becomes a real `<label
   * for>`.
   *
   * @type {{
   *   label: string,
   *   description?: string,
   *   layout?: 'stack' | 'row',
   *   controlId?: string,
   *   children: import('svelte').Snippet<[{ labelId: string, descriptionId: string }]>,
   * }}
   */
  let { label, description, layout = 'stack', controlId, children } = $props()

  const uid = $props.id()
  const labelId = `${uid}-label`
  const descriptionId = `${uid}-description`
</script>

<div class="field {layout}">
  <div class="line">
    {#if controlId}
      <label class="label" id={labelId} for={controlId}>{label}</label>
    {:else}
      <div class="label" id={labelId}>{label}</div>
    {/if}
    <div class="control">{@render children({ labelId, descriptionId })}</div>
  </div>
  {#if description}
    <div class="description" id={descriptionId}>{description}</div>
  {/if}
</div>

<style>
  /* ---- stack ------------------------------------------------------------ */
  .stack .line { display: block }
  .stack .label {
    display: block;
    margin-bottom: var(--s-2);
    font-size: 11px;
    color: var(--t3);
  }
  .stack .description {
    margin-top: 7px;
    font-size: 10.5px;
    color: var(--t3);
    line-height: 1.5;
  }

  /* ---- row -------------------------------------------------------------- */
  .row { border-bottom: 1px solid var(--line) }
  .row .line {
    display: flex;
    align-items: center;
    gap: var(--s-4);
    min-height: 32px;
  }
  .row .label { flex: 1; font-size: 12px; color: var(--t2) }
  .row .control { display: flex; justify-content: flex-end }
  .row .description {
    padding-bottom: 8px;
    font-size: 10.5px;
    color: var(--t3);
    line-height: 1.5;
  }
</style>
