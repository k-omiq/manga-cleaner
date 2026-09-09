<script>
  /**
   * The cost confirmation, asked before the first
   * spend of a session unconditionally, and before later spends only while
   * *Confirm before spending* is set. Which of those applies is the adapter's
   * decision, not this dialog's; the dialog is raised when it is asked for.
   *
   * The estimate is `spec.props.estimatedCost`, a number, formatted by the
   * catalogue's `currency` format. **It is the only money the app writes**, and
   * it is written through `Intl.NumberFormat` rather than assembled with a `$`
   * - see `masks.value.cloudCost`, which is the same number in the provenance
   * row.
   *
   * `blocking: true` and the buttons come from `spec`, as with the transmission
   * statement. Resolving with `confirm` carries `confirmSpend: true` back into
   * `applyTool`; anything else spends nothing.
   */
  import { Button, Modal } from '../ui/index.js'
  import { closeModal, modalWidth } from '../state/app.svelte.js'
  import { t } from '../i18n/index.js'

  /** @type {{ spec: import('../state/app.svelte.js').ModalSpec }} */
  let { spec } = $props()

  const estimatedCost = $derived(Number(/** @type {any} */ (spec.props)?.estimatedCost) || 0)
</script>

<Modal
  title={t(spec.titleKey)}
  width={modalWidth(spec.kind)}
  blocking={spec.blocking}
  onclose={spec.dismissable ? () => closeModal(null) : undefined}
>
  <p class="statement">{t('modal.body.cloudCost', { cost: estimatedCost })}</p>


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
