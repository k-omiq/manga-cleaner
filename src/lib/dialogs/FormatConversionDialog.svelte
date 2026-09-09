<script>
  /**
   * "Convert to an editable format?" - raised by
   * `openEditorChapter` when `openChapter` comes back with a
   * `pendingConversion`. Neon Alley's WebP chapters are the fixture that
   * triggers it.
   *
   * The dialog is pushed with its actions already declared, but nothing was
   * wired to them: this component is what makes `convert` do something.
   *
   * **The Tauri backend never raises it, and that is deliberate.**
   * `openChapter` in `src-tauri/src/library.rs` returns `pendingConversion:
   * null` unconditionally, because conversion is no longer a question put to
   * the user: a chapter created from JPEG, WebP, GIF or BMP files converted
   * them at ingest, and what arrives here is already a chapter of PNGs. The
   * chapter's own `notice.input.converted` says so. A format even that pass
   * cannot read - AVIF, HEIC - is refused at ingest as
   * `input.skipReason.notAnImage` and listed with the rest of the chapter's
   * input report, and a Convert button for one of those would be a button that
   * cannot convert.
   *
   * So the only thing that reaches this component is the mock's Neon Alley,
   * whose WebP chapters are fixture data. It stays wired because the seam
   * still carries `pendingConversion` and the mock still exercises it.
   *
   * - **Convert** re-opens the same chapter with `{convert: true}`. That is the
   *   whole operation - the adapter rewrites the page files and reports
   *   `notice.convert.finished`; the originals are kept.
   * - **Cancel** leaves the editor. The chapter cannot be edited in a format
   *   the editor cannot write, so staying would be an editor that silently
   *   refuses every action. `back()` goes one level up, to the project's
   *   chapter list, which is where the decision can be taken again.
   *
   * `blocking: true`, so the backdrop is inert; Escape closes, because Cancel
   * exists and Escape means Cancel - and the Escape path has to leave the
   * editor for the same reason the Cancel button does, so both run `dismiss()`.
   *
   * The buttons come from `spec.actions`, as they do in both cloud dialogs, so
   * the labels and the order live in one place - the push in
   * `openEditorChapter` - rather than being declared there and re-typed here.
   * This component supplies only what a spec cannot: what each id *does*.
   */
  import { untrack } from 'svelte'
  import { Button, Modal } from '../ui/index.js'
  import { back, closeModal, modalWidth } from '../state/app.svelte.js'
  import { openEditorChapter } from '../state/editor.svelte.js'
  import { t } from '../i18n/index.js'

  /** @type {{ spec: import('../state/app.svelte.js').ModalSpec }} */
  let { spec } = $props()

  // Fixed for the life of this dialog; the host keys the block by modal id.
  const pending = untrack(() => /** @type {any} */ (spec.props) ?? {})
  const from = String(pending.from ?? '')
  const to = String(pending.to ?? '')
  const fileCount = Number(pending.fileCount) || 0

  let busy = $state(false)

  async function convert() {
    if (busy) return
    busy = true
    closeModal('convert')
    await openEditorChapter(pending.projectId, pending.chapterId, { convert: true })
  }

  function dismiss() {
    closeModal('cancel')
    back()
  }

  /** Action id → what it does. Anything unrecognised leaves the editor. */
  const RUN = { convert, cancel: dismiss }

  /** @param {string} id */
  function run(id) {
    ;(RUN[id] ?? dismiss)()
  }
</script>

<Modal
  title={t(spec.titleKey)}
  width={modalWidth(spec.kind)}
  blocking={spec.blocking}
  onclose={spec.dismissable ? dismiss : undefined}
>
  <p class="statement">
    {t('modal.body.formatConversion', { count: fileCount, from, to })}
  </p>


  {#snippet buttons()}
    {#each spec.actions as action (action.id)}
      <Button
        variant={action.variant ?? 'ghost'}
        disabled={busy && action.id === 'convert'}
        onclick={() => run(action.id)}
      >
        {t(action.labelKey)}
      </Button>
    {/each}
  {/snippet}
</Modal>

<style>
  .statement { margin: 0 }
</style>
