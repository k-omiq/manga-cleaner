<script>
  import { Button, Field, Modal, Segmented, TextInput } from '../../ui/index.js'
  import { t } from '../../i18n/index.js'
  import { closeModal, modalWidth } from '../../state/app.svelte.js'
  import { createProject, library } from '../library.svelte.js'
  import SourceFolderField from './SourceFolderField.svelte'

  /**
   * New project.
   *
   * Mode is the only thing on this screen that can never be undone: it decides
   * how pages are stitched and split, so changing it later would invalidate
   * every split point in the project. It is stated three times, in three
   * registers - the field's own description, the dialog's footnote, and the
   * commit button, which names the mode it is about to fix ("Create longstrip
   * project"). One of those is a line of copy someone can skim past; all three
   * are not.
   *
   * @type {{ spec: import('../../state/app.svelte.js').ModalSpec }}
   */
  let { spec } = $props()

  const uid = $props.id()
  const nameId = `${uid}-name`

  let sourcePath = $state('')
  let name = $state('')
  let nameTouched = $state(false)
  /** @type {'single'|'longstrip'} */
  let mode = $state('single')

  /** The folder's own name is almost always the project's name. */
  $effect(() => {
    const segment = sourcePath.split('/').filter(Boolean).pop() ?? ''
    if (!nameTouched) name = segment
  })

  const canCreate = $derived(name.trim() !== '' && !library.busy)

  async function create() {
    if (!canCreate) return
    const project = await createProject({
      name: name.trim(),
      mode,
      sourcePath: sourcePath.trim() || undefined,
    })
    // A failed create announces itself as a notice and leaves the dialog
    // standing with what was typed still in it. Closing would discard the name
    // and the path and leave nothing on screen that failed.
    if (project) closeModal('create')
  }

  /** @param {KeyboardEvent} event */
  function onkeydown(event) {
    if (event.key === 'Enter') {
      event.preventDefault()
      create()
    }
  }
</script>

<Modal
  title={t(spec.titleKey)}
  width={modalWidth(spec.kind)}
  onclose={() => closeModal(null)}
>
  <div class="form">
    <SourceFolderField
      value={sourcePath}
      onchange={(value) => (sourcePath = value)}
      label={t('home.newProject.source')}
      placeholder={t('home.newProject.sourcePlaceholder')}
      browseLabel={t('shell.action.chooseFolder')}
      chooserTitle={t('home.newProject.source')}
      {onkeydown}
    />

    <Field label={t('home.newProject.name')} controlId={nameId}>
      {#snippet children()}
        <TextInput
          id={nameId}
          value={name}
          onchange={(value) => {
            nameTouched = true
            name = value
          }}
          placeholder={t('home.newProject.namePlaceholder')}
          {onkeydown}
        />
      {/snippet}
    </Field>

    <Field label={t('home.newProject.mode')} description={t('home.newProject.modeNote')}>
      {#snippet children({ labelId })}
        <Segmented
          options={[
            { value: 'single', label: t('project.mode.single') },
            { value: 'longstrip', label: t('project.mode.longstrip') },
          ]}
          value={mode}
          onchange={(value) => (mode = /** @type {'single'|'longstrip'} */ (value))}
          size="md"
          align="start"
          labelledBy={labelId}
        />
      {/snippet}
    </Field>
  </div>

  {#snippet footnote()}
    {t('home.newProject.permanent')}
  {/snippet}

  {#snippet buttons()}
    <Button onclick={() => closeModal(null)}>{t('shell.action.cancel')}</Button>
    <Button variant="primary" disabled={!canCreate} onclick={create}>
      {t(mode === 'longstrip'
        ? 'home.newProject.createLongstrip'
        : 'home.newProject.createSingle')}
    </Button>
  {/snippet}
</Modal>

<style>
  .form { display: flex; flex-direction: column; gap: var(--s-5); margin-top: 16px }
</style>
