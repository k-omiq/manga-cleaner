<script>
  /**
   * Top right: how the page is being looked at, then what can be done with it.
   *
   * Export is drawn in the active icon-button state - it is the one action in
   * the chrome that finishes the job, and the design file gives it the filled
   * treatment for that reason. Settings opens the Settings dialog directly,
   * which is what the `,` in its tooltip does.
   */
  import { pushModal } from '../state/app.svelte.js'
  import { IconButton } from '../ui/index.js'
  import { t } from '../i18n/index.js'
  import Pill from './Pill.svelte'
  import ViewControls from './ViewControls.svelte'
</script>

<div class="cluster" role="group" aria-label={t('editor.region.view')}>
  <Pill height={34} gap={7} pad="0 9px">
    <ViewControls />
  </Pill>

  <Pill height={34} gap={4} pad="0 5px">
    <IconButton
      icon="settings"
      label={t('editor.action.settings')}
      shortcut=","
      onclick={() => pushModal({ kind: 'settings' })}
    />
    <IconButton
      icon="export"
      label={t('editor.action.export')}
      shortcut="E"
      active
      onclick={() => pushModal({ kind: 'export' })}
    />
  </Pill>
</div>

<style>
  .cluster {
    position: absolute;
    right: 16px;
    top: 16px;
    z-index: 40;
    display: flex;
    align-items: center;
    gap: var(--s-3);
  }
</style>
