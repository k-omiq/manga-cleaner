<script>
  /**
   * The transmission statement, asked once per
   * session before the first cloud request.
   *
   * The sentence is the whole point of the dialog, and it is the one the
   * revision-1 spec got wrong: what leaves the machine is **a bounded crop of
   * the page including the image content around the text**, not the masked
   * region alone. Steps 5 and 6 need that ring. Saying "the region" here would
   * be a false statement about what the user's scans are used for.
   *
   * Pushed by `src/lib/editor/cloudflow.svelte.js#confirmCloud` with
   * `blocking: true` - Escape still closes it, because Cancel exists and
   * Escape means Cancel; the backdrop is inert, because a stray click is not a
   * decision about a third party. Resolving with `continue` is what carries
   * `acknowledgeTransmission: true` back into `applyTool`; anything else
   * abandons the request and sends nothing.
   *
   * The buttons come from `spec.actions` rather than being written here, so the
   * ids and labels stay wherever the flow declares them.
   */
  import { Button, Modal } from '../ui/index.js'
  import { closeModal, modalWidth } from '../state/app.svelte.js'
  import { t } from '../i18n/index.js'

  /** @type {{ spec: import('../state/app.svelte.js').ModalSpec }} */
  let { spec } = $props()
</script>

<Modal
  title={t(spec.titleKey)}
  meta={t('modal.meta.cloudTransmission')}
  width={modalWidth(spec.kind)}
  blocking={spec.blocking}
  onclose={spec.dismissable ? () => closeModal(null) : undefined}
>
  <p class="statement">{t('modal.body.cloudTransmission')}</p>


  {#snippet buttons()}
    {#each spec.actions as action (action.id)}
      <Button variant={action.variant ?? 'ghost'} onclick={() => closeModal(action.id)}>
        {t(action.labelKey)}
      </Button>
    {/each}
  {/snippet}
</Modal>

<style>
  .statement { margin: 0 }
</style>
