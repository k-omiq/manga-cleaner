<script>
  import { statusMark } from './status.js'
  import { t } from '../i18n/index.js'

  /**
   * A rollup status, as a glyph *and* the word - never one without the other,
   * and never a colour on its own.
   *
   * `chip` is the floating badge on a project cover; `inline` is the bare
   * glyph a chapter row puts in its 14px mark cell, where the status word is
   * already spelled out in its own column. In `inline` the word still reaches
   * assistive tech, via the visually-hidden span.
   *
   * @type {{
   *   status: 'notStarted'|'inProgress'|'review'|'completed',
   *   statusKey: string,
   *   variant?: 'chip'|'inline',
   * }}
   */
  let { status, statusKey, variant = 'chip' } = $props()

  const mark = $derived(statusMark(status))
  const word = $derived(t(statusKey))
</script>

{#if variant === 'chip'}
  <span class="chip" class:dim={mark.dim}>
    <span class="glyph" aria-hidden="true">{mark.glyph}</span>{word}
  </span>
{:else}
  <span class="inline" class:dim={mark.dim}>
    <span aria-hidden="true">{mark.glyph}</span>
    <span class="sr">{word}</span>
  </span>
{/if}

<style>
  .chip {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    padding: 3px 7px;
    border-radius: var(--r-sm);
    background: var(--surface);
    box-shadow: var(--edge-soft);
    color: var(--t2);
    font-size: 10px;
    white-space: nowrap;
  }
  .glyph { font-size: 10px; line-height: 1 }

  .inline {
    display: block;
    width: 14px;
    flex: none;
    text-align: center;
    font-size: 11.5px;
    color: var(--t2);
  }
  .dim { color: var(--t3) }

  .sr {
    position: absolute;
    width: 1px;
    height: 1px;
    margin: -1px;
    padding: 0;
    overflow: hidden;
    clip-path: inset(50%);
    white-space: nowrap;
  }
</style>
