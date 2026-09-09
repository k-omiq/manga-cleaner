<script>
  import { untrack } from 'svelte'
  import { Button, Field, Modal, TextInput } from '../../ui/index.js'
  import { t } from '../../i18n/index.js'
  import { closeModal, modalWidth } from '../../state/app.svelte.js'
  import { library, projectById, renameProject } from '../library.svelte.js'

  /**
   * Rename a project. The name is a label on the library entry, not the folder
   * on disk, and nothing else about the project changes.
   *
   * @type {{ spec: import('../../state/app.svelte.js').ModalSpec }}
   */
  let { spec } = $props()

  const uid = $props.id()
  const nameId = `${uid}-name`
  // Fixed for the life of this dialog - the host keys it by modal id.
  const projectId = /** @type {string} */ (
    untrack(() => /** @type {any} */ (spec.props)?.projectId)
  )
  const project = $derived(projectById(projectId))

  let name = $state(projectById(projectId)?.name ?? '')

  const canRename = $derived(
    !!project && name.trim() !== '' && name.trim() !== project.name && !library.busy,
  )

  async function rename() {
    if (!canRename) return
    const renamed = await renameProject({ projectId, name: name.trim() })
    // Only on success: a failed rename keeps the dialog and the typed name.
    if (renamed) closeModal('rename')
  }

  /** @param {KeyboardEvent} event */
  function onkeydown(event) {
    if (event.key === 'Enter') {
      event.preventDefault()
      rename()
    }
  }
</script>

<Modal
  title={t(spec.titleKey)}
  width={modalWidth(spec.kind)}
  onclose={() => closeModal(null)}
>
  <div class="form">
    <Field
      label={t('home.rename.label')}
      controlId={nameId}
    >
      {#snippet children()}
        <TextInput
          id={nameId}
          value={name}
          onchange={(value) => (name = value)}
          {onkeydown}
        />
      {/snippet}
    </Field>
  </div>

  {#snippet buttons()}
    <Button onclick={() => closeModal(null)}>{t('shell.action.cancel')}</Button>
    <Button variant="primary" disabled={!canRename} onclick={rename}>
      {t('home.action.rename')}
    </Button>
  {/snippet}
</Modal>

<style>
  .form { margin-top: 16px }
</style>
