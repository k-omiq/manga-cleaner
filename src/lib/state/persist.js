/**
 * Defensive `localStorage` access and value validation.
 *
 * `localStorage` is a shared, user-writable, cross-version bag of strings. A
 * stale record from an older build, a hand-edited value, a half-written JSON
 * blob or a browser in private mode must never be able to brick the app, so
 * every read runs through here: parse defensively, validate each field, fall
 * back to the default. Nothing derived is ever written - callers pass an
 * explicit, hand-picked record.
 *
 * Plain JavaScript, no Svelte. `storage` is injectable so the validators and
 * the read/write pair can be tested in the `node` test environment, where
 * there is no `localStorage` at all.
 */

/** Every key this app owns is prefixed, so it never collides on a shared origin. */
export const STORAGE_PREFIX = 'mangaCleaner.'

/**
 * The real storage, or `null` when it is unavailable - Safari private mode
 * throws on *access*, not just on write, so even the property read is guarded.
 *
 * @returns {Storage|null}
 */
export function defaultStorage() {
  try {
    return globalThis.localStorage ?? null
  } catch {
    return null
  }
}

/**
 * Read a stored record. Anything that is not a JSON object - absent, corrupt,
 * a bare primitive, an array, `null` - yields `fallback`.
 *
 * @param {string} key - unprefixed
 * @param {T} fallback
 * @param {Storage|null} [store]
 * @returns {T|Record<string, unknown>}
 * @template T
 */
export function readRecord(key, fallback, store = defaultStorage()) {
  if (!store) return fallback
  let raw
  try {
    raw = store.getItem(STORAGE_PREFIX + key)
  } catch {
    return fallback
  }
  if (typeof raw !== 'string' || raw === '') return fallback
  let parsed
  try {
    parsed = JSON.parse(raw)
  } catch {
    return fallback
  }
  if (parsed === null || typeof parsed !== 'object' || Array.isArray(parsed)) return fallback
  return parsed
}

/**
 * Write a record. Quota exhaustion and private mode are not errors the
 * interface should ever surface - a preference that failed to persist is a
 * preference that reverts next launch, not a broken app.
 *
 * @param {string} key - unprefixed
 * @param {Record<string, unknown>} value
 * @param {Storage|null} [store]
 * @returns {boolean} whether it stuck
 */
export function writeRecord(key, value, store = defaultStorage()) {
  if (!store) return false
  try {
    store.setItem(STORAGE_PREFIX + key, JSON.stringify(value))
    return true
  } catch {
    return false
  }
}

/**
 * @param {string} key - unprefixed
 * @param {Storage|null} [store]
 * @returns {boolean}
 */
export function removeRecord(key, store = defaultStorage()) {
  if (!store) return false
  try {
    store.removeItem(STORAGE_PREFIX + key)
    return true
  } catch {
    return false
  }
}

/* ------------------------------------------------------------------ */
/* Validators                                                          */
/* ------------------------------------------------------------------ */

/**
 * @param {unknown} value
 * @param {readonly string[]} allowed
 * @param {string} fallback
 * @returns {string}
 */
export function oneOf(value, allowed, fallback) {
  return typeof value === 'string' && allowed.includes(value) ? value : fallback
}

/**
 * @param {unknown} value
 * @param {boolean} fallback
 * @returns {boolean}
 */
export function boolOr(value, fallback) {
  return typeof value === 'boolean' ? value : fallback
}

/**
 * Finite numbers are clamped into range rather than rejected: a zoom of 40
 * from a build with a wider range should become the current maximum, not
 * silently reset to the default. Non-finite and non-numeric yield `fallback`.
 *
 * @param {unknown} value
 * @param {{min: number, max: number, fallback: number}} spec
 * @returns {number}
 */
export function numberIn(value, { min, max, fallback }) {
  if (typeof value !== 'number' || !Number.isFinite(value)) return fallback
  return Math.min(max, Math.max(min, value))
}

/**
 * @param {unknown} value
 * @param {string|null} fallback
 * @returns {string|null}
 */
export function idOr(value, fallback) {
  return typeof value === 'string' && value !== '' ? value : fallback
}

/**
 * A list of ids, filtered to non-empty strings, deduped and capped. Caps
 * matter: a record grown unbounded across sessions is a slow leak.
 *
 * @param {unknown} value
 * @param {{max?: number}} [opts]
 * @returns {string[]}
 */
export function idList(value, { max = 500 } = {}) {
  if (!Array.isArray(value)) return []
  const seen = new Set()
  for (const item of value) {
    if (typeof item === 'string' && item !== '') seen.add(item)
    if (seen.size >= max) break
  }
  return [...seen]
}

/**
 * @param {unknown} value
 * @returns {Record<string, unknown>}
 */
export function plainObject(value) {
  return value !== null && typeof value === 'object' && !Array.isArray(value)
    ? /** @type {Record<string, unknown>} */ (value)
    : {}
}

/**
 * Keep a keyed map from growing without bound, oldest insertion first.
 *
 * @param {Record<string, T>} map
 * @param {number} max
 * @returns {Record<string, T>}
 * @template T
 */
export function capEntries(map, max) {
  const keys = Object.keys(map)
  if (keys.length <= max) return map
  const kept = {}
  for (const key of keys.slice(keys.length - max)) kept[key] = map[key]
  return kept
}
