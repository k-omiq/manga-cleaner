<script>
  import { Modal, Button } from '../ui/index.js'
  import { closeModal } from '../state/app.svelte.js'
  import { editor } from '../state/editor.svelte.js'
  import { session } from '../state/session.svelte.js'
  import { workflowForDetectorModels } from '../model/pipelines.js'
  import { t } from '../i18n/index.js'
  import WorkflowAnalysis from './WorkflowAnalysis.svelte'

  let { spec } = $props()
</script>

<Modal title={t('workflow.title.review')} width={1040} onclose={() => closeModal(null)}>
  <WorkflowAnalysis
    chapterId={spec.props.chapterId}
    initialPageIndex={spec.props.pageIndex ?? editor.pageIndex}
    initialWorkflow={workflowForDetectorModels(session.detectorModels)}
    initialRtProfile={session.detectorModels.includes('rtFull') ? 'full-halves' : 'small-whole'}
  />

  {#snippet buttons()}
    <Button variant="ghost" onclick={() => closeModal('done')}>{t('shell.action.done')}</Button>
  {/snippet}
</Modal>
