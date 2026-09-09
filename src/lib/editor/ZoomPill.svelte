<script>
  /**
   * Bottom centre: what acts on the view. Zoom on the left of the rule,
   * history on the right - both describe the page directly above the pill.
   */
  import { t } from '../i18n/index.js'
  import Pill from './Pill.svelte'
  import ZoomControls from './ZoomControls.svelte'
  import HistoryControls from './HistoryControls.svelte'
</script>

<div class="anchor">
  <Pill height={36} gap={3} pad="0 6px" label={t('editor.region.viewControls')}>
    <ZoomControls />
    <span class="rule"></span>
    <HistoryControls />
  </Pill>
</div>

<style>
  .anchor {
    position: absolute;
    left: 50%;
    bottom: 18px;
    transform: translateX(-50%);
    z-index: 40;
  }

  .rule {
    flex: none;
    width: 1px;
    height: 18px;
    margin: 0 var(--s-1);
    background: var(--line);
  }
</style>
