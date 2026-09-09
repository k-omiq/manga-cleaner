<script>
  import { editor, pageCount, pageByArrow, readingDirection } from '../state/editor.svelte.js'
  import { pageNavControls } from '../model/paging.js'
  import { IconButton, Readout } from '../ui/index.js'
  import { t } from '../i18n/index.js'

  /**
   * `‹ ›` - page navigation.
   *
   * The two controls sit in fixed positions and are **labelled by reading
   * order, not by screen position**: manga is right-to-left by default, so the
   * left control is *next* and the right control is *previous*.
   * No component decides that -
   * `model/paging.js#pageNavControls` does, and this one asks it for both the
   * action and the label.
   */
  const nav = $derived(pageNavControls(readingDirection()))
  const total = $derived(pageCount())
  const index = $derived(editor.pageIndex + 1)
  // Two digits, as the design file prints it: the readout must not change
  // width as the chapter passes page nine.
  const shown = $derived(String(index).padStart(2, '0'))

  const atFirst = $derived(editor.pageIndex <= 0)
  const atLast = $derived(editor.pageIndex >= total - 1)

  /** @param {'next'|'prev'} action */
  function spent(action) {
    return action === 'next' ? atLast : atFirst
  }
</script>

<IconButton
  icon="chevron-left"
  label={t(nav.left.tooltipKey)}
  shortcut="←"
  size={28}
  disabled={spent(nav.left.action)}
  onclick={() => pageByArrow('left')}
/>
<Readout
  text="{shown} / {total}"
  label={t('editor.readout.page', { index, total })}
  minWidth={48}
/>
<IconButton
  icon="chevron-right"
  label={t(nav.right.tooltipKey)}
  shortcut="→"
  size={28}
  disabled={spent(nav.right.action)}
  onclick={() => pageByArrow('right')}
/>
