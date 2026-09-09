/**
 * Session preferences - the things that survive a relaunch and are not part of
 * any one project: theme, the default reading direction for new projects,
 * whether cloud engines are allowed, how the original-view control behaves,
 * and where the editor's three floating windows sit.
 *
 * Shape of the module: a single rune-backed object plus free functions that
 * mutate it. Not a class with getters over hidden mutables - consumers read
 * `session.theme` directly and get reactivity for free.
 *
 * Persistence goes through `persist.js`: validated on read, defaults on
 * anything unrecognised, and written explicitly by the setters rather than by
 * an effect, so `systemDark` (derived from `matchMedia`) can never be written
 * to disk.
 */

import {
  readRecord,
  writeRecord,
  oneOf,
  boolOr,
  numberIn,
  plainObject,
} from './persist.js'
import {
  DEFAULT_POINTER_MODIFIER,
  isApplePlatform,
  modifierCap,
  normalizePointerModifier,
  rebindShortcut,
  resetAllShortcuts,
  resetShortcut,
  sanitizeShortcutOverrides,
  setShortcutOverrides,
  shortcutOverrides,
  unbindShortcut,
} from '../shortcuts.js'
import { provideContextParam } from '../i18n/index.js'
import {
  WINDOW_IDS,
  defaultGeometry,
  clampPosition,
  clampSize,
  raisedOrder,
  minWidthFor,
  contentSized,
  MAX_WIDTH,
  MIN_HEIGHT,
} from '../model/windows.js'

const STORAGE_KEY = 'session.v1'

export const THEMES = /** @type {const} */ (['light', 'dark', 'system'])
export const READING_DIRECTIONS = /** @type {const} */ (['rtl', 'ltr'])
export const ORIGINAL_VIEW_MODES = /** @type {const} */ (['hold', 'pinned'])

/**
 * The values the AI redraw engine row offers.
 *
 * `'auto'` first because it is the default and the answer for almost everybody:
 * the core resolves it per platform. The other two are the backends
 * `cleaner_core::sidecar::backend` has a contract for and can render through -
 * `sdcpp` is in that table and is deliberately **not** here, because it declines
 * every open and a control that fails is worse than a control that is absent.
 */
export const FLUX_BACKENDS = /** @type {const} */ (['auto', 'mflux', 'sdnq'])

export { WINDOW_IDS }

/**
 * The shortcut table names the three windows by what they used to be - panels
 * called `pages`, `masks` and `tools`. `src/lib/shortcuts.js` is the one place
 * a keybinding may be declared and is not this task's to edit, so the mapping
 * onto window ids lives here, in one line, rather than in the keyboard layer.
 */
const PANEL_TO_WINDOW = /** @type {Record<string, string>} */ ({
  pages: 'pages',
  masks: 'layers',
  tools: 'tool',
})

/** The viewport the default layout is computed against. */
function viewport() {
  return {
    vw: Math.round(globalThis.innerWidth) || 1400,
    vh: Math.round(globalThis.innerHeight) || 900,
  }
}

/**
 * @typedef {import('../model/windows.js').WindowGeometry & {open: boolean, fold: boolean}} WindowState
 */

/**
 * @typedef {Object} PersistedSession
 * @property {'light'|'dark'|'system'} theme
 * @property {'rtl'|'ltr'} readingDirection - the default for new projects; a project overrides it
 * @property {boolean} cloudAllowed
 * @property {'hold'|'pinned'} originalView
 * @property {string} sidecarPath
 * @property {string} fluxModel
 * @property {'auto'|'mflux'|'sdnq'} fluxBackend
 * @property {string} accelerator - `auto`, `cpu`, or an execution provider id; see `setAccelerator`
 * @property {'alt'|'meta'|'control'|'shift'} cloneSourceModifier - held while clicking to set Clone / heal's source; see `setCloneSourceModifier`
 * @property {boolean} firstLaunchOffered - whether the first-launch download offer has been made on this machine
 * @property {Record<string, import('../shortcuts.js').Chord|null>} shortcuts - rebindings, by shortcut id; only the differences from the defaults
 * @property {Record<string, WindowState>} windows
 */

/** @returns {PersistedSession} */
function defaults() {
  const { vw, vh } = viewport()
  const geometry = defaultGeometry(vw, vh)
  /** @type {Record<string, WindowState>} */
  const windows = {}
  for (const id of WINDOW_IDS) windows[id] = { ...geometry[id], open: true, fold: false }
  return {
    theme: 'system',
    readingDirection: 'rtl',
    cloudAllowed: false,
    originalView: 'hold',
    sidecarPath: '',
    fluxModel: '',
    fluxBackend: 'auto',
    accelerator: 'auto',
    cloneSourceModifier: DEFAULT_POINTER_MODIFIER,
    firstLaunchOffered: false,
    shortcuts: {},
    windows,
  }
}

/**
 * Coerce anything at all into a valid session record. Exported so the
 * validation can be tested without a DOM.
 *
 * A stored geometry is validated field by field but **not** clamped to the
 * current viewport here: the viewport at read time may not be the one the
 * editor will mount into. `clampWindowsToViewport()` does that, from the
 * editor, and again on every resize.
 *
 * @param {unknown} raw
 * @returns {PersistedSession}
 */
export function sanitizeSession(raw) {
  const base = defaults()
  const record = plainObject(raw)
  const stored = plainObject(record.windows)

  /** @type {Record<string, WindowState>} */
  const windows = {}
  for (const id of WINDOW_IDS) {
    const fallback = base.windows[id]
    const win = plainObject(stored[id])
    // A null height is "size to content", and it is a value in its own right -
    // it must survive both a missing record and a stored `null`.
    const height = win.h === undefined ? fallback.h : win.h
    windows[id] = {
      // A window may hang off an edge, so x and y take a wide band here and
      // are brought onto the real viewport by `clampWindowsToViewport`.
      x: numberIn(win.x, { min: -4000, max: 8000, fallback: fallback.x }),
      y: numberIn(win.y, { min: -4000, max: 8000, fallback: fallback.y }),
      // A content-sized window's stored width is a stale measurement rather
      // than a preference: it is kept only so the first clamp has a truthful
      // number to keep a grabbable strip by, and the element overwrites it as
      // soon as it is on screen. So no ceiling for it either.
      w: numberIn(win.w, {
        min: minWidthFor(id),
        max: contentSized(id) ? 8000 : MAX_WIDTH,
        fallback: fallback.w,
      }),
      // A content-sized window has no height and cannot be folded: the tool
      // bar is one row of controls, and it has neither a resize corner nor a
      // body to collapse. A record written before it was a bar still carries
      // the tool window's `h` and may carry `fold: true`, and restoring either
      // would leave the store asserting a geometry the element does not have.
      // Nothing would be seen to go wrong - `ToolBar.svelte` reads neither
      // field - which is the reason to drop them here rather than trust that
      // no future reader looks. Dropped on read rather than migrated: there is
      // nothing to keep.
      h: contentSized(id)
        ? null
        : height === null
          ? null
          : numberIn(height, { min: MIN_HEIGHT, max: 8000, fallback: fallback.h ?? MIN_HEIGHT }),
      open: boolOr(win.open, fallback.open),
      fold: contentSized(id) ? false : boolOr(win.fold, fallback.fold),
    }
  }

  return {
    theme: /** @type {'light'|'dark'|'system'} */ (oneOf(record.theme, THEMES, base.theme)),
    readingDirection: /** @type {'rtl'|'ltr'} */ (
      oneOf(record.readingDirection, READING_DIRECTIONS, base.readingDirection)
    ),
    cloudAllowed: boolOr(record.cloudAllowed, base.cloudAllowed),
    originalView: /** @type {'hold'|'pinned'} */ (
      oneOf(record.originalView, ORIGINAL_VIEW_MODES, base.originalView)
    ),
    sidecarPath: typeof record.sidecarPath === 'string' ? record.sidecarPath : (typeof record.sidecarFolder === 'string' ? record.sidecarFolder : base.sidecarPath),
    fluxModel: typeof record.fluxModel === 'string' ? record.fluxModel : (typeof record.sidecarModel === 'string' ? record.sidecarModel : base.fluxModel),
    fluxBackend: /** @type {'auto'|'mflux'|'sdnq'} */ (
      oneOf(record.fluxBackend ?? record.sidecarBackend, FLUX_BACKENDS, base.fluxBackend)
    ),
    // Not validated against a list, and deliberately: the providers a machine
    // has come from the loaded ONNX Runtime (`listAccelerators`), so the only
    // side that could check this value is the backend - which refuses an
    // unusable one with a reason rather than silently substituting. A closed
    // list here would drop a provider a newer runtime added.
    accelerator: typeof record.accelerator === 'string' ? record.accelerator : base.accelerator,
    // The shortcut module owns this vocabulary, the same way it owns the chord
    // one: anything that is not one of the four modifiers a pointer event can
    // carry falls back to the modifier this gesture has always used.
    cloneSourceModifier: normalizePointerModifier(record.cloneSourceModifier),
    // False is the safe default in both directions: a record written before
    // this flag existed offers the download once, which is the whole point of
    // the offer, and a machine that has already been asked carries a `true` it
    // wrote itself.
    firstLaunchOffered: boolOr(record.firstLaunchOffered, base.firstLaunchOffered),
    // The shortcut table owns this vocabulary and validates it: an id the
    // table no longer has, a chord that will not parse, or a chord that is
    // only the default written out is dropped here rather than kept as a
    // binding nothing can run.
    shortcuts: sanitizeShortcutOverrides(record.shortcuts),
    windows,
  }
}

/** Initial stacking: the order they are declared in, lowest first. */
function initialStacking() {
  /** @type {Record<string, number>} */
  const ranks = {}
  WINDOW_IDS.forEach((id, index) => {
    ranks[id] = index + 1
  })
  return ranks
}

const storedRecord = readRecord(STORAGE_KEY, null)

/**
 * Whether this machine had a session on disk when the app started. Read once,
 * at module load, because the first `save()` makes it true forever after - and
 * "has this user ever set a preference in this app" is the question
 * `reconcileSettings()` needs answered.
 */
export const sessionWasStored = storedRecord !== null

/**
 * The live session. `systemDark` and `stacking` are runtime-only -
 * `systemDark` mirrors the OS, and which window was raised last is not worth
 * a disk write per click (see `persistable()`).
 */
export const session = $state({
  ...sanitizeSession(storedRecord),
  systemDark: false,
  /** @type {Record<string, number>} id → 1..n, highest on top */
  stacking: initialStacking(),
})

// `src/lib/shortcuts.js` holds the table actually in force and every consumer
// reads it from there. `session.shortcuts` is where that table's *differences*
// are stored and persisted, and the two are seeded here and kept in step by
// `syncShortcuts()` - never edited apart.
setShortcutOverrides(session.shortcuts)

/**
 * The subset that goes to disk. Hand-picked, so no derived value can leak in.
 * @returns {PersistedSession}
 */
function persistable() {
  /** @type {Record<string, WindowState>} */
  const windows = {}
  for (const id of WINDOW_IDS) {
    const win = session.windows[id]
    windows[id] = { x: win.x, y: win.y, w: win.w, h: win.h, open: win.open, fold: win.fold }
  }
  return {
    theme: session.theme,
    readingDirection: session.readingDirection,
    cloudAllowed: session.cloudAllowed,
    originalView: session.originalView,
    sidecarPath: session.sidecarPath,
    fluxModel: session.fluxModel,
    fluxBackend: session.fluxBackend,
    accelerator: session.accelerator,
    cloneSourceModifier: session.cloneSourceModifier,
    firstLaunchOffered: session.firstLaunchOffered,
    shortcuts: shortcutOverrides(),
    windows,
  }
}

function save() {
  writeRecord(STORAGE_KEY, persistable())
}

/* ------------------------------------------------------------------ */
/* Theme                                                               */
/* ------------------------------------------------------------------ */

/**
 * The theme actually in force: `system` resolves against the OS.
 * Call it inside a template or an effect and it tracks both inputs.
 *
 * @returns {'light'|'dark'}
 */
export function resolvedTheme() {
  if (session.theme === 'system') return session.systemDark ? 'dark' : 'light'
  return session.theme
}

/**
 * Write the resolved theme onto `<html data-theme>`, which is what `app.css`
 * keys the dark token block off. Reactive: call it from an `$effect`.
 */
export function applyTheme() {
  const root = globalThis.document?.documentElement
  if (root) root.dataset.theme = resolvedTheme()
}

/**
 * Track `prefers-color-scheme` for the lifetime of the app, so `system` follows
 * the OS *live* and not only at load.
 *
 * @returns {() => void} teardown - return it straight from an `$effect`
 */
export function installThemeSync() {
  const query = globalThis.matchMedia?.('(prefers-color-scheme: dark)')
  if (!query) return () => {}
  session.systemDark = query.matches
  /** @param {MediaQueryListEvent} event */
  const onChange = (event) => {
    session.systemDark = event.matches
  }
  query.addEventListener('change', onChange)
  return () => query.removeEventListener('change', onChange)
}

/** @param {'light'|'dark'|'system'} theme */
export function setTheme(theme) {
  session.theme = oneOf(theme, THEMES, session.theme)
  save()
}

/* ------------------------------------------------------------------ */
/* Everything else                                                     */
/* ------------------------------------------------------------------ */

/** @param {'rtl'|'ltr'} direction */
export function setReadingDirection(direction) {
  session.readingDirection = oneOf(direction, READING_DIRECTIONS, session.readingDirection)
  save()
}

/** @param {boolean} allowed */
export function setCloudAllowed(allowed) {
  session.cloudAllowed = boolOr(allowed, session.cloudAllowed)
  save()
}

/**
 * Remember that the first-launch download offer has been made.
 *
 * Set by both answers the dialog takes - `Not now` and `Download` - because
 * what it records is that the user *was asked*, not what they said: a machine
 * that started the downloads and lost its connection halfway has been asked as
 * surely as one that declined, and a modal that came back on the next launch
 * to say the same thing again would be nagging rather than offering. Settings ›
 * Models is the way back for both of them.
 *
 * **Not a backend setting.** It is a fact about this interface on this machine
 * and nothing in the core reads it, so it stays out of `backendSettingsPatch()`
 * and out of `adoptBackendSettings()`; there is one copy of it and it is here.
 */
export function markFirstLaunchOffered() {
  session.firstLaunchOffered = true
  save()
}

/** @param {'hold'|'pinned'} mode */
export function setOriginalView(mode) {
  session.originalView = oneOf(mode, ORIGINAL_VIEW_MODES, session.originalView)
  save()
}

/** @param {string} path */
export function setSidecarPath(path) {
  session.sidecarPath = typeof path === 'string' ? path : ''
  save()
}

/** @param {string} model */
export function setFluxModel(model) {
  session.fluxModel = typeof model === 'string' ? model : ''
  save()
}

/**
 * Which sidecar backend the AI redraw rung asks for.
 *
 * `'auto'` is not a backend - it is a deferral to the core, which resolves it
 * per platform (`mflux` on Apple Silicon, `sdnq` elsewhere) beside the table it
 * resolves against. Nothing on this side knows which platform it is on, and
 * nothing here should: a value this store invented would be a second copy of
 * that decision.
 *
 * @param {string} backend
 */
export function setFluxBackend(backend) {
  session.fluxBackend = /** @type {'auto'|'mflux'|'sdnq'} */ (
    oneOf(backend, FLUX_BACKENDS, session.fluxBackend)
  )
  save()
}

/**
 * Which execution provider the next session is built on.
 *
 * `auto` - the default and the right answer for almost everybody - lets
 * `cleaner_core::accel` choose per model, which is not one answer: the
 * inpainter needs a `DFT` kernel that CUDA does not have, so a machine can
 * correctly run the detector on the GPU and LaMa on the CPU. `cpu` pins
 * everything to the CPU, and a provider id forces that one and is **refused
 * with a reason** where it cannot be had rather than silently ignored.
 *
 * **It takes effect on the next session, not on the running one.** `run.rs`
 * reads it before `Pipeline::open` and a session already built keeps the
 * provider it was built with, which is why the Settings row says so.
 *
 * @param {string} id
 */
export function setAccelerator(id) {
  session.accelerator = typeof id === 'string' && id ? id : 'auto'
  save()
}

/**
 * Which modifier is held while clicking the page to set where Clone / heal
 * reads from.
 *
 * It was `Alt` and nothing else, written into `DrawLayer`'s `onpointerdown` as
 * `event.altKey` and into the tool window's hint as the word *Alt* - a word
 * that is not printed on an Apple keyboard, where the key is `⌥` and is called
 * Option. So the gesture was documented in a name its user could not find.
 *
 * **Not a backend setting.** It is a fact about this interface's pointer
 * handling; nothing in the core reads it, and `run.rs` has no gestures. So it
 * stays out of `backendSettingsPatch()` and out of `adoptBackendSettings()`,
 * beside `firstLaunchOffered` - there is one copy of it and it is here.
 *
 * @param {string} modifier - one of `POINTER_MODIFIERS`
 */
export function setCloneSourceModifier(modifier) {
  session.cloneSourceModifier = normalizePointerModifier(modifier)
  save()
}

/**
 * And the same value as a keycap, for the one string whose call site cannot
 * pass it: `tools.hint.cloneHeal` is rendered by the tool bar as
 * `t(spec.hintKey)`, over a key the tool table chose.
 *
 * Registered here because this module owns the value. `i18n` imports nothing
 * to read it; the getter is pushed in, and reading `session` inside it is what
 * makes the tool bar's hint redraw when the row in Settings moves.
 */
provideContextParam('cloneSourceModifier', () =>
  modifierCap(session.cloneSourceModifier, { apple: isApplePlatform() }),
)

/* ------------------------------------------------------------------ */
/* Floating windows                                                    */
/* ------------------------------------------------------------------ */

/**
 * Position and size are written *live* while a pointer drags and persisted
 * once, on release: `save()` serialises the whole record to `localStorage`, and
 * a drag would otherwise do that a few hundred times. Open and fold are
 * discrete decisions and persist immediately.
 *
 * @param {string} id
 * @param {{x?: number, y?: number, w?: number, h?: number|null}} patch
 */
export function setWindowBox(id, patch) {
  const win = session.windows[id]
  if (!win) return
  const { vw, vh } = viewport()
  const size = clampSize(
    { w: patch.w ?? win.w, h: patch.h === undefined ? win.h : patch.h },
    vh,
    id,
  )
  const position = clampPosition(
    { x: patch.x ?? win.x, y: patch.y ?? win.y, w: size.w },
    vw,
    vh,
    id,
  )
  win.x = position.x
  win.y = position.y
  win.w = size.w
  win.h = size.h
}

/** Persist whatever the last drag or resize left behind. */
export function commitWindows() {
  save()
}

/**
 * Bring a window to the front of the other two. Not persisted - which window
 * was on top last is worth less than a disk write per click.
 *
 * @param {string} id
 */
export function raiseWindow(id) {
  if (session.stacking[id] === WINDOW_IDS.length) return
  session.stacking = raisedOrder(session.stacking, id)
}

/**
 * @param {string} id
 * @param {boolean} open
 */
export function setWindowOpen(id, open) {
  const win = session.windows[id]
  if (!win) return
  win.open = open
  if (open) raiseWindow(id)
  save()
}

/** Open a window if it is closed, and raise it either way. @param {string} id */
export function openWindow(id) {
  if (!session.windows[id]) return
  if (!session.windows[id].open) setWindowOpen(id, true)
  else raiseWindow(id)
}

/** @param {string} id */
export function toggleWindow(id) {
  if (!session.windows[id]) return
  setWindowOpen(id, !session.windows[id].open)
}

/** @param {string} id */
export function foldWindow(id) {
  const win = session.windows[id]
  if (!win) return
  win.fold = !win.fold
  save()
}

/**
 * Back to where this window started - the Escape of a keyboard move, and
 * **position only**, as the chrome spec says.
 *
 * Escape is bound on the grip, which is the move handle; the size belongs to
 * the corner, which has no Escape of its own. A user who resized a window and
 * then pressed Escape to re-place it asked for one of those two things, and
 * assigning the whole default geometry took the other away as well.
 *
 * The default position is computed from the current viewport and then clamped
 * through `setWindowBox` like any other move, so a window cannot be reset to
 * somewhere a short viewport does not have.
 *
 * @param {string} id
 */
export function resetWindowBox(id) {
  const win = session.windows[id]
  if (!win) return
  const { vw, vh } = viewport()
  const geometry = defaultGeometry(vw, vh)[id]
  if (!geometry) return
  const x = contentSized(id)
    ? Math.max(16, Math.round(vw / 2 - (win.w || geometry.w) / 2))
    : geometry.x
  setWindowBox(id, { x, y: geometry.y })
  save()
}

/**
 * Pull every window back onto the viewport. Called when the editor mounts and
 * on every viewport resize: a layout saved on a 27" display must not leave the
 * windows unreachable on a laptop.
 *
 * Deliberately does not persist - a resize is not a decision about where the
 * windows live, and the next drag will save the corrected geometry anyway.
 *
 * @param {number} [vw]
 * @param {number} [vh]
 */
export function clampWindowsToViewport(vw, vh) {
  const size = viewport()
  const width = vw ?? size.vw
  const height = vh ?? size.vh
  for (const id of WINDOW_IDS) {
    const win = session.windows[id]
    if (!win) continue
    const box = clampSize({ w: win.w, h: win.h }, height, id)
    const position = clampPosition({ x: win.x, y: win.y, w: box.w }, width, height, id)
    win.x = position.x
    win.y = position.y
    win.w = box.w
    win.h = box.h
  }
}

/**
 * The `F` / `L` / `T` shortcuts, which still speak the shortcut table's panel
 * vocabulary. See `PANEL_TO_WINDOW`.
 *
 * @param {'pages'|'masks'|'tools'} panel
 */
export function togglePanel(panel) {
  const id = PANEL_TO_WINDOW[panel]
  if (id) toggleWindow(id)
}

/* ------------------------------------------------------------------ */
/* Keyboard shortcuts                                                  */
/* ------------------------------------------------------------------ */

/**
 * Mirror the table's overrides back onto the session and write them.
 *
 * The reassignment is what makes the change visible: `session.shortcuts` is
 * the rune every consumer of the sheet reads to know the bindings moved, and
 * `src/lib/shortcuts.js` is where they actually live.
 */
function syncShortcuts() {
  session.shortcuts = shortcutOverrides()
  save()
}

/**
 * Bind a shortcut to a recorded chord. Refused, with a reason, when the chord
 * already belongs to another entry in an overlapping scope.
 *
 * @param {string} id
 * @param {import('../shortcuts.js').Chord} chord
 * @returns {import('../shortcuts.js').RebindResult}
 */
export function setShortcutBinding(id, chord) {
  const result = rebindShortcut(id, chord)
  if (result.ok) syncShortcuts()
  return result
}

/**
 * Clear a shortcut's chord. The row stays in the sheet, reading unbound.
 *
 * @param {string} id
 * @returns {import('../shortcuts.js').RebindResult}
 */
export function clearShortcutBinding(id) {
  const result = unbindShortcut(id)
  if (result.ok) syncShortcuts()
  return result
}

/**
 * Give one shortcut its default back.
 *
 * @param {string} id
 * @returns {import('../shortcuts.js').RebindResult}
 */
export function resetShortcutBinding(id) {
  const result = resetShortcut(id)
  if (result.ok) syncShortcuts()
  return result
}

/** Give every shortcut its default back. */
export function resetAllShortcutBindings() {
  resetAllShortcuts()
  syncShortcuts()
}

/* ------------------------------------------------------------------ */
/* Reconciling with the backend's own settings                         */
/* ------------------------------------------------------------------ */

/**
 * Adopt the backend's settings snapshot (`backend.readSettings()`) into the
 * session. The mapping between the two vocabularies - `cloudEngines:
 * 'allowed'|'blocked'` there, `cloudAllowed: boolean` here - lives only in this
 * function and its mirror, `backendSettingsPatch()`.
 *
 * Called by `reconcileSettings()`, never directly.
 *
 * @param {Record<string, unknown>} settings
 */
export function adoptBackendSettings(settings) {
  const record = plainObject(settings)
  if (record.theme !== undefined) setTheme(/** @type {any} */ (record.theme))
  if (record.readingDirection !== undefined) {
    setReadingDirection(/** @type {any} */ (record.readingDirection))
  }
  if (record.cloudEngines !== undefined) setCloudAllowed(record.cloudEngines === 'allowed')
  if (record.originalView !== undefined) setOriginalView(/** @type {any} */ (record.originalView))
  if (record.sidecarPath !== undefined) {
    setSidecarPath(typeof record.sidecarPath === 'string' ? record.sidecarPath : '')
  } else if (record.sidecarFolder !== undefined) {
    setSidecarPath(typeof record.sidecarFolder === 'string' ? record.sidecarFolder : '')
  }
  if (record.fluxModel !== undefined) {
    setFluxModel(typeof record.fluxModel === 'string' ? record.fluxModel : '')
  } else if (record.sidecarModel !== undefined) {
    setFluxModel(typeof record.sidecarModel === 'string' ? record.sidecarModel : '')
  }
  if (record.fluxBackend !== undefined) {
    setFluxBackend(/** @type {any} */ (record.fluxBackend))
  } else if (record.sidecarBackend !== undefined) {
    setFluxBackend(/** @type {any} */ (record.sidecarBackend))
  }
  if (record.accelerator !== undefined) {
    setAccelerator(typeof record.accelerator === 'string' ? record.accelerator : 'auto')
  }
  if (record.shortcuts !== undefined) {
    setShortcutOverrides(record.shortcuts)
    syncShortcuts()
  }
}

/**
 * The session's shared preferences in the backend's vocabulary - what
 * `backend.writeSettings()` takes.
 *
 * `shortcuts` is the one nested value in the patch, and it survives the seam's
 * shallow one-level merge intact *because* it is nested: the whole override map
 * is written as one key, so clearing the last override is expressible (an empty
 * object) where a flattened `shortcuts.tool.brush` family would leave stale
 * entries behind forever.
 *
 * @returns {{theme: string, readingDirection: string, cloudEngines: 'allowed'|'blocked', originalView: string, sidecarPath: string, fluxModel: string, fluxBackend: string, accelerator: string, shortcuts: Record<string, import('../shortcuts.js').Chord|null>}}
 */
export function backendSettingsPatch() {
  return {
    theme: session.theme,
    readingDirection: session.readingDirection,
    cloudEngines: session.cloudAllowed ? 'allowed' : 'blocked',
    originalView: session.originalView,
    sidecarPath: session.sidecarPath,
    fluxModel: session.fluxModel,
    fluxBackend: session.fluxBackend,
    accelerator: session.accelerator,
    shortcuts: shortcutOverrides(),
  }
}

/**
 * Reconcile the backend's settings with the session, and say what to write
 * back. **This is the only place the two are compared**, and the direction it
 * runs is:
 *
 * > **The backend seeds a session that has never existed. After that the
 * > session wins and the backend is made to agree with it.**
 *
 * Both halves matter, and neither is arbitrary.
 *
 * *The backend seeds a first run* because on a machine with no stored session
 * there is no user preference to defend - the backend's snapshot is the only
 * opinion in the system, and in the shipped app it is a real config file that a
 * user may well have written by hand.
 *
 * *The session wins thereafter* because the backend is not obliged to be
 * durable and today's is not: the mock rebuilds `defaultSettings()` on every
 * reload. Adopting unconditionally would reset the theme every time the page
 * refreshed and silently discard a preference the user had set in this app,
 * which is a worse failure than the two stores disagreeing was.
 *
 * The consequence that matters most: `session.cloudAllowed` - which
 * `cloudRefused()` gates every cloud request on - and the adapter's own
 * `settings.cloudEngines` can no longer drift apart, because opening Settings
 * pushes the session's value down, and every change pushes it again.
 *
 * @param {Record<string, unknown>} settings - `backend.readSettings()`
 * @returns {ReturnType<typeof backendSettingsPatch>} the patch to write back
 */
export function reconcileSettings(settings) {
  if (!sessionWasStored) adoptBackendSettings(settings)
  return backendSettingsPatch()
}
