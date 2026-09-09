<script>
  import { editor } from '../state/editor.svelte.js'
  import { Readout } from '../ui/index.js'
  import { t } from '../i18n/index.js'

  /**
   * The run's progress, in the Pages panel header.
   *
   * The Pages list *is* the progress indicator
   * and a separate progress bar is not shown. What the header adds is the
   * total - the list can only show the page it is on. The state is carried by a
   * glyph and a count, never by colour (constraints.md), and the count is a
   * `Readout`, so the eye gets `2 / 12` and assistive tech gets the sentence.
   *
   * Cancelling is *not* here: it goes where the run was
   * started, which is Task 10's Auto clean tool.
   */
  const active = $derived(editor.run.active)
  const label = $derived(
    t('editor.run.progress', { done: editor.run.pagesDone, total: editor.run.queued }),
  )
</script>

{#if active}
  <span class="run">
    <span class="dot" aria-hidden="true">●</span>
    <Readout text="{editor.run.pagesDone} / {editor.run.queued}" {label} minWidth={0} />
  </span>
{/if}

<style>
  .run {
    display: inline-flex;
    align-items: center;
    flex: none;
    gap: 3px;
  }

  /* The prototype's own idiom for "working now" (mcBlink, from the verbatim
     keyframe block). It is decoration on top of the glyph and the count, and
     `prefers-reduced-motion` stops it. */
  .dot {
    font-size: 9px;
    line-height: 1;
    color: var(--t2);
    animation: mcBlink 1.4s ease-in-out infinite;
  }
</style>
