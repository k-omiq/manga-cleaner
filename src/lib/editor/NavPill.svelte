<script>
  /**
   * Bottom right: what moves through the chapter. The review set on the left
   * of the rule, the pages on the right.
   *
   * The warning glyph names what the first cluster counts. It is decorative -
   * the readout beside it carries the same fact as a translated sentence - so
   * it is `aria-hidden` and the group's own label does the work.
   */
  import Icon from '../icons/Icon.svelte'
  import { t } from '../i18n/index.js'
  import Pill from './Pill.svelte'
  import ReviewControls from './ReviewControls.svelte'
  import PageControls from './PageControls.svelte'
</script>

<div class="anchor">
  <Pill height={36} gap={3} pad="0 6px" label={t('editor.region.chapterNav')}>
    <span class="glyph" title={t('editor.region.reviewSet')} aria-hidden="true">
      <Icon name="warning-triangle" size={14} />
    </span>
    <ReviewControls />
    <span class="rule"></span>
    <PageControls />
  </Pill>
</div>

<style>
  .anchor {
    position: absolute;
    right: 16px;
    bottom: 18px;
    z-index: 40;
  }

  .glyph { display: flex; flex: none; margin: 0 3px; color: var(--t3) }

  .rule {
    flex: none;
    width: 1px;
    height: 18px;
    margin: 0 var(--s-1);
    background: var(--line);
  }
</style>
