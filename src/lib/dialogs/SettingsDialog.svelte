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
   * Settings, as a full-window screen over the route underneath.
   *
   * It is still the `settings` modal kind, so every opener pushes the same
   * spec and the editor stays mounted behind it. `Screen` owns the layer
   * (focus in and back out, the Tab trap, Escape); this file owns the layout:
   * a sidebar with the way back and a vertical tab list, and one scrolling
   * column per section.
   *
   * **Seven sections, in the order a visit needs them.** General holds the
   * preferences and the download token. Detection and Cleaning are the two
   * pipelines a page goes through, each with its engines and the files those
   * engines need, so a file is managed beside the choice that made it
   * necessary; the FLUX helper is Cleaning's. Cloud is `InferenceSettings`,
   * unchanged. Performance is what the engines run on: the runtime, its build,
   * where downloads go, and the accelerator. Shortcuts and About close the
   * list; About is a GPL-3.0 obligation and this screen is its only route.
   *
   * **Every panel is mounted, and only the selected one is shown.** `hidden`
   * rather than `{#if}`, so `aria-controls` names an element that exists, a
   * panel keeps its scroll position while another is looked at, and the
   * mount-time work is one `listModels` and one `about` however many panels
   * are visited.
   *
   * **The tab list is a real one.** `role="tablist"` with
   * `aria-orientation="vertical"`, one tab stop, Up and Down to move, Home and
   * End to the ends. Selection follows focus, which is safe because every
   * panel is already mounted. The outline is h1 (the screen), h2 (the
   * section), h3 below it.
   *
   * **A panel is focusable when, and only when, its tail is not** (WCAG
   * 2.1.1). Performance ends in the placement list and About in prose, so both
   * scrollers take a tab stop. Detection does too while it has no file rows,
   * when it ends in the engine table.
   *
   * **The backend reconciliation is not here.** `session.*` and
   * `backend.readSettings()` are reconciled once at boot, in `App.svelte`.
   * What this screen owns is the second half: every change pushes the
   * session's values down to the backend immediately.
   *
   * **The shortcut section is not a read-only list.** `ShortcutSheet` is where
   * a binding is *changed*, and it writes its own half of the settings, so it
   * behaves identically here and mounted on its own by `?`.
   */
  import { Button, Field, Screen, Segmented, Select, TextInput, ThemePicker } from '../ui/index.js'
  import Icon from '../icons/Icon.svelte'
  import { onMount, untrack } from 'svelte'
  import { closeModal } from '../state/app.svelte.js'
  import { getBackend } from '../api/backend.js'
  import { chooseFolder } from '../api/folder.js'
  import { CATALOGUES, LOCALE, hasKey, t } from '../i18n/index.js'
  import { capabilities, loadCapabilities } from '../state/capabilities.svelte.js'
  import {
    THEMES,
    THEME_LABEL_KEYS,
    backendSettingsPatch,
    session,
    setAccelerator,
    setCloseToTray,
    setDetection,
    setFluxBackend,
    setFluxModel,
    setOriginalView,
    setReadingDirection,
    setSidecarPath,
    setTheme,
  } from '../state/session.svelte.js'
  import { CLEANERS, DETECTORS, LANGUAGES, detectorsFor, engineBytes } from '../model/pipelines.js'
  import EngineTable from './EngineTable.svelte'
  import ShortcutSheet from './ShortcutSheet.svelte'
  import AboutSection from './AboutSection.svelte'
  import InferenceSettings from './InferenceSettings.svelte'
  import { offerFirstLaunch } from './firstlaunch.svelte.js'

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
          // Written straight away, like every other change here. Not `push()`:
          // that calls back into this function.
          await getBackend().writeSettings(backendSettingsPatch())
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

  let backgroundError = $state(false)

  async function updateCloseToTray(input) {
    const enabled = input.checked
    backgroundError = false
    try {
      await getBackend().writeSettings({ closeToTray: enabled })
      setCloseToTray(enabled)
    } catch {
      backgroundError = true
      input.checked = session.closeToTray
    }
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

  let replayError = $state(false)
  let replaying = $state(false)

  /**
   * "Run setup again": the offer a first launch makes, over a fresh catalogue
   * so the download step shows what is here now. Settings closes first,
   * because the setup is drawn beside the modal stack and only while the
   * stack is empty (`App.svelte`) - but only once the offer has been made, so
   * a failure is said here, on a screen that is still open.
   */
  async function replayOnboarding() {
    if (replaying) return
    replaying = true
    replayError = false
    let offered = false
    try {
      const view = await getBackend().listModels()
      offered = offerFirstLaunch(view, { force: true })
      if (!offered) replayError = true
    } catch {
      replayError = true
    } finally {
      replaying = false
    }
    if (offered) closeModal(null)
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
   * What a refused row actually says.
   *
   * Two shapes arrive here as one string. A refusal the backend has a sentence
   * for answers as a catalogue key with its figures beside it, in the grammar
   * `key name=value ...` with decimal values - `notice.runtime.noSpace
   * needed=253000000 free=1200000`, and `notice.runtime.inUse` with no figures
   * at all. Everything else answers with whatever the failure itself said,
   * which is what this row showed for every failure before the two Windows
   * refusals existed. The byte counts travel as data rather than inside the
   * sentence because a byte count is not translatable; `{needed:memory}` in the
   * catalogue is what turns them into something a person reads.
   *
   * Only `notice.runtime.*` is honoured, and deliberately: a rejection free to
   * name any key in the catalogue would be the backend choosing what the
   * interface says. It is the same restriction `reportRegionEditFailure`
   * applies to `decline.reason.*` for a region edit, for the same reason.
   *
   * @param {string} id
   * @returns {string}
   */
  function failureText(id) {
    const message = String(failures[id] ?? '')
    if (!message) return ''
    const [key, ...pairs] = message.split(' ')
    if (!key.startsWith('notice.runtime.') || !hasKey(key)) return message
    /** @type {Record<string, number>} */
    const params = {}
    for (const pair of pairs) {
      const at = pair.indexOf('=')
      // A malformed pair is dropped rather than shown: the sentence it belongs
      // to renders without that figure, which is a worse sentence and not a
      // wrong one.
      if (at <= 0) continue
      const value = Number(pair.slice(at + 1))
      if (Number.isFinite(value)) params[pair.slice(0, at)] = value
    }
    return t(key, params)
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

  /**
   * Whether the last `listAccelerators` was **rejected**, as against not having
   * been asked for yet. `accelerators === null` cannot tell those two apart,
   * and the difference is the whole of what this panel has to say: a list not
   * yet asked for draws nothing, a list that was refused has to explain why
   * Automatic is the only thing on offer.
   */
  let accelFailure = $state(false)

  async function refreshAccelerators() {
    try {
      accelerators = await getBackend().listAccelerators()
      accelFailure = false
    } catch (error) {
      // Caught and dropped until now, which on the machine this matters most on
      // said nothing at all: a Windows install whose ONNX Runtime will not load
      // - the missing Microsoft redistributable - answers nothing here, and the
      // picker collapsed to a bare Automatic with no explanation beside it. The
      // *reason* is the runtime's own and is reported on the runtime's row; the
      // panel's share is that the list is missing and where the reason is.
      console.error('listAccelerators was rejected', error)
      accelerators = null
      accelFailure = true
    }
  }

  /**
   * Asked for when the Performance panel is shown, not when the screen
   * mounts.
   *
   * The list is the loaded runtime answering, so asking for it maps the
   * runtime's library into this process - and Windows will not replace a file
   * that is mapped, which is exactly what the runtime row's Download has to
   * do. Opening Settings to fetch a runtime must not be the thing that makes
   * the fetch impossible, so the question waits until the panel that shows the
   * answer is actually looked at.
   *
   * Asked once, unless it was refused: a refusal is asked again the next time
   * the panel is shown, which is after a runtime download has had its chance
   * to put the cause right. Only `active` is tracked, so a refusal does not
   * retry itself in a loop.
   */
  let accelAsked = false
  $effect(() => {
    if (active !== 'performance') return
    untrack(() => {
      if (accelAsked && !accelFailure) return
      accelAsked = true
      refreshAccelerators()
    })
  })

  /**
   * Whether `Automatic` is itself a guess.
   *
   * It is, when the setting in force puts models on providers and not one of
   * those has ever been timed on this kind of machine - which is every Windows
   * GPU today. `active` is the backend saying which providers the current
   * setting actually uses, and this is what it is for.
   *
   * Asked **only while Automatic is the setting in force**, which is what the
   * absence of a `selected` provider means. `active` describes the current
   * placement, so under a forced provider it says nothing about what Automatic
   * would have done, and reading it there would put a caveat on a choice
   * nobody had made. An empty list says nothing either: no rows is not
   * evidence of anything.
   */
  const autoUnmeasured = $derived.by(() => {
    const providers = accelerators?.providers ?? []
    if (providers.some((provider) => provider.selected)) return false
    const inUse = providers.filter((provider) => provider.active)
    return inUse.length > 0 && inUse.every((provider) => !provider.measured)
  })

  /**
   * Which option the picker shows as the current one.
   *
   * `selected` is the backend's own reading of the stored preference, so once
   * the list has been read it is the better answer than the session copy - the
   * two differ whenever a stored value was not one the backend kept. Before the
   * list has been read there is nothing to compare against, and the session's
   * value is all there is.
   */
  const acceleratorValue = $derived(
    accelerators
      ? (accelerators.providers.find((provider) => provider.selected)?.id ?? 'auto')
      : session.accelerator,
  )

  /**
   * `Automatic`, then every provider the runtime reports - the unusable ones
   * included, disabled, with the reason on them.
   *
   * Shown rather than filtered out for the same reason the cloud rung is shown
   * disabled: a user looking for CUDA and finding no entry at all concludes the
   * application does not support it, where a disabled entry saying "it needs
   * CUDA and cuDNN installed on this machine" is an instruction.
   *
   * `note` is one field for two different things, because the option has one
   * line to say either in: why a provider cannot be picked, or - for one that
   * can - that picking it rests on how the provider works rather than on a
   * timing taken here. The second half is `measured`, which crossed the wire
   * from the start and was read by nothing, so every Windows choice was offered
   * as though it had been measured.
   */
  const acceleratorOptions = $derived([
    {
      id: 'auto',
      label: t('settings.accel.auto'),
      disabled: false,
      note: autoUnmeasured ? t('accel.chosen.unmeasured') : undefined,
    },
    ...(accelerators?.providers ?? []).map((provider) => ({
      id: provider.id,
      label: t(provider.labelKey),
      disabled: !provider.available,
      note: !provider.available
        ? provider.reasonKey
          ? t(provider.reasonKey)
          : undefined
        : provider.measured
          ? undefined
          : t('accel.chosen.unmeasured'),
    })),
  ])

  /**
   * The options the picker draws. A stored id the list does not offer - a
   * provider this runtime no longer reports, or any id before the list has
   * been read - is drawn as itself rather than left out: a native select with
   * no matching option shows its first one, and the picker would read
   * Automatic while the setting in force was something else.
   */
  const acceleratorChoices = $derived.by(() => {
    const rows = acceleratorOptions.map((option) => ({
      value: option.id,
      label: option.note ? `${option.label}: ${option.note}` : option.label,
      disabled: option.disabled,
      title: option.note,
    }))
    if (rows.some((row) => row.value === acceleratorValue)) return rows
    return [...rows, { value: acceleratorValue, label: t('settings.accel.saved', { id: acceleratorValue }) }]
  })

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

  /**
   * What the folder field holds while it is being typed into, or `null` when
   * it is showing the stored path. The path is committed on blur and on
   * Enter, not per keystroke: each commit is a settings write, a capability
   * probe and a model listing, and a half-typed path is none of those.
   *
   * @type {string|null}
   */
  let sidecarDraft = $state(null)

  function commitSidecar() {
    if (sidecarDraft === null) return
    const next = sidecarDraft
    sidecarDraft = null
    if (next === session.sidecarPath) return
    setSidecarPath(next)
    push()
  }

  async function browseSidecar() {
    if (choosing) return
    choosing = true
    try {
      const chosen = await chooseFolder({
        title: t('settings.sidecar.chooserTitle'),
        defaultPath: session.sidecarPath || undefined,
      })
      if (chosen !== null) {
        sidecarDraft = null
        setSidecarPath(chosen)
        await push()
      }
    } finally {
      choosing = false
    }
  }

  /**
   * The helper's models as options. A stored model the helper does not list
   * is drawn as itself, for the reason `acceleratorChoices` gives; an empty
   * list with nothing stored is one disabled line saying so.
   */
  const sidecarChoices = $derived.by(() => {
    const rows = sidecarModels.map((model) => ({ value: model.id, label: model.label }))
    if (session.fluxModel && !rows.some((row) => row.value === session.fluxModel)) {
      rows.push({ value: session.fluxModel, label: t('settings.sidecarModel.missing', { id: session.fluxModel }) })
    }
    return rows.length > 0 ? rows : [{ value: '', label: t('settings.sidecarModel.noneFound') }]
  })

  const themes = THEMES.map((value) => ({ value, label: t(THEME_LABEL_KEYS[value]) }))
  const directions = [
    { value: 'rtl', label: t('settings.direction.rtl') },
    { value: 'ltr', label: t('settings.direction.ltr') },
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


  /* ---------- the two pipelines ---------- */

  /** What a language row stores for "skip this language". */
  const SKIP = ''

  /** @param {string} language */
  function detectorOptions(language) {
    return [
      ...detectorsFor(language).map((engine) => ({ value: engine.id, label: engine.name })),
      { value: SKIP, label: t('pipelines.skip') },
    ]
  }

  /**
   * Which catalogue rows belong to Cleaning: the files a cleaner names. Every
   * other row is Detection's, so a weight added to the backend before this
   * table knows it still has a place to be managed from.
   */
  const CLEANING_FILES = new Set(CLEANERS.flatMap((engine) => engine.files))
  const detectionModels = $derived(catalogue?.models.filter((model) => !CLEANING_FILES.has(model.id)) ?? [])
  const cleaningModels = $derived(catalogue?.models.filter((model) => CLEANING_FILES.has(model.id)) ?? [])

  /** The catalogue by id, in the shape `engineBytes` reads. */
  const filesById = $derived(Object.fromEntries((catalogue?.models ?? []).map((model) => [model.id, model])))

  /**
   * Whether the FLUX helper lists this engine's model. Such an engine runs
   * through the helper and has nothing to download here.
   *
   * @param {import('../model/pipelines.js').Engine} engine
   */
  function found(engine) {
    return Boolean(engine.sidecar && sidecarModels.some((model) => model.id === engine.sidecar))
  }

  /**
   * The last column of an engine row: what it still costs, or that it is
   * here. Blank until the catalogue answers, rather than a guess.
   *
   * @param {import('../model/pipelines.js').Engine} engine
   */
  function engineState(engine) {
    if (!engine.ready) {
      if (found(engine)) return t('pipelines.status.found')
      return engine.sidecar ? t('pipelines.status.needsHelper') : t('pipelines.status.soon')
    }
    if (!catalogue) return ''
    const bytes = engineBytes(engine, filesById)
    return bytes > 0 ? t('models.value.size', { bytes }) : t('pipelines.status.installed')
  }

  /* ---------- the tab list ---------- */

  /**
   * The sections, in the order they are offered. The Cloud section keeps the
   * id `inference`, which `openCloudSettings` asks for by name.
   */
  const TABS = [
    { id: 'general', labelKey: 'settings.section.general', icon: 'sliders' },
    { id: 'detection', labelKey: 'pipelines.detection', icon: 'search' },
    { id: 'cleaning', labelKey: 'pipelines.cleaning', icon: 'brush' },
    { id: 'inference', labelKey: 'settings.section.inference', icon: 'cloud' },
    { id: 'performance', labelKey: 'settings.section.performance', icon: 'cpu' },
    { id: 'shortcuts', labelKey: 'settings.section.shortcuts', icon: 'keyboard' },
    { id: 'about', labelKey: 'settings.section.about', icon: 'info' },
  ]

  /**
   * Ids a caller may still ask for from before the split: Models became
   * Detection (with Cleaning beside it), Acceleration became Performance.
   */
  const RENAMED_TABS = /** @type {Record<string, string>} */ ({
    models: 'detection',
    acceleration: 'performance',
  })

  /** @param {unknown} requested */
  function initialTab(requested) {
    const id = typeof requested === 'string' ? (RENAMED_TABS[requested] ?? requested) : ''
    return TABS.some((tab) => tab.id === id) ? id : 'general'
  }

  // A caller may open Settings on a section (`openCloudSettings` asks for
  // Cloud). Read once: after that the list owns it.
  let active = $state(untrack(() => initialTab(spec?.props?.tab)))

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
    // The element identity does not change - the list is keyed over a static
    // table - so the press can move focus without waiting for a flush.
    if (focus) tabButtons[TABS.findIndex((tab) => tab.id === id)]?.focus()
  }

  /**
   * Up and Down move and select, Home and End go to the ends. Stopped as well
   * as prevented: the editor underneath pages the chapter on the arrows.
   *
   * @param {KeyboardEvent} event
   */
  function onlistkeydown(event) {
    const index = TABS.findIndex((tab) => tab.id === active)
    let next
    switch (event.key) {
      case 'ArrowDown':
        next = (index + 1) % TABS.length
        break
      case 'ArrowUp':
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

<!-- One catalogue row: a weight's name, size and state, with the presses that
     apply to it. Detection and Cleaning both draw their files through this. -->
{#snippet fileRow(/** @type {any} */ model)}
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
        <span class="row-error">{failureText(model.id)}</span>
      {/if}
      {#if notes[model.id]}
        <span class="row-error">{t(notes[model.id])}</span>
      {/if}
      <!-- The bytes a stopped download left. Not shown while one runs: the
           progress in the meta line already says it. -->
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
        <!-- Beside either pair: a `.part` can outlive a weight installed by
             hand, and it is still disk nobody asked to spend. -->
        {#if model.partialBytes}
          <Button size="sm" onclick={() => discard(model.id)}>
            {t('settings.models.action.discard')}
          </Button>
        {/if}
      {/if}
    </div>
  </li>
{/snippet}

{#snippet fileList(/** @type {any[]} */ models)}
  {#if catalogue}
    {#if models.length > 0}
      <h3 class="sub">{t('settings.models.heading')}</h3>
      <ul class="rows">
        {#each models as model (model.id)}
          {@render fileRow(model)}
        {/each}
      </ul>
    {/if}
  {:else}
    <p class="note">{t('settings.models.unavailable')}</p>
  {/if}
{/snippet}

<Screen label={t(spec.titleKey)} onclose={() => closeModal(null)}>
  <div class="settings">
    <nav class="side" aria-labelledby="{uid}-title">
      <div class="side-head">
        <button
          type="button"
          class="back"
          aria-label={t('shell.action.done')}
          title={t('shell.action.done')}
          onclick={() => closeModal('done')}
        ><Icon name="chevron-left" size={16} /></button>
        <h1 id="{uid}-title">{t(spec.titleKey)}</h1>
      </div>

      <!-- One tab stop for the whole list: the selected tab. The handler is on
           the tabs because the list itself is not focusable. -->
      <div class="tabs" role="tablist" aria-orientation="vertical" aria-label={t('settings.tabs.label')}>
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
            title={t(tab.labelKey)}
            onclick={() => select(tab.id)}
            onkeydown={onlistkeydown}
          >
            <Icon name={tab.icon} size={16} />
            <span class="tab-label">{t(tab.labelKey)}</span>
          </button>
        {/each}
      </div>
    </nav>

    <!-- General -->
    <div
      class="panel"
      role="tabpanel"
      id={panelId('general')}
      aria-labelledby={tabId('general')}
      hidden={active !== 'general'}
    >
      <div class="column">
        <h2>{t('settings.section.general')}</h2>

        <div class="block theme">
          <Field label={t('settings.theme.label')}>
            {#snippet children({ labelId })}
              <ThemePicker
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
        </div>

        <Field
          label={t('settings.background.label')}
          description={t('settings.background.description')}
          layout="row"
          controlId="settings-close-to-tray"
        >
          {#snippet children({ descriptionId })}
            <input
              id="settings-close-to-tray"
              class="check"
              type="checkbox"
              aria-describedby={descriptionId}
              checked={session.closeToTray}
              onchange={(event) => updateCloseToTray(/** @type {HTMLInputElement} */ (event.currentTarget))}
            />
          {/snippet}
        </Field>
        {#if backgroundError}<p class="error" role="alert">{t('settings.background.saveFailed')}</p>{/if}

        <Field label={t('settings.direction.label')} layout="row">
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

        <!-- The cloud permission has one switch, on the Cloud section beside
             the endpoints it governs. This row says where it stands and goes
             there. -->
        <Field
          label={t('settings.cloud.label')}
          description={session.cloudAllowed ? t('settings.cloud.descriptionOn') : t('settings.cloud.descriptionOff')}
          layout="row"
        >
          {#snippet children()}
            <Button size="sm" onclick={() => select('inference', { focus: true })}>{t('settings.cloud.open')}</Button>
          {/snippet}
        </Field>

        <Field label={t('settings.originalView.label')} layout="row">
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

        <Field label={t('settings.language.label')} layout="row">
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

        <!-- Where someone who skipped part of the setup goes looking for it. -->
        <Field
          label={t('onboarding.replay.label')}
          description={t('onboarding.replay.description')}
          layout="row"
        >
          {#snippet children({ descriptionId })}
            <Button size="sm" onclick={replayOnboarding} disabled={replaying} aria-describedby={descriptionId}>
              {t('onboarding.replay.action')}
            </Button>
          {/snippet}
        </Field>
        {#if replayError}<p class="error" role="alert">{t('onboarding.replay.failed')}</p>{/if}

        <!-- Both pipelines download from Hugging Face, so the token is
             General's rather than either one's. Write-only: the stored token
             never comes back across the seam, so the box is empty on every
             open and "a token is saved" is said in words under it. -->
        <div class="block token">
          <Field label={t('settings.models.token.label')} controlId="settings-hf-token">
            {#snippet children()}
              <div class="inline">
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
                <p class="line failed">{t(tokenFailure)}</p>
              {:else if tokenNote}
                <p class="line">{tokenNote}</p>
                {#if tokenReason}
                  <p class="line">{tokenReason}</p>
                {/if}
              {/if}
            {/snippet}
          </Field>
        </div>
      </div>
    </div>

    <!-- Detection. With no file rows it ends in the engine table, which holds
         nothing focusable, so the scroller carries the tab stop. -->
    <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
    <div
      class="panel"
      role="tabpanel"
      tabindex={detectionModels.length > 0 ? undefined : 0}
      id={panelId('detection')}
      aria-labelledby={tabId('detection')}
      hidden={active !== 'detection'}
    >
      <div class="column">
        <h2>{t('pipelines.detection')}</h2>

        <div class="languages">
          {#each LANGUAGES as language (language.id)}
            <Field label={t(language.labelKey)} layout="row" controlId="settings-detector-{language.id}">
              {#snippet children()}
                <div class="pick">
                  <Select
                    id="settings-detector-{language.id}"
                    options={detectorOptions(language.id)}
                    value={session.detection[language.id] ?? SKIP}
                    label={t('pipelines.detectorFor', { language: t(language.labelKey) })}
                    onchange={(value) => setDetection(language.id, value || null)}
                  />
                </div>
              {/snippet}
            </Field>
          {/each}
        </div>

        <EngineTable engines={DETECTORS} label={t('pipelines.detection')} stateOf={engineState} />

        {@render fileList(detectionModels)}
      </div>
    </div>

    <!-- Cleaning -->
    <div
      class="panel"
      role="tabpanel"
      id={panelId('cleaning')}
      aria-labelledby={tabId('cleaning')}
      hidden={active !== 'cleaning'}
    >
      <div class="column">
        <h2>{t('pipelines.cleaning')}</h2>

        <EngineTable
          engines={CLEANERS}
          label={t('pipelines.cleaning')}
          stateOf={engineState}
          isAvailable={(engine) => engine.ready || found(engine)}
        />

        {@render fileList(cleaningModels)}

        <!-- An external FLUX install: where it lives, which backend runs it,
             and which of its models. -->
        <h3 class="sub">{t('settings.sidecar.heading')}</h3>
        <div class="block flush">
          <Field label={t('settings.sidecar.label')} controlId="settings-sidecar-path">
            {#snippet children()}
              <div class="inline">
                <TextInput
                  id="settings-sidecar-path"
                  value={sidecarDraft ?? session.sidecarPath}
                  onchange={(value) => (sidecarDraft = value)}
                  onblur={commitSidecar}
                  onkeydown={(/** @type {KeyboardEvent} */ event) => {
                    if (event.key === 'Enter') commitSidecar()
                  }}
                />
                <Button onclick={browseSidecar} disabled={choosing}>
                  {t('shell.action.chooseFolder')}
                </Button>
              </div>
              {#if session.sidecarPath && !capabilities.sidecar}
                <p class="line">{t('settings.sidecar.notFound')}</p>
              {/if}
            {/snippet}
          </Field>
        </div>

        {#if capabilities.sidecar}
          <Field label={t('settings.fluxBackend.label')} layout="row">
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

          <Field label={t('settings.sidecarModel.label')} layout="row" controlId="settings-sidecar-model">
            {#snippet children()}
              <Select
                id="settings-sidecar-model"
                fit
                disabled={sidecarModels.length === 0}
                options={sidecarChoices}
                value={session.fluxModel || sidecarChoices[0].value}
                onchange={(value) => {
                  setFluxModel(value)
                  push()
                }}
              />
            {/snippet}
          </Field>
        {/if}
      </div>
    </div>

    <!-- Cloud: the component is the tab, unchanged; only its container moved. -->
    <div
      class="panel"
      role="tabpanel"
      id={panelId('inference')}
      aria-labelledby={tabId('inference')}
      hidden={active !== 'inference'}
    >
      <div class="column">
        <h2>{t('settings.section.inference')}</h2>
        <InferenceSettings />
      </div>
    </div>

    <!-- Performance. Its tail is the placement list, which holds nothing
         focusable, so the scroller carries the tab stop. -->
    <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
    <div
      class="panel"
      role="tabpanel"
      tabindex="0"
      id={panelId('performance')}
      aria-labelledby={tabId('performance')}
      hidden={active !== 'performance'}
    >
      <div class="column">
        <h2>{t('settings.section.performance')}</h2>

        {#if catalogue}
          <!-- The runtime is an archive that gets unpacked, not a catalogue
               row. The build named is the one a Download would fetch. -->
          <ul class="rows">
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
                  <span class="row-error">{failureText(RUNTIME_ID)}</span>
                {/if}
                {#if notes[RUNTIME_ID]}
                  <span class="row-error">{t(notes[RUNTIME_ID])}</span>
                {/if}
                <!-- Which build is actually here, said only when it is not the
                     one the row names. -->
                {#if installedNote}
                  <span class="row-partial">{installedNote}</span>
                {/if}
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
                  <!-- Over an installed runtime only where there is a choice of
                       build: elsewhere it would re-download what is there. -->
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

          <!-- Only where the platform publishes more than one build: a select
               with one option only asks a question it has already answered. -->
          {#if catalogue.runtime.flavours.length > 1}
            <Field label={t('settings.models.runtime.flavour')} layout="row" controlId="settings-runtime-flavour">
              {#snippet children()}
                <Select
                  id="settings-runtime-flavour"
                  fit
                  options={catalogue.runtime.flavours.map((build) => ({
                    value: build.id,
                    label: `${build.id} · ${build.ortVersion} · ${t('models.value.size', { bytes: build.bytes })}`,
                  }))}
                  value={catalogue.runtime.flavour}
                  onchange={chooseFlavour}
                />
              {/snippet}
            </Field>
            {#if flavourNeeds}
              <p class="line">{flavourNeeds}</p>
            {/if}
          {/if}

          {#if catalogue.modelsDir}
            <p class="path">{t('settings.models.folder', { path: catalogue.modelsDir })}</p>
          {/if}
        {:else}
          <p class="note">{t('settings.models.unavailable')}</p>
        {/if}

        <div class="accel">
          <Field label={t('settings.accel.label')} layout="row" controlId="settings-accelerator">
            {#snippet children()}
              <!-- The note is on the option's face as well as its tooltip: a
                   caveat only a hover can find is one a keyboard never sees. -->
              <Select
                id="settings-accelerator"
                fit
                options={acceleratorChoices}
                value={acceleratorValue}
                onchange={chooseAccelerator}
              />
            {/snippet}
          </Field>
        </div>

        {#if accelFailure}
          <p class="line">{t('settings.accel.unreadable')}</p>
        {/if}

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
    </div>

    <!-- Shortcuts -->
    <div
      class="panel"
      role="tabpanel"
      id={panelId('shortcuts')}
      aria-labelledby={tabId('shortcuts')}
      hidden={active !== 'shortcuts'}
    >
      <div class="column">
        <h2>{t('settings.section.shortcuts')}</h2>
        <ShortcutSheet headingLevel="h3" />
      </div>
    </div>

    <!-- About ends in the written offer of source and the cloud terms, so its
         scroller carries the tab stop. It is a GPL-3.0 obligation and this
         screen is its only route. -->
    <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
    <div
      class="panel"
      role="tabpanel"
      tabindex="0"
      id={panelId('about')}
      aria-labelledby={tabId('about')}
      hidden={active !== 'about'}
    >
      <div class="column">
        <h2>{t('settings.section.about')}</h2>
        <AboutSection />
      </div>
    </div>
  </div>
</Screen>

<style>
  .settings {
    flex: 1;
    min-height: 0;
    display: flex;
  }

  /* ---- sidebar ---------------------------------------------------------- */

  .side {
    flex: none;
    width: 212px;
    display: flex;
    flex-direction: column;
    gap: var(--s-6);
    padding: var(--s-4) var(--s-3);
    background: var(--sb);
    border-right: 1px solid var(--line);
    overflow-y: auto;
  }

  .side-head {
    display: flex;
    align-items: center;
    gap: var(--s-2);
  }
  .back {
    flex: none;
    display: grid;
    place-items: center;
    width: 28px;
    height: 28px;
    padding: 0;
    border: none;
    border-radius: var(--r-lg);
    background: none;
    color: var(--t2);
    cursor: pointer;
    transition: background var(--dur-fast) var(--ease), color var(--dur-fast) var(--ease);
  }
  .back:hover { background: var(--accent-soft); color: var(--text) }
  .back:active { transform: scale(.96) }
  h1 {
    margin: 0;
    font-size: 14px;
    font-weight: 600;
    letter-spacing: -.005em;
  }

  .tabs {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  /* Weight never changes with selection; the fill and the icon's colour carry
     it, so a label never shifts under the pointer. */
  .tab {
    display: flex;
    align-items: center;
    gap: var(--s-3);
    height: 30px;
    padding: 0 var(--s-3);
    border: none;
    border-radius: var(--r-chip);
    background: none;
    color: var(--t2);
    font: inherit;
    font-size: 12.5px;
    text-align: left;
    white-space: nowrap;
    cursor: pointer;
    transition: background var(--dur-fast) var(--ease), color var(--dur-fast) var(--ease);
  }
  .tab:hover { background: var(--accent-soft); color: var(--text) }
  .tab.on { background: var(--accent-soft); color: var(--text) }
  .tab.on :global(svg) { color: var(--accent) }
  .tab:focus-visible { outline-offset: -2px }
  .tab-label { overflow: hidden; text-overflow: ellipsis }

  /* ---- content ---------------------------------------------------------- */

  .panel {
    flex: 1;
    min-width: 0;
    overflow-y: auto;
    overscroll-behavior: contain;
    scrollbar-gutter: stable;
  }
  .panel:focus-visible { outline-offset: -2px }
  .panel[hidden] { display: none }

  .column {
    max-width: 680px;
    margin: 0 auto;
    padding: var(--s-8) var(--s-6) 64px;
  }

  h2 {
    margin: 0 0 var(--s-6);
    font-size: 20px;
    font-weight: 600;
    letter-spacing: -.01em;
    line-height: 1.2;
  }
  .sub {
    margin: var(--s-8) 0 var(--s-2);
    font-size: 12.5px;
    font-weight: 600;
  }

  /* Rows at screen scale: the Field row's 32px floor was set for a 560px
     dialog. */
  .column :global(.field.row .line) { min-height: 40px }
  .column :global(.field.row .label) { font-size: 12.5px }

  .block {
    padding: var(--s-3) 0 var(--s-5);
    border-bottom: 1px solid var(--line);
  }
  .block.flush { padding-top: 0 }
  .theme { container-type: inline-size; padding-top: 0 }
  /* Six swatches: one row of six where they fit, two rows of three where
     they do not, never five and one. */
  .theme :global(.picker) { grid-template-columns: repeat(6, minmax(0, 1fr)) }
  @container (max-width: 520px) {
    .theme :global(.picker) { grid-template-columns: repeat(3, minmax(0, 1fr)) }
  }
  .token { border-bottom: none; padding-top: var(--s-6) }

  .check {
    width: 15px;
    height: 15px;
    margin: 0;
    accent-color: var(--accent);
    cursor: pointer;
  }

  .inline {
    display: flex;
    align-items: center;
    gap: var(--s-2);
    width: 100%;
  }
  .inline :global(> *:first-child) {
    flex: 1;
    min-width: 0;
  }

  .languages { margin-bottom: var(--s-8) }
  /* One width for the three pickers, so a language with a longer engine name
     does not make its row look different from the others. */
  .pick { width: 15rem; max-width: 100% }

  .accel { margin-top: var(--s-6) }

  /* One row is a name and a meta line on the left and its buttons on the
     right, with the hairline every row on this screen uses. */
  .rows {
    margin: 0;
    padding: 0;
    list-style: none;
  }
  .row {
    display: flex;
    align-items: center;
    gap: var(--s-4);
    padding: var(--s-3) 0;
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
    font-size: 12.5px;
    color: var(--text);
  }
  .row-meta,
  .row-partial {
    font-size: 11px;
    color: var(--t3);
    line-height: 1.4;
  }
  /* `--t2` rather than `--t3`: a failure is the one line the reader has to act
     on, and `--t3` is under 4.5:1 against the dark field. */
  .row-error {
    font-size: 11px;
    color: var(--t2);
    line-height: 1.4;
    word-break: break-word;
  }
  .row-actions {
    display: flex;
    flex: none;
    gap: var(--s-2);
  }

  .note,
  .line {
    margin: var(--s-2) 0 0;
    font-size: 11px;
    color: var(--t3);
    line-height: 1.45;
    max-width: 68ch;
  }
  .note { margin-top: var(--s-6) }
  .line.failed,
  .error { color: var(--warn) }
  .error {
    margin: var(--s-2) 0 0;
    font-size: 11px;
    line-height: 1.45;
  }

  /* A path is data: selectable, and broken anywhere rather than overflowing. */
  .path {
    margin: var(--s-3) 0 0;
    font-size: 11px;
    color: var(--t3);
    line-height: 1.4;
    word-break: break-all;
    user-select: text;
  }

  /* A narrow window keeps the list and drops its words: the icons stay, and
     the labels remain the tabs' accessible names. */
  @media (max-width: 720px) {
    .side { width: 52px; padding-inline: var(--s-2) }
    .side-head h1,
    .tab-label {
      position: absolute;
      width: 1px;
      height: 1px;
      overflow: hidden;
      clip-path: inset(50%);
      white-space: nowrap;
    }
    .tab { justify-content: center; padding: 0 }
    .back { margin-inline: auto }
    .column { padding-inline: var(--s-5) }
  }
</style>
