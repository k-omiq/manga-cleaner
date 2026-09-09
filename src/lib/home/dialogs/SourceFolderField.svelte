<script>
  import { Button, Field, TextInput } from '../../ui/index.js'
  import { chooseFolder } from '../../api/folder.js'
  import { notify } from '../../state/app.svelte.js'

  /**
   * "Where are the scans?" - the field New project and New chapter both need.
   *
   * One component rather than two copies, because the two dialogs have to agree
   * about a detail that is easy to get subtly different: the typed path and the
   * chosen path are **the same value**. The chooser writes into the text field
   * and the text field is what is submitted, so a user can pick a folder and
   * then edit the path, or paste one and never open the chooser at all. A
   * chooser that set some hidden state the field did not show would give two
   * answers to one question.
   *
   * The button sits beside the field rather than replacing it. Typing is not a
   * fallback for a chooser that does not work - it is the faster path for
   * anyone who already has the path on their clipboard, and it is the only path
   * at all outside a Tauri window, where `chooseFolder` has nothing to open.
   *
   * @type {{
   *   value: string,
   *   onchange: (value: string) => void,
   *   label: string,
   *   description?: string,
   *   placeholder?: string,
   *   browseLabel: string,
   *   chooserTitle: string,
   *   defaultPath?: string,
   *   disabled?: boolean,
   *   onkeydown?: (e: KeyboardEvent) => void,
   * }}
   */
  let {
    value,
    onchange,
    label,
    description,
    placeholder,
    browseLabel,
    chooserTitle,
    defaultPath,
    disabled = false,
    onkeydown,
  } = $props()

  const uid = $props.id()
  const inputId = `${uid}-path`

  let choosing = $state(false)

  async function browse() {
    if (choosing) return
    choosing = true
    try {
      // A dismissed chooser and a chooser that was never there both answer
      // null, and both mean the same thing here: nothing was chosen, so
      // whatever is already in the field stands.
      const chosen = await chooseFolder({ title: chooserTitle, defaultPath: value || defaultPath })
      if (chosen) onchange(chosen)
    } catch {
      // The chooser failing is not the same as the user declining it, and
      // silently doing nothing would look identical to a dismissed dialog.
      notify({ key: 'notice.project.sourcePickFailed', params: {}, tone: 'warn' })
    } finally {
      choosing = false
    }
  }
</script>

<Field {label} {description} controlId={inputId}>
  {#snippet children()}
    <div class="row">
      <TextInput
        id={inputId}
        {value}
        {onchange}
        {placeholder}
        {disabled}
        {onkeydown}
      />
      <Button onclick={browse} disabled={disabled || choosing}>{browseLabel}</Button>
    </div>
  {/snippet}
</Field>

<style>
  /* The field takes the room and the button takes what it needs - the path is
     the long value and the one worth being able to read. */
  .row { display: flex; align-items: center; gap: var(--s-6) }
  .row :global(> *:first-child) { flex: 1; min-width: 0 }
</style>
