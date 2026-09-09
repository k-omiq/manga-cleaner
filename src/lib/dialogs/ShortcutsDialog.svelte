<script>
  /**
   * The shortcut sheet on its own - `?` pushes `{kind: 'shortcuts'}` from
   * anywhere, and it must not depend on Settings being open.
   *
   * The body is `ShortcutSheet`, the same component Settings mounts - and the
   * same one a shortcut is rebound in, so `?` reaches the editor as well as the
   * list. Only the heading level differs: here the group headings sit directly
   * under the dialog title, so they are `h3`.
   *
   * The body is focusable for the same reason Settings' is: the sheet's chords
   * are focusable but its group headings, its note and the scroll between them
   * are not, so a scroll container with no `tabindex` of its own would still
   * leave a keyboard user unable to reach the bottom of a long list.
   */
  import { Button, Modal } from '../ui/index.js'
  import { closeModal, modalWidth } from '../state/app.svelte.js'
  import { t } from '../i18n/index.js'
  import ShortcutSheet from './ShortcutSheet.svelte'

  /** @type {{ spec: import('../state/app.svelte.js').ModalSpec }} */
  let { spec } = $props()
</script>

<Modal title={t(spec.titleKey)} width={modalWidth(spec.kind)} onclose={() => closeModal(null)}>
  <!-- Focusable because most of the sheet is not; see the note in
       `SettingsDialog.svelte`. -->
  <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
  <div class="body" role="group" tabindex="0" aria-label={t(spec.titleKey)}>
    <ShortcutSheet headingLevel="h3" />
  </div>

  {#snippet buttons()}
    <Button variant="primary" onclick={() => closeModal('close')}>{t('shell.action.close')}</Button>
  {/snippet}
</Modal>

<style>
  .body { margin-top: 4px }
  .body:focus-visible { outline-offset: -2px }
</style>
