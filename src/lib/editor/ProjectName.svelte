<script>
  import { tick } from 'svelte'
  import { editor, renameOpenProject } from '../state/editor.svelte.js'
  import { TextInput } from '../ui/index.js'
  import { t } from '../i18n/index.js'

  /**
   * The project name, inline-editable.
   *
   * Click or Enter opens the field, Escape reverts, blur commits, Enter commits
   * and hands focus back to the trigger. The commit goes through the adapter's
   * `renameProject` - never into local state - so the library and the open
   * project cannot disagree about the name.
   *
   * Escape is stopped here rather than left to the shell: the keyboard layer's
   * Escape blurs the focused field, and a blur is a commit, so letting it
   * through would turn "revert" into "save".
   *
   * The closed state is deliberately *not* a `Button`. It is the project's
   * name, at the name's own size and weight, that happens to accept a click -
   * giving it a control's fill would put a box around the one piece of text in
   * the bar that is the user's own words.
   */
  let editing = $state(false)
  let draft = $state('')
  /** @type {{focus: () => void}|undefined} */
  let field = $state()
  /** @type {HTMLButtonElement|undefined} */
  let trigger = $state()

  /** Set while the field is being torn down, so its blur is not a second commit. */
  let closing = false

  const name = $derived(editor.project?.name ?? '')
  const label = $derived(t('editor.action.renameProject'))

  function begin() {
    if (!editor.project) return
    draft = name
    editing = true
  }

  /**
   * @param {{commit: boolean, refocus: boolean}} how
   */
  async function end({ commit, refocus }) {
    if (!editing || closing) return
    closing = true
    editing = false
    if (commit) renameOpenProject(draft)
    if (refocus) {
      await tick()
      trigger?.focus()
    }
    closing = false
  }

  /** @param {KeyboardEvent} event */
  function onkeydown(event) {
    if (event.key === 'Enter') {
      event.preventDefault()
      event.stopPropagation()
      end({ commit: true, refocus: true })
    } else if (event.key === 'Escape') {
      event.preventDefault()
      event.stopPropagation()
      end({ commit: false, refocus: true })
    }
  }

  $effect(() => {
    if (editing) field?.focus()
  })
</script>

{#if editing}
  <div class="field">
    <TextInput
      bind:this={field}
      size="sm"
      value={draft}
      {label}
      onchange={(value) => (draft = value)}
      {onkeydown}
      onblur={() => end({ commit: true, refocus: false })}
    />
  </div>
{:else}
  <button
    bind:this={trigger}
    class="trigger"
    type="button"
    title={label}
    disabled={!editor.project}
    onclick={begin}
  >
    {name}
  </button>
{/if}

<style>
  .field { width: 200px }

  .trigger {
    display: block;
    min-width: 0;
    max-width: 100%;
    height: 22px;
    padding: 0 5px;
    margin-left: -5px;
    border: 1px solid transparent;
    border-radius: var(--r-sm);
    background: transparent;
    color: var(--text);
    font-size: 12px;
    font-weight: 600;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    text-align: start;
    cursor: text;
    transition: background var(--dur-fast) var(--ease);
  }
  .trigger:hover:not(:disabled) { background: var(--accent-soft) }
  .trigger:disabled { cursor: default }
</style>
