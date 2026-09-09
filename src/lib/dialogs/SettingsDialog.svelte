<script module>
  /**
   * Whether a `model-progress` event is about a row this dialog does not know
   * is downloading.
   *
   * The event channel is process-wide, so a second window hears the first
   * one's transfer; `catalogue` is a snapshot taken on mount and refreshed only
   * when something *ends*, so the row underneath that live progress can still
   * read "Not installed" with a Download button on it. `true` here means the
   * snapshot is behind the world and one `listModels` would put it right.
   *
   * Pure, and exported, so the decision is pinned without mounting a dialog:
   * everything around it is a timer and a call.
   *
   * @param {import('../api/backend.js').ModelsView|null} catalogue
   * @param {string} id
   * @param {string} runtimeId
   * @returns {boolean}
   */
  export function isStrangerDownload(catalogue, id, runtimeId = 'runtime') {
    // Nothing drawn yet: the mount's own `listModels` is already on its way and
    // a second one would answer the same question twice.
    if (!catalogue) return false
    if (id === runtimeId) return catalogue.runtime.downloading !== true
    const row = catalogue.models.find((model) => model.id === id)
    // An id with no row at all is a catalogue older than the backend - a
    // seventh weight added since this snapshot - which is the same remedy.
    return row?.downloading !== true
  }
</script>

<script>
  /**
   * Settings - five preference rows, the model catalogue, acceleration, the
   * shortcut sheet and About, across five tabs in one 560px dialog.
   *
   * **Tabs, and how the old argument against them was answered.** This file
   * used to carry a case for one scrolling body: a tab strip would put the
   * five preference rows a click further away, would make the dialog's height
   * jump, would need a roving-tabindex widget of its own, and would cost the
   * heading outline. The user overruled it - the body had grown past a
   * thousand pixels and Shortcuts and About were below the fold of a fold  - 
   * so the four objections are answered rather than argued with:
   *
   * - **The five rows did not move.** `General` is the tab the dialog opens
   *   on, so they are exactly where they were: the first thing on screen, no
   *   press to reach them.
   * - **The height does not move.** Every panel is one fixed-height scroller
   *   (`.panel`), so switching tabs changes what is inside the box and never
   *   the box. The height is `min(52vh, 460px)` - capped so a tall screen does
   *   not get a dialog it has to look up and down, and proportional below that
   *   so a short one never needs the modal's own scrollbar as well.
   * - **The strip is a real tab list.** `role="tablist"` / `role="tab"` /
   *   `role="tabpanel"`, `aria-selected`, `aria-controls`, one tab stop for
   *   the whole strip, arrows to move, Home and End to the ends. Selection
   *   follows focus, as it does in `Segmented` two files away, which is safe
   *   here for the reason it is safe there: every panel is already mounted.
   * - **The outline is still real.** The dialog title is the `h2`; each panel
   *   opens with an `h3` naming it, and the shortcut sheet's group headings
   *   are still `h4`s under it. The `h3` is visually hidden because the
   *   selected tab is already that heading on screen, and printing the word
   *   twice, an inch apart, is not a heading - it is an echo.
   *
   * **Every panel is mounted, and only the selected one is shown.** `hidden`
   * rather than `{#if}`, so `aria-controls` names an element that exists, the
   * scroll position of a panel survives a look at another one, and - the part
   * that matters most - the mount-time work is exactly what it was when this
   * was one body: one `listModels`, one `listAccelerators`, one `about`. A
   * tab that mounted on first press would turn opening Settings into four
   * separate rounds of the same calls.
   *
   * **What went in which tab.** The sidecar path, the FLUX backend and the
   * sidecar model went to `Models` rather than to `General` or to a tab of
   * their own: all three answer one question - what this machine can run, and
   * where it came from - which is the question the whole tab answers, and
   * three rows do not make a tab. `General` keeps the five that are genuinely
   * preferences: how it looks, which way pages read, whether the cloud may be
   * used, what the original view does, and the language.
   *
   * **The backend reconciliation is not here.** `session.*` and
   * `backend.readSettings()` are two stores of the same four preferences, and
   * they are reconciled once at boot, in `App.svelte` - doing it on this
   * dialog's mount left the two free to drift for any session in which the
   * dialog was never opened. What this dialog owns is the second half: every
   * change pushes the session's values down to the backend immediately, so the
   * two cannot part company again while it is open.
   *
   * **A panel is focusable when, and only when, its tail is not.** The one
   * `tabindex` this dialog used to carry was on the whole body, because that
   * body ended in About, which holds no focusable descendant at all (WCAG
   * 2.1.1: a scroll container the keyboard cannot reach is pointer-only). Per
   * panel the question is asked again and answers differently: `Acceleration`
   * ends in a placement table and a note, and `About` ends in the written
   * offer of source and the cloud terms, so both are focusable. `General`,
   * `Models` and `Shortcuts` each end in a control - the language row, the
   * token field, Reset all - so tabbing through them reaches the bottom, and
   * a `tabindex` on those would be a stop that does nothing.
   *
   * About stays one press away in either case, which is the point: it is a
   * GPL-3.0 obligation and this dialog is still its only route.
   *
   * **The shortcut section is not a read-only list.** `ShortcutSheet` is where
   * a binding is *changed*, and it writes its own half of the settings - both
   * stores, the same way `push()` below does - so it behaves identically here
   * and mounted on its own by `?`.
   */
  import { Button, Field, Modal, Segmented, TextInput } from '../ui/index.js'
  import { onMount } from 'svelte'
  import { closeModal, modalWidth } from '../state/app.svelte.js'
  import { getBackend } from '../api/backend.js'
  import { chooseFolder } from '../api/folder.js'
  import { CATALOGUES, LOCALE, t } from '../i18n/index.js'
  import { capabilities, loadCapabilities } from '../state/capabilities.svelte.js'
  import {
    backendSettingsPatch,
    session,
    setAccelerator,
    setCloudAllowed,
    setFluxBackend,
    setFluxModel,
    setOriginalView,
    setReadingDirection,
    setSidecarPath,
    setTheme,
  } from '../state/session.svelte.js'
  import ShortcutSheet from './ShortcutSheet.svelte'
  import AboutSection from './AboutSection.svelte'

  /** @type {{ spec: import('../state/app.svelte.js').ModalSpec }} */
  let { spec } = $props()

  let sidecarModels = $state(/** @type {Array<{id: string, label: string}>} */ ([]))

  async function loadModels() {
    if (capabilities.sidecar) {
      try {
        sidecarModels = await getBackend().listSidecarModels()
        if (!session.fluxModel && sidecarModels.length > 0) {
          const defaultModel = sidecarModels.some((m) => m.id === 'flux2-klein-4b')
            ? 'flux2-klein-4b'
            : sidecarModels[0].id
          setFluxModel(defaultModel)
        }
      } catch {
        sidecarModels = []
      }
    } else {
      sidecarModels = []
    }
  }

  $effect(() => {
    if (capabilities.sidecar) {
      loadModels()
    }
  })

  /** Push the session's values down to the backend after any change. */
  async function push() {
    await getBackend().writeSettings(backendSettingsPatch())
    await loadCapabilities()
    await loadModels()
  }

  /* ---------- Models ---------- */

  /**
   * The catalogue, the runtime row, and where a download would go.
   *
   * `null` until the first answer: the section draws nothing rather than a
   * list of six rows all reading "Not installed", which is what a `[]` default
   * would show for the moment before the call returns and is the most alarming
   * thing this dialog could say untruthfully.
   *
   * @type {import('../api/backend.js').ModelsView|null}
   */
  let catalogue = $state(null)

  /**
   * What each download has reported, by id: `{downloaded, total}` while it
   * runs, gone the moment it ends.
   *
   * Held here rather than on `catalogue` because it arrives on the event
   * channel - `model-progress`, the seam's sixth event - and `listModels` is a
   * snapshot that knows nothing between two calls. The two are joined at the
   * row: the catalogue says what a thing *is*, this says what is happening to
   * it now.
   *
   * @type {Record<string, {downloaded: number, total: number|null}>}
   */
  let progress = $state({})

  /** The last failure per id, until something else happens to that row. @type {Record<string, string>} */
  let failures = $state({})

  /**
   * What the last press on a row *answered*, as an i18n key, for the presses
   * that did nothing.
   *
   * Separate from `failures` because none of these is a failure: a download
   * that was already running and a delete that found nothing are both this
   * window looking at a row another window has moved on from.
   * The sentence goes under the row and is cleared by the next thing
   * that happens to it.
   *
   * @type {Record<string, string>}
   */
  let notes = $state({})

  /**
   * The sentence for a press that did nothing, by the answer it gave.
   *
   * Whole keys in a table rather than one assembled from the answer: the
   * catalogue's test asks for keys to be *chosen between* rather than built,
   * because a dead string cannot be found behind a template. An answer with no
   * entry - `started`, `deleted` - is the ordinary one and says nothing.
   */
  const DECLINED = {
    alreadyRunning: 'settings.models.declined.alreadyRunning',
    alreadyInstalled: 'settings.models.declined.alreadyInstalled',
    notFound: 'settings.models.declined.notFound',
    readOnlyElsewhere: 'settings.models.declined.readOnlyElsewhere',
    busy: 'settings.models.declined.busy',
  }

  /** @param {string} id @param {string|null} key */
  function note(id, key) {
    notes = key === null
      ? Object.fromEntries(Object.entries(notes).filter(([held]) => held !== id))
      : { ...notes, [id]: key }
  }

  /**
   * Ask for the catalogue again.
   *
   * `retryStore` is passed on the **open** and on nothing else. The token
   * migration is offered to the credential store once per process, which is
   * what keeps a locked keychain from prompting on every poll
   * and is also what leaves a keychain unlocked since launch unnoticed. An open
   * of this dialog is a user arriving at the one screen that says where their
   * token is, so it is worth one more attempt; the refreshes below - after a
   * press, after a download ends, on a stranger's progress event - are not.
   *
   * @param {{retryStore?: boolean}} [options]
   */
  async function refreshCatalogue(options = {}) {
    try {
      catalogue = await getBackend().listModels(options)
    } catch {
      catalogue = null
    }
  }

  /**
   * A download this window did not start.
   *
   * `model-progress` goes to every subscriber in the process, so a second
   * window hears the first one's transfer - but `catalogue` is a snapshot taken
   * on mount and refreshed only when something ends, so the row underneath the
   * live progress still reads "Not installed" with a Download button on it.
   * One `listModels` puts that right, and it is debounced
   * because the events arrive every 4 MiB and the catalogue is six `stat` calls
   * plus a settings read.
   *
   * @type {ReturnType<typeof setTimeout>|null}
   */
  let unknownRefresh = null

  /** @param {string} id */
  function refreshForStranger(id) {
    if (unknownRefresh !== null || !isStrangerDownload(catalogue, id, RUNTIME_ID)) return
    unknownRefresh = setTimeout(() => {
      unknownRefresh = null
      refreshCatalogue()
    }, 500)
  }

  onMount(() => {
    // The one call that asks for the credential store to be tried again.
    refreshCatalogue({ retryStore: true })
    // One handler for the whole dialog's lifetime. `subscribe` is the merge of
    // both implementations' streams (`api/tauri-events.js`), so this receives
    // the mock's simulated download in a browser and the command's real one
    // inside a Tauri window, with no branch here.
    const unsubscribe = getBackend().subscribe((event) => {
      if (event.type !== 'model-progress') return
      if (event.done) {
        const { [event.id]: _gone, ...rest } = progress
        progress = rest
        // Whatever the last press said about this row is about a press that
        // has now been overtaken by an ending.
        note(event.id, null)
        // A cancellation is a failure with a name the user chose, so it is
        // not shown as one: the row goes back to "Not installed", which is
        // the true thing about it, and that is the whole report.
        failures =
          event.error && event.error !== 'cancelled'
            ? { ...failures, [event.id]: event.error }
            : Object.fromEntries(Object.entries(failures).filter(([id]) => id !== event.id))
        refreshCatalogue()
        // A download changes which engines can run, which is the point of the
        // whole section.
        loadCapabilities()
        return
      }
      failures = Object.fromEntries(Object.entries(failures).filter(([id]) => id !== event.id))
      note(event.id, null)
      // A row that does not know it is downloading is a row drawn before
      // another window pressed. Ask again.
      refreshForStranger(event.id)
      progress = { ...progress, [event.id]: { downloaded: event.downloaded, total: event.total } }
    })
    return () => {
      if (unknownRefresh !== null) clearTimeout(unknownRefresh)
      unsubscribe()
    }
  })

  /**
   * One row's status, as a key and its parameters.
   *
   * Five states and they are ordered by what the reader needs to know first: a
   * download in flight beats everything, then a failure they have to act on,
   * then a digest that did not match, then presence.
   *
   * @param {string} id
   * @param {boolean} installed
   * @param {boolean|null} [sha256Ok]
   * @returns {{key: string, params?: Object}}
   */
  function statusOf(id, installed, sha256Ok) {
    const live = progress[id]
    if (live) {
      const percent = live.total ? Math.floor((live.downloaded / live.total) * 100) : null
      return percent === null
        ? { key: 'settings.models.status.downloading' }
        : { key: 'settings.models.status.downloadingPercent', params: { percent } }
    }
    if (failures[id]) return { key: 'settings.models.status.failed' }
    if (installed && sha256Ok === false) return { key: 'settings.models.status.mismatch' }
    if (installed) return { key: 'settings.models.status.installed' }
    return { key: 'settings.models.status.missing' }
  }

  /**
   * Press Download, and say what the press answered.
   *
   * `started` is the ordinary answer and says nothing - the progress bar is the
   * report. The other two are refusals, and both mean the row was drawn before
   * something else happened to it, so the sentence goes up *and* the list is
   * refreshed.
   *
   * @param {string} id
   */
  async function download(id) {
    failures = Object.fromEntries(Object.entries(failures).filter(([key]) => key !== id))
    note(id, null)
    try {
      const outcome =
        id === RUNTIME_ID
          ? await getBackend().downloadRuntime()
          : await getBackend().downloadModel({ id })
      note(id, DECLINED[outcome] ?? null)
    } catch (error) {
      failures = { ...failures, [id]: String(error) }
    }
    await refreshCatalogue()
  }

  /** @param {string} id */
  async function cancel(id) {
    try {
      await getBackend().cancelDownload({ id })
    } finally {
      await refreshCatalogue()
    }
  }

  /**
   * Press Delete, and say what it found.
   *
   * Three answers rather than a boolean: the file went, there
   * was nothing there, or there is one and it is not this application's to
   * remove. The last two are only reachable from a stale row, which is exactly
   * when the user needs telling rather than left with a button that did
   * nothing.
   *
   * @param {string} id
   */
  async function remove(id) {
    note(id, null)
    try {
      const outcome =
        id === RUNTIME_ID
          ? await getBackend().deleteRuntime()
          : await getBackend().deleteModel({ id })
      note(id, DECLINED[outcome] ?? null)
    } catch (error) {
      failures = { ...failures, [id]: String(error) }
    }
    await refreshCatalogue()
    await loadCapabilities()
  }

  /**
   * Give back the bytes a stopped download left.
   *
   * The row reports them because keeping them is what makes the next Download
   * resume rather than start over, and 180 MB of a 207 MB
   * transfer nobody came back for is disk the user did not agree to spend.
   * `false` means there was nothing there - or that a download is
   * writing into it, which is the same news to this dialog and is why the list
   * is refreshed either way: the row then shows the transfer it lost to.
   *
   * @param {string} id
   */
  async function discard(id) {
    note(id, null)
    try {
      await getBackend().discardPartial({ id })
    } catch (error) {
      failures = { ...failures, [id]: String(error) }
    }
    await refreshCatalogue()
  }

  /** @param {string} id */
  async function verify(id) {
    note(id, null)
    try {
      await getBackend().verifyModel({ id })
    } catch (error) {
      failures = { ...failures, [id]: String(error) }
    }
    await refreshCatalogue()
  }

  /** The id the ONNX Runtime's own download reports under (`src-tauri/src/weights.rs`). */
  const RUNTIME_ID = 'runtime'

  /**
   * Which ONNX Runtime build this machine should fetch.
   *
   * Only Windows publishes more than one - DirectML by default, CUDA 12 and
   * CUDA 13 offered - so the picker is drawn only where there is something to
   * pick, which is what `flavours.length > 1` says without this file knowing
   * which platform it is on. The id is an ordinary setting; the
   * backend resolves anything it does not publish back to the default, so a
   * stale value cannot leave a machine unable to download a runtime at all.
   *
   * It does not re-download by itself. Switching build is 200-455 MB and the
   * press for that is the row's own Download button, which is offered beside
   * Delete wherever there is a choice.
   *
   * @param {string} id
   */
  async function chooseFlavour(id) {
    await getBackend().writeSettings({ runtimeFlavour: id })
    await refreshCatalogue()
  }

  /**
   * One line under the picker for a build that needs something installed first.
   *
   * `userInstalled` is version strings - `CUDA 12.x`, `cuDNN 9.x` - which are
   * data and are interpolated rather than translated, the same way a licence
   * identifier is. Empty for every build but CUDA, and that emptiness
   * is the argument for the default.
   */
  /**
   * The line that says the machine is running a build other than the chosen
   * one, and says nothing at all the rest of the time.
   *
   * Two facts sat side by side and never met: the row's
   * `flavour` is what a Download press *would* fetch, and until that press
   * happens the computer is still running whatever it unpacked last. Both are
   * true; on screen together, with only the first named, they read as a claim
   * about the library on disk. `installedFlavour` is null for a runtime this
   * application did not unpack - unknown rather than none - and an unknown
   * build is nothing to report a difference about.
   */
  const installedNote = $derived.by(() => {
    const runtime = catalogue?.runtime
    if (!runtime?.installed || !runtime.installedFlavour) return null
    const same =
      runtime.installedFlavour === runtime.flavour &&
      runtime.installedVersion === runtime.version
    if (same) return null
    return t('settings.models.runtime.installedDiffers', {
      installed: runtime.installedFlavour,
      installedVersion: runtime.installedVersion,
      chosen: runtime.flavour,
      chosenVersion: runtime.version,
    })
  })

  const flavourNeeds = $derived.by(() => {
    const chosen = catalogue?.runtime.flavours.find((row) => row.id === catalogue?.runtime.flavour)
    if (!chosen || chosen.userInstalled.length === 0) return null
    return t('settings.models.runtime.flavourNeeds', { items: chosen.userInstalled.join(', ') })
  })

  /* ---------- the Hugging Face token ---------- */

  /**
   * The field's contents, which are **write-only**.
   *
   * The stored token is never read back across the seam - `listModels` answers
   * `hasToken` and `tokenStore` and nothing more - so this box starts empty on
   * every open, and "a token is saved" is said in words beside it rather than
   * by filling a field with dots. A password input whose value came from the
   * backend would be a secret sitting in the DOM for the length of a dialog,
   * for no gain: the user cannot read it through the dots anyway.
   */
  let tokenDraft = $state('')

  /**
   * Which sentence goes under the field.
   *
   * The token lives in the operating system's credential store; a computer
   * without one keeps it in `settings.json` instead. That is a
   * difference the user is entitled to know about *before* they paste a token,
   * so the fallback is said whether or not one is saved, and the saved note
   * says which of the two places it went.
   *
   * Three states, not two, because "this build has no credential store" and
   * "there is one and it would not answer" are opposite pieces of news: the
   * first is permanent and the second is usually a locked keychain the user can
   * unlock.
   */
  const tokenNote = $derived.by(() => {
    if (!catalogue) return null
    if (catalogue.tokenStore === 'keychain') {
      return catalogue.hasToken ? t('settings.models.token.saved') : null
    }
    // The two fallbacks read almost the same and mean opposite things, so each
    // has its own sentence rather than sharing one about "no credential store".
    const unreachable = catalogue.tokenStore === 'fileStoreUnavailable'
    if (catalogue.hasToken) {
      return t(unreachable ? 'settings.models.token.savedStoreUnreachable' : 'settings.models.token.savedInFile')
    }
    return t(unreachable ? 'settings.models.token.storeUnreachable' : 'settings.models.token.fileOnly')
  })

  /**
   * The sentence for *why* the store would not answer.
   *
   * Whole keys in a table rather than one assembled from the id, for the reason
   * `DECLINED` above gives: a key built from a value cannot be found by the
   * catalogue's scan, and a dead string survives behind a template. An id with
   * no entry - or none at all, which is every location but the unreachable one
   * - says nothing, and the sentence above it is still true on its own.
   */
  const STORE_REASON = {
    locked: 'settings.models.token.reason.locked',
    unreachable: 'settings.models.token.reason.unreachable',
    ambiguous: 'settings.models.token.reason.ambiguous',
    unknown: 'settings.models.token.reason.unknown',
  }

  /**
   * One line under the note, naming what the store did.
   *
   * Separate from `tokenNote` rather than four more variants of it: the two
   * sentences answer different questions - where the token is, and what is
   * wrong with the place it should be - and a locked keychain reads the same
   * whether or not a token has been saved yet.
   */
  const tokenReason = $derived.by(() => {
    const reason = catalogue?.tokenStoreReason
    if (catalogue?.tokenStore !== 'fileStoreUnavailable' || !reason) return null
    const key = STORE_REASON[reason]
    return key ? t(key) : null
  })

  async function saveToken() {
    const value = tokenDraft.trim()
    if (!value) return
    tokenFailure = false
    await getBackend().writeSettings({ hfToken: value })
    tokenDraft = ''
    await refreshCatalogue()
  }

  /**
   * The sentence `settings::write` rejects with when the credential store kept
   * the secret - `format!("the credential store kept the token: {err}")`, from
   * `TokenWrite::NotCleared`.
   *
   * **Matched as a string, deliberately and unhappily.** The seam carries one
   * `Err(String)` per call and there is no id on it, so the
   * only thing that separates "your token is still in the keychain" from "the
   * settings file could not be written" is the words the backend chose. The
   * two are opposite instructions - one sends the user to their keychain, the
   * other says nothing was changed and to try again - so telling them apart on
   * the prefix is better than saying the alarming one for every failure. A
   * `reasonKey` on the rejection would end this, and is the row's remedy.
   */
  const TOKEN_KEPT = 'the credential store kept the token'

  /**
   * Clear the stored token.
   *
   * The press can genuinely fail, in two ways that read nothing alike. A
   * credential store that refuses to delete leaves the secret in it, and the
   * backend rejects rather than removing the settings key and reporting a
   * deletion that did not happen - the user has to hear that,
   * because the remedy, their keychain, is somewhere this dialog cannot reach.
   * Any other rejection - an unwritable settings file, an adapter that is not
   * answering - is a write that did not happen at all, and telling that user
   * their token is still in a credential store would send them looking for a
   * secret that is not there.
   */
  async function clearToken() {
    tokenFailure = false
    try {
      await getBackend().writeSettings({ hfToken: '' })
    } catch (error) {
      tokenFailure = String(error).includes(TOKEN_KEPT)
        ? 'settings.models.token.clearFailed'
        : 'settings.models.token.clearFailedOther'
      return
    }
    tokenDraft = ''
    await refreshCatalogue()
  }

  /**
   * Which sentence the last Clear earned, or `false` for a press that did not
   * fail. A key rather than a boolean because there are two failures and they
   * are opposite news; `false` because `saveToken` clears it that way and a
   * falsy value is what the template tests.
   *
   * @type {string|false}
   */
  let tokenFailure = $state(/** @type {string|false} */ (false))

  /* ---------- Acceleration ---------- */

  /** @type {import('../api/backend.js').Accelerators|null} */
  let accelerators = $state(null)

  async function refreshAccelerators() {
    try {
      accelerators = await getBackend().listAccelerators()
    } catch {
      accelerators = null
    }
  }

  onMount(refreshAccelerators)

  /**
   * `Automatic`, then every provider the runtime reports - the unusable ones
   * included, disabled, with the reason on them.
   *
   * Shown rather than filtered out for the same reason the cloud rung is shown
   * disabled: a user looking for CUDA and finding no entry at all concludes the
   * application does not support it, where a disabled entry saying "it needs
   * CUDA and cuDNN installed on this machine" is an instruction.
   */
  const acceleratorOptions = $derived([
    { id: 'auto', label: t('settings.accel.auto'), disabled: false, title: undefined },
    ...(accelerators?.providers ?? []).map((provider) => ({
      id: provider.id,
      label: t(provider.labelKey),
      disabled: !provider.available,
      title: provider.reasonKey ? t(provider.reasonKey) : undefined,
    })),
  ])

  /**
   * One line per model: where it will run, and the caveat if there is one.
   *
   * Three things can be true of a row and all three are said: the provider it
   * landed on, whether that was a measurement or a guess (`accel.chosen.*`),
   * and - where a forced provider was refused - which one and why, with the two
   * byte figures beside the sentence rather than inside it, because a byte
   * count is not translatable.
   *
   * @param {{modelKey: string, labelKey: string, noteKey: string|null, declinedKey: string|null, declinedId: string|null, neededBytes: number|null, roomBytes: number|null}} row
   */
  function placementOf(row) {
    const parts = [t(row.labelKey)]
    if (row.noteKey) parts.push(t(row.noteKey))
    if (row.declinedKey) {
      const which = row.declinedId ? t(`accel.${row.declinedId}`) : ''
      const reason = t(row.declinedKey)
      const sizes =
        row.neededBytes !== null && row.roomBytes !== null
          ? ` (${t('models.value.size', { bytes: row.neededBytes })} / ${t('models.value.size', { bytes: row.roomBytes })})`
          : ''
      parts.push(`${which}: ${reason}${sizes}`.trim())
    }
    return parts.join(' · ')
  }

  async function chooseAccelerator(id) {
    setAccelerator(id)
    await push()
    await refreshAccelerators()
  }

  let choosing = $state(false)

  async function browseSidecar() {
    if (choosing) return
    choosing = true
    try {
      const chosen = await chooseFolder({
        title: t('settings.sidecar.chooserTitle'),
        defaultPath: session.sidecarPath || undefined,
      })
      if (chosen !== null) {
        setSidecarPath(chosen)
        await push()
      }
    } finally {
      choosing = false
    }
  }

  const themes = [
    { value: 'light', label: t('settings.theme.light') },
    { value: 'dark', label: t('settings.theme.dark') },
    { value: 'system', label: t('settings.theme.system') },
  ]
  const directions = [
    { value: 'rtl', label: t('settings.direction.rtl') },
    { value: 'ltr', label: t('settings.direction.ltr') },
  ]
  const cloud = [
    { value: 'allowed', label: t('settings.cloud.allowed') },
    { value: 'blocked', label: t('settings.cloud.blocked') },
  ]
  const originalViews = [
    { value: 'hold', label: t('settings.originalView.hold') },
    { value: 'pinned', label: t('settings.originalView.pinned') },
  ]
  /**
   * Which sidecar backend the AI redraw rung asks for.
   *
   * Three options and not two, because `Auto` is the honest default: which
   * runtime is better on a given machine is a question about that machine, the
   * core answers it per platform (`mflux` on Apple Silicon, `sdnq` elsewhere),
   * and hiding that behind a two-way switch would make every Mac user choose
   * between two things they have no way to compare. The two named values are
   * shown on **every** platform rather than filtered by `capabilities`: a
   * control that silently drops the option a user is looking for reads as a
   * missing feature, and an impossible choice is refused with a reason
   * (`decline.reason.sidecarPlatform`) at the moment it is used.
   */
  const fluxBackends = [
    { value: 'auto', label: t('settings.fluxBackend.auto') },
    { value: 'mflux', label: t('settings.fluxBackend.mflux') },
    { value: 'sdnq', label: t('settings.fluxBackend.sdnq') },
  ]

  /**
   * The catalogues that exist, not a wish list. One entry today; the row ships
   * anyway, with the reason in its description, because a Language control that
   * is missing reads as an app with no i18n and a dead dropdown reads as a
   * broken one.
   */
  const languages = Object.keys(CATALOGUES).map((tag) => ({
    value: tag,
    label: t('settings.language.english'),
  }))

  /* ---------- the tab strip ---------- */

  /**
   * The five panels, in the order they are offered.
   *
   * `General` is first because it is what the dialog opens on and what most
   * visits are about; `About` is last because it is a reference rather than a
   * setting. The three between them are ordered by how large a thing they
   * change - what is on the disk, what the engines run on, what the keyboard
   * does.
   *
   * The label keys are the section keys the headings already used, so the tab
   * and the panel's own `h3` are one string and cannot drift apart.
   */
  const TABS = [
    { id: 'general', labelKey: 'settings.section.general' },
    { id: 'models', labelKey: 'settings.section.models' },
    { id: 'acceleration', labelKey: 'settings.section.acceleration' },
    { id: 'shortcuts', labelKey: 'settings.section.shortcuts' },
    { id: 'about', labelKey: 'settings.section.about' },
  ]

  let active = $state('general')

  const uid = $props.id()
  /** @param {string} id */
  const tabId = (id) => `${uid}-tab-${id}`
  /** @param {string} id */
  const panelId = (id) => `${uid}-panel-${id}`

  /** @type {HTMLButtonElement[]} */
  let tabButtons = $state([])

  /**
   * @param {string} id
   * @param {{focus?: boolean}} [options]
   */
  function select(id, { focus = false } = {}) {
    active = id
    if (!focus) return
    // The element identity does not change - the strip is keyed over a static
    // table - so the press can move focus without waiting for a flush.
    tabButtons[TABS.findIndex((tab) => tab.id === id)]?.focus()
  }

  /**
   * The strip's own keys. Selection follows focus, so there is one press per
   * move rather than a move and then a commit.
   *
   * Both stopped as well as prevented: the editor's global arrows page the
   * chapter, and Home / End belong to whatever is under this dialog.
   *
   * @param {KeyboardEvent} event
   */
  function onstripkeydown(event) {
    const index = TABS.findIndex((tab) => tab.id === active)
    let next
    switch (event.key) {
      case 'ArrowRight':
        next = (index + 1) % TABS.length
        break
      case 'ArrowLeft':
        next = (index - 1 + TABS.length) % TABS.length
        break
      case 'Home':
        next = 0
        break
      case 'End':
        next = TABS.length - 1
        break
      default:
        return
    }
    event.preventDefault()
    event.stopPropagation()
    select(TABS[next].id, { focus: true })
  }
</script>

<Modal
  title={t(spec.titleKey)}
  width={modalWidth(spec.kind)}
  onclose={() => closeModal(null)}
>
  <div class="tabs">
    <!-- One tab stop for the whole strip - the selected tab - and the arrows
         move it. The handler is on the tabs rather than on the list, because
         the list is not focusable and an interactive role that takes keys and
         cannot be focused is a control nobody can reach. -->
    <div class="strip" role="tablist" aria-label={t('settings.tabs.label')}>
      {#each TABS as tab, index (tab.id)}
        <button
          bind:this={tabButtons[index]}
          type="button"
          role="tab"
          class="tab"
          class:on={tab.id === active}
          id={tabId(tab.id)}
          aria-selected={tab.id === active}
          aria-controls={panelId(tab.id)}
          tabindex={tab.id === active ? 0 : -1}
          onclick={() => select(tab.id)}
          onkeydown={onstripkeydown}
        >{t(tab.labelKey)}</button>
      {/each}
    </div>

    <div
      class="panel"
      role="tabpanel"
      id={panelId('general')}
      aria-labelledby={tabId('general')}
      hidden={active !== 'general'}
    >
      <h3 class="panel-heading">{t('settings.section.general')}</h3>

      <Field label={t('settings.theme.label')} layout="row">
        {#snippet children({ labelId })}
          <Segmented
            options={themes}
            value={session.theme}
            labelledBy={labelId}
            onchange={(value) => {
              setTheme(/** @type {any} */ (value))
              push()
            }}
          />
        {/snippet}
      </Field>

      <Field
        label={t('settings.direction.label')}
        layout="row"
      >
        {#snippet children({ labelId })}
          <Segmented
            options={directions}
            value={session.readingDirection}
            labelledBy={labelId}
            onchange={(value) => {
              setReadingDirection(/** @type {any} */ (value))
              push()
            }}
          />
        {/snippet}
      </Field>

      <Field
        label={t('settings.cloud.label')}
        description={t('settings.cloud.description')}
        layout="row"
      >
        {#snippet children({ labelId })}
          <Segmented
            options={cloud}
            value={session.cloudAllowed ? 'allowed' : 'blocked'}
            labelledBy={labelId}
            onchange={(value) => {
              setCloudAllowed(value === 'allowed')
              push()
            }}
          />
        {/snippet}
      </Field>

      <Field
        label={t('settings.originalView.label')}
        layout="row"
      >
        {#snippet children({ labelId })}
          <Segmented
            options={originalViews}
            value={session.originalView}
            labelledBy={labelId}
            onchange={(value) => {
              setOriginalView(/** @type {any} */ (value))
              push()
            }}
          />
        {/snippet}
      </Field>

      <Field
        label={t('settings.language.label')}
        layout="row"
      >
        {#snippet children({ labelId })}
          <Segmented
            options={languages}
            value={LOCALE}
            labelledBy={labelId}
            disabled={languages.length < 2}
            onchange={() => {}}
          />
        {/snippet}
      </Field>
    </div>

    <!-- Models. The weights and the ONNX Runtime are downloaded after install;
         until this section existed the only way to get them was a developer's
         shell script. One row per artefact, the runtime beside them because to
         a reader they are one list, and the directory spelled out for the
         offline install as well.

         The three sidecar rows lead it: where an external FLUX install lives,
         which backend it should use, and which of its models. They are the
         same question as the rows below them - what can this machine run - and
         the answer to it is what this tab is. -->
    <div
      class="panel"
      role="tabpanel"
      id={panelId('models')}
      aria-labelledby={tabId('models')}
      hidden={active !== 'models'}
    >
      <h3 class="panel-heading">{t('settings.section.models')}</h3>

      <div class="sidecar-field-wrap">
        <Field
          label={t('settings.sidecar.label')}
          controlId="settings-sidecar-path"
        >
          {#snippet children()}
            <div class="sidecar-row">
              <TextInput
                id="settings-sidecar-path"
                value={session.sidecarPath}
                onchange={(value) => {
                  setSidecarPath(value)
                  push()
                }}
              />
              <Button onclick={browseSidecar} disabled={choosing}>
                {t('shell.action.chooseFolder')}
              </Button>
            </div>
            {#if session.sidecarPath && !capabilities.sidecar}
              <div class="sidecar-status-note">{t('settings.sidecar.notFound')}</div>
            {/if}
          {/snippet}
        </Field>
      </div>

      {#if capabilities.sidecar}
        <div class="sidecar-field-wrap">
          <Field
            label={t('settings.fluxBackend.label')}
            layout="row"
          >
            {#snippet children({ labelId })}
              <Segmented
                options={fluxBackends}
                value={session.fluxBackend}
                labelledBy={labelId}
                onchange={(value) => {
                  setFluxBackend(/** @type {any} */ (value))
                  push()
                }}
              />
            {/snippet}
          </Field>
        </div>

        <div class="sidecar-field-wrap">
          <Field
            label={t('settings.sidecarModel.label')}
            controlId="settings-sidecar-model"
          >
            {#snippet children()}
              <select
                id="settings-sidecar-model"
                class="sidecar-model-select"
                disabled={sidecarModels.length === 0}
                value={session.fluxModel}
                onchange={(e) => {
                  setFluxModel(/** @type {HTMLSelectElement} */ (e.currentTarget).value)
                  push()
                }}
              >
                {#if sidecarModels.length === 0}
                  <option value="">{t('settings.sidecarModel.noneFound')}</option>
                {:else}
                  {#each sidecarModels as model (model.id)}
                    <option value={model.id}>{model.label}</option>
                  {/each}
                {/if}
              </select>
            {/snippet}
          </Field>
        </div>
      {/if}


      {#if catalogue}
        <ul class="rows">
          {#each catalogue.models as model (model.id)}
            {@const status = statusOf(model.id, model.installed, model.sha256Ok)}
            <li class="row">
              <div class="row-text">
                <span class="row-name">{t(model.kindKey)}</span>
                <span class="row-meta">
                  {t('models.value.size', { bytes: model.bytes })} ·
                  {t(status.key, status.params)}{#if model.installed && model.readOnly}
                    · {t('settings.models.status.readOnly')}{/if}
                </span>
                {#if failures[model.id]}
                  <span class="row-error">{failures[model.id]}</span>
                {/if}
                {#if notes[model.id]}
                  <span class="row-error">{t(notes[model.id])}</span>
                {/if}
                <!-- The bytes a stopped download left, which the next
                     Download resumes from. Not shown while one
                     is running: the progress line above is already saying it,
                     and better. -->
                {#if model.partialBytes && !progress[model.id]}
                  <span class="row-partial">
                    {t('settings.models.status.partial', { bytes: model.partialBytes })}
                  </span>
                {/if}
              </div>
              <div class="row-actions">
                {#if progress[model.id]}
                  <Button size="sm" onclick={() => cancel(model.id)}>
                    {t('settings.models.action.cancel')}
                  </Button>
                {:else}
                  {#if model.installed}
                    <Button size="sm" onclick={() => verify(model.id)}>
                      {t('settings.models.action.verify')}
                    </Button>
                    <Button size="sm" disabled={model.readOnly} onclick={() => remove(model.id)}>
                      {t('settings.models.action.delete')}
                    </Button>
                  {:else}
                    <Button size="sm" onclick={() => download(model.id)}>
                      {t('settings.models.action.download')}
                    </Button>
                  {/if}
                  <!-- Offered beside either pair: a `.part` can outlive the
                       weight being installed by hand, and it is still disk
                       nobody asked to spend. -->
                  {#if model.partialBytes}
                    <Button size="sm" onclick={() => discard(model.id)}>
                      {t('settings.models.action.discard')}
                    </Button>
                  {/if}
                {/if}
              </div>
            </li>
          {/each}

          <!-- The runtime. Not a catalogue row - it is an archive that gets
               unpacked - but it does say how large its download is: the sizes
               are on the package table now, read from each host's own
               `Content-Length`. The build reported is the one
               that *would* be fetched, which is the chosen flavour rather than
               whatever is on disk: nothing short of loading the library can
               tell what an installed one is. -->
          <li class="row">
            <div class="row-text">
              <span class="row-name">{t('settings.models.runtime.label')}</span>
              <span class="row-meta">
                {#if catalogue.runtime.available}
                  {#if catalogue.runtime.bytes}
                    {t('models.value.size', { bytes: catalogue.runtime.bytes })} ·
                  {/if}
                  {catalogue.runtime.version} · {catalogue.runtime.flavour} ·
                  {t(statusOf(RUNTIME_ID, catalogue.runtime.installed, null).key,
                    statusOf(RUNTIME_ID, catalogue.runtime.installed, null).params)}{#if catalogue.runtime.installed && catalogue.runtime.readOnly}
                    · {t('settings.models.status.readOnly')}{/if}
                {:else}
                  {t('settings.models.runtime.unavailable')}
                {/if}
              </span>
              {#if failures[RUNTIME_ID]}
                <span class="row-error">{failures[RUNTIME_ID]}</span>
              {/if}
              {#if notes[RUNTIME_ID]}
                <span class="row-error">{t(notes[RUNTIME_ID])}</span>
              {/if}
              <!-- Which build is actually here, said only when it is not the
                   one the row names. -->
              {#if installedNote}
                <span class="row-partial">{installedNote}</span>
              {/if}
              <!-- The runtime's own remainder is its archives, in the download
                   directory it was given. -->
              {#if catalogue.runtime.partialBytes && !progress[RUNTIME_ID]}
                <span class="row-partial">
                  {t('settings.models.status.partial', { bytes: catalogue.runtime.partialBytes })}
                </span>
              {/if}
            </div>
            <div class="row-actions">
              {#if progress[RUNTIME_ID]}
                <Button size="sm" onclick={() => cancel(RUNTIME_ID)}>
                  {t('settings.models.action.cancel')}
                </Button>
              {:else}
                {#if catalogue.runtime.partialBytes}
                  <Button size="sm" onclick={() => discard(RUNTIME_ID)}>
                    {t('settings.models.action.discard')}
                  </Button>
                {/if}
                {#if catalogue.runtime.installed}
                  <Button
                    size="sm"
                    disabled={catalogue.runtime.readOnly}
                    onclick={() => remove(RUNTIME_ID)}
                  >
                    {t('settings.models.action.delete')}
                  </Button>
                {/if}
                <!-- Offered over an installed runtime **only where there is a
                     choice**: fetching the chosen build over the top of the one
                     that is there is the only way to switch, and on a platform
                     with one build it would be a button that re-downloads what
                     the machine already has. -->
                {#if !catalogue.runtime.installed || catalogue.runtime.flavours.length > 1}
                  <Button
                    size="sm"
                    disabled={!catalogue.runtime.available}
                    onclick={() => download(RUNTIME_ID)}
                  >
                    {t('settings.models.action.download')}
                  </Button>
                {/if}
              {/if}
            </div>
          </li>
        </ul>

        <!-- The flavour picker. Drawn only where the platform
             publishes more than one build, which is Windows and Linux x64;
             macOS and Linux aarch64 have one archive each and a select with
             one option in it is a control that only asks a question it has
             already answered. -->
        {#if catalogue.runtime.flavours.length > 1}
          <div class="flavour">
            <Field
              label={t('settings.models.runtime.flavour')}
              controlId="settings-runtime-flavour"
            >
              {#snippet children()}
                <select
                  id="settings-runtime-flavour"
                  class="sidecar-model-select"
                  value={catalogue.runtime.flavour}
                  onchange={(e) =>
                    chooseFlavour(/** @type {HTMLSelectElement} */ (e.currentTarget).value)}
                >
                  {#each catalogue.runtime.flavours as build (build.id)}
                    <option value={build.id}>
                      {build.id} · {build.ortVersion} ·
                      {t('models.value.size', { bytes: build.bytes })}
                    </option>
                  {/each}
                </select>
              {/snippet}
            </Field>
            {#if flavourNeeds}
              <p class="note flavour-note">{flavourNeeds}</p>
            {/if}
          </div>
        {/if}

        {#if catalogue.modelsDir}
          <p class="path">{t('settings.models.folder', { path: catalogue.modelsDir })}</p>
        {/if}
      {:else}
        <p class="note">{t('settings.models.unavailable')}</p>
      {/if}

      <!-- Four of the six weights are on Hugging Face, which rate-limits
           anonymous downloads. The field is write-only: the stored token never
           comes back across the seam, so this box is empty on every open and
           "a token is saved" is said in words beside it. -->
      <div class="token">
        <Field
          label={t('settings.models.token.label')}
          controlId="settings-hf-token"
        >
          {#snippet children()}
            <div class="token-row">
              <TextInput
                id="settings-hf-token"
                type="password"
                value={tokenDraft}
                placeholder={t('settings.models.token.placeholder')}
                onchange={(value) => (tokenDraft = value)}
              />
              <Button disabled={tokenDraft.trim().length === 0} onclick={saveToken}>
                {t('settings.models.token.save')}
              </Button>
              <Button disabled={!catalogue?.hasToken} onclick={clearToken}>
                {t('settings.models.token.clear')}
              </Button>
            </div>
            {#if tokenFailure}
              <div class="token-note failed">{t(tokenFailure)}</div>
            {:else if tokenNote}
              <div class="token-note">{tokenNote}</div>
              <!-- And what the store did, where it said. A
                   second line rather than a fourth variant of the first: the
                   two answer different questions, and this one reads the same
                   whether or not a token has been saved yet. -->
              {#if tokenReason}
                <div class="token-note">{tokenReason}</div>
              {/if}
            {/if}
          {/snippet}
        </Field>
      </div>
    </div>

    <!-- Acceleration. `listAccelerators` has been registered, mirrored in the
         mock and typed at the seam, and nothing called it: the only
         way to force a provider was to edit settings.json by hand. -->
    <!-- The panel's tail is the placement table and a note, neither of which
         holds a focusable descendant, so the scroller carries the tab stop. -->
    <!-- `tabindex` because this panel's **tail** holds no focusable
         descendant, so without it its last inch is pointer-only (WCAG 2.1.1).
         Only this panel and About are like that; the other three each end in a
         control, so tabbing already reaches their bottom. -->
    <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
    <div
      class="panel"
      role="tabpanel"
      tabindex="0"
      id={panelId('acceleration')}
      aria-labelledby={tabId('acceleration')}
      hidden={active !== 'acceleration'}
    >
      <h3 class="panel-heading">{t('settings.section.acceleration')}</h3>

      <Field label={t('settings.accel.label')} controlId="settings-accelerator">
        {#snippet children()}
          <select
            id="settings-accelerator"
            class="sidecar-model-select"
            value={session.accelerator}
            onchange={(e) =>
              chooseAccelerator(/** @type {HTMLSelectElement} */ (e.currentTarget).value)}
          >
            {#each acceleratorOptions as option (option.id)}
              <option value={option.id} disabled={option.disabled} title={option.title}>
                {option.label}{option.disabled && option.title ? `: ${option.title}` : ''}
              </option>
            {/each}
          </select>
        {/snippet}
      </Field>

      {#if accelerators && accelerators.models.length > 0}
        <ul class="rows">
          {#each accelerators.models as row (row.modelKey)}
            <li class="row">
              <div class="row-text">
                <span class="row-name">{t(row.modelKey)}</span>
                <span class="row-meta">{placementOf(row)}</span>
              </div>
            </li>
          {/each}
        </ul>
      {/if}
    </div>

    <div
      class="panel"
      role="tabpanel"
      id={panelId('shortcuts')}
      aria-labelledby={tabId('shortcuts')}
      hidden={active !== 'shortcuts'}
    >
      <h3 class="panel-heading">{t('settings.section.shortcuts')}</h3>
      <ShortcutSheet headingLevel="h4" />
    </div>

    <!-- About ends in the written offer of source and the cloud terms, so its
         scroller carries the tab stop as well. It is a GPL-3.0 obligation and
         this dialog is its only route. -->
    <!-- Focusable for the same reason the Acceleration panel is: it ends in
         prose rather than in a control. -->
    <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
    <div
      class="panel"
      role="tabpanel"
      tabindex="0"
      id={panelId('about')}
      aria-labelledby={tabId('about')}
      hidden={active !== 'about'}
    >
      <h3 class="panel-heading">{t('settings.section.about')}</h3>
      <AboutSection />
    </div>
  </div>


  {#snippet buttons()}
    <Button variant="primary" onclick={() => closeModal('done')}>{t('shell.action.done')}</Button>
  {/snippet}
</Modal>

<style>
  /* The strip and the panels run to the dialog's own edges rather than to the
     text column's, so the strip's hairline reads as a divider across the head
     and a panel's scrollbar sits where the modal's own used to. The `--s-6`
     the modal body pads with is given back inside each of them. */
  .tabs {
    margin: 6px calc(var(--s-6) * -1) 0;
  }

  .strip {
    display: flex;
    gap: var(--s-5);
    padding: 0 var(--s-6);
    border-bottom: 1px solid var(--line);
  }

  /* Weight does not change with selection: a bold label is wider than the same
     word in regular, and the four tabs beside the selected one would step
     sideways on every press. Colour and the rule under it carry the state.
     `--t2` at rest rather than the `--t3` a static label would take, for the
     reason `Segmented` gives about its idle chips - this is a control. */
  .tab {
    position: relative;
    padding: 0 0 var(--s-3);
    border: none;
    background: none;
    font: inherit;
    font-size: 11.5px;
    color: var(--t2);
    white-space: nowrap;
    cursor: pointer;
    transition: color var(--dur-fast) var(--ease);
  }
  .tab:hover { color: var(--text) }
  .tab.on { color: var(--text) }
  .tab.on::after {
    content: '';
    position: absolute;
    left: 0;
    right: 0;
    bottom: -1px;
    height: 2px;
    border-radius: var(--r-pill);
    background: var(--accent);
  }
  .tab:focus-visible { outline-offset: -1px }

  /* One fixed height for every panel, so the dialog does not resize under the
     pointer as tabs swap. Capped in pixels so a tall screen does not get a
     dialog it has to look up and down, proportional below the cap so a short
     one never needs the modal's own scrollbar underneath this one. */
  .panel {
    height: min(52vh, 460px);
    padding: var(--s-4) var(--s-6) var(--s-3);
    overflow-y: auto;
    overscroll-behavior: contain;
    scrollbar-gutter: stable;
  }
  .panel:focus-visible { outline-offset: -2px }
  .panel[hidden] { display: none }

  /* The selected tab is this heading on screen; printing the word again an
     inch below it would be an echo, not a heading. It stays in the markup so
     the outline is real - h2 title, h3 panel, h4 shortcut groups - and so the
     sheet's own headings have something to hang from. */
  .panel-heading {
    position: absolute;
    width: 1px;
    height: 1px;
    margin: -1px;
    padding: 0;
    overflow: hidden;
    clip-path: inset(50%);
    white-space: nowrap;
  }

  /* The Models panel opens on a sentence rather than on a row, so it needs the
     gap under it that a `Field` brings with it. */

  .sidecar-field-wrap {
    padding-top: var(--s-3);
    padding-bottom: var(--s-2);
    border-bottom: 1px solid var(--line);
  }
  .sidecar-row {
    display: flex;
    align-items: center;
    gap: var(--s-2);
    width: 100%;
  }
  .sidecar-row :global(> *:first-child) {
    flex: 1;
    min-width: 0;
  }
  /* The Models and Acceleration lists. One row is a name and a meta line on
     the left and its buttons on the right; the meta line is `--t3` like every
     other secondary line in this dialog, and the whole row keeps the hairline
     the sidecar fields already use so the section reads as one table. */
  .rows {
    margin: var(--s-2) 0 0;
    padding: 0;
    list-style: none;
  }
  .row {
    display: flex;
    align-items: center;
    gap: var(--s-3);
    padding: var(--s-2) 0;
    border-bottom: 1px solid var(--line);
  }
  .row-text {
    display: flex;
    flex-direction: column;
    gap: 2px;
    flex: 1;
    min-width: 0;
  }
  .row-name {
    font-size: 12px;
    color: var(--text);
  }
  .row-meta {
    font-size: 10.5px;
    color: var(--t3);
    line-height: 1.4;
  }
  /* `--t2` rather than `--t3`: a failure is the one line in this section the
     reader has to act on, and `--t3` is under 4.5:1 against `--panel` in dark. */
  .row-error {
    font-size: 10.5px;
    color: var(--t2);
    line-height: 1.4;
    word-break: break-word;
  }
  /* The kept bytes, and the build that is here rather than the one chosen.
     Neither is a failure and neither is the row's own subject, so they take
     `--t3` with the meta line rather than the `--t2` a failure gets. */
  .row-partial {
    font-size: 10.5px;
    color: var(--t3);
    line-height: 1.4;
  }
  .row-actions {
    display: flex;
    flex: none;
    gap: var(--s-2);
  }
  .note {
    margin: 0 0 var(--s-2);
    font-size: 10.5px;
    color: var(--t3);
    line-height: 1.45;
  }
  /* A path is data, not copy: it must be selectable and must not be broken by
     the interface's own word wrapping in a way that makes it untypable. */
  .path {
    margin: var(--s-2) 0 0;
    font-size: 10.5px;
    color: var(--t3);
    line-height: 1.4;
    word-break: break-all;
    user-select: text;
  }
  .flavour { padding-top: var(--s-3) }
  .flavour-note { margin: var(--s-2) 0 0 }
  .token { padding-top: var(--s-3) }
  .token-row {
    display: flex;
    align-items: center;
    gap: var(--s-2);
    width: 100%;
  }
  .token-row :global(> *:first-child) {
    flex: 1;
    min-width: 0;
  }
    /* The one token note that is a failure the user has to act on. */
  .token-note.failed { color: var(--warn) }
  .token-note {
    margin-top: var(--s-2);
    font-size: 10.5px;
    color: var(--t3);
    line-height: 1.4;
  }

  .sidecar-status-note {
    margin-top: var(--s-2);
    font-size: 10.5px;
    color: var(--t3);
    line-height: 1.4;
  }
  .sidecar-model-select {
    width: 100%;
    height: 28px;
    padding: 0 var(--s-2);
    border: 1px solid var(--line2);
    border-radius: var(--r-md);
    background: var(--panel);
    color: var(--text);
    font: inherit;
    font-size: 11.5px;
    cursor: pointer;
    transition:
      background var(--dur-fast) var(--ease),
      border-color var(--dur-fast) var(--ease);
  }
  .sidecar-model-select:hover:not(:disabled) {
    border-color: var(--accent);
  }
  .sidecar-model-select:focus-visible {
    outline: none;
    border-color: var(--accent);
  }
  .sidecar-model-select:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

</style>
