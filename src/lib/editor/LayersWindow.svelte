<script>
  /**
   * The Layers window - masks and review. The shell is `FloatingWindow`; the
   * body is `MaskList`, which Task 8 owns (see that file's header for the
   * contract).
   */
  import { editor, scopedRegions } from '../state/editor.svelte.js'
  import { flaggedCount } from './maskrows.js'
  import { t } from '../i18n/index.js'
  import FloatingWindow from './FloatingWindow.svelte'
  import MaskList from './MaskList.svelte'

  // Both counts are of the panel's scope - the open page, or the strip's
  // viewport in longstrip. A header counting the chapter while the list showed
  // one page would be two answers to one question.
  const regions = $derived(scopedRegions())
  const applied = $derived(regions.filter((region) => region.mask).length)
  const flagged = $derived(flaggedCount(regions))

  // The header counts whatever the panel is currently listing.
  const meta = $derived(
    editor.reviewFilter
      ? t('editor.meta.masksFlagged', { count: flagged })
      : t('editor.meta.masksApplied', { count: applied }),
  )
</script>

<FloatingWindow id="layers" title={t('editor.panel.layers')} {meta} pad="0 7px 12px">
  <MaskList />
</FloatingWindow>
