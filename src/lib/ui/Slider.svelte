<script>
  /**
   * Labelled range with a value readout. The readout text is also the
   * `aria-valuetext`, so the unit is spoken rather than left as a bare number.
   *
   * `compact` is the tool bar's variant: label, an 88px track and the number,
   * laid out end to end rather than on the panel grid. A bar is one row of
   * things that are each as wide as they need to be, and a slider that
   * reserved an 84px label column inside it would be a column of one.
   *
   * @type {{
   *   id?: string,
   *   label: string,
   *   value: number,
   *   min: number,
   *   max: number,
   *   step?: number,
   *   unit?: string,
   *   format?: (v: number) => string,
   *   onchange: (value: number) => void,
   *   compact?: boolean,
   *   disabled?: boolean,
   * }}
   */
  let {
    id: customId,
    label,
    value,
    min,
    max,
    step = 1,
    unit = '',
    format,
    onchange,
    compact = false,
    disabled = false,
  } = $props()

  const autoId = $props.id()
  const id = $derived(customId ?? autoId)
  const display = $derived(format ? format(value) : `${value}${unit}`)
</script>

<div class="row" class:compact class:disabled>
  <label class="label" for={id}>{label}</label>
  <input
    {id}
    type="range"
    {min}
    {max}
    {step}
    {value}
    {disabled}
    aria-valuetext={display}
    oninput={(e) => onchange(Number(e.currentTarget.value))}
  />
  <output class="readout" for={id}>{display}</output>
</div>

<style>
  /* Three columns, and the same three every row of a panel gets: a fixed label
     column, the control filling what is left, and a fixed readout slot at the
     right. The track therefore starts at one x and ends at one x down the
     whole panel, which is the point - a column of sliders whose tracks each
     began where their own label happened to end read as six unrelated
     controls. The widths are `--field-label` and `--field-readout` so a panel
     can set them once for every row in it - `editor/ToolBar.svelte`'s
     Adjustments popover does. */
  .row {
    display: grid;
    grid-template-columns:
      var(--field-label, 84px)
      minmax(0, 1fr)
      var(--field-readout, 40px);
    align-items: center;
    column-gap: var(--s-3);
    min-height: 28px;
  }
  .label {
    min-width: 0;
    font-size: 11px;
    line-height: 1.3;
    color: var(--t2);
  }
  input[type=range] {
    width: 100%;
    min-width: 0;
    height: 16px;
    margin: 0;
    cursor: pointer;
  }
  .readout {
    min-width: 0;
    text-align: right;
    font-size: 11px;
    color: var(--t2);
  }
  /* End to end, and nothing reserved: the label is as wide as its word, the
     track is a fixed 88 - enough to aim with, short enough that a bar with two
     of them is still a bar - and the number takes what it needs. */
  .row.compact {
    display: flex;
    align-items: center;
    column-gap: var(--s-2);
  }
  .row.compact input[type=range] { flex: none; width: 88px }
  .row.compact .readout {
    flex: none;
    min-width: 36px;
    font-variant-numeric: tabular-nums;
    text-align: left;
  }

  .disabled .label,
  .disabled .readout { opacity: .45 }
  .disabled input[type=range] { opacity: .45; cursor: default }
</style>
