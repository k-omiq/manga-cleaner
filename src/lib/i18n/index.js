/**
 * The string catalogue and the one function that reads it.
 *
 *   t(key: string, params?: Record<string, unknown>): string
 *
 * Every user-visible string in the app comes through here. A component holds no
 * English of its own - the catalogue is an
 * obligation, not a convenience, and the only way to keep one honest is for
 * there to be no second route to the screen.
 *
 * The contract, unchanged from the shim Task 5 wrote against:
 *
 * - `key` is `<namespace>.<category>.<camelCaseName>`.
 * - `params` are **named placeholders** (`{count}`, `{path}`), interpolated
 *   into the translated sentence. Never concatenation: word order belongs to
 *   the translator.
 * - **A param whose name ends in `Key` carries an i18n key** and is resolved
 *   with `t()` before it is interpolated. `{statusKey}`, `{reasonKey}`,
 *   `{rungKey}`, `{toolKey}`, `{fillModeKey}`, `{causeKey}`, `{modeKey}`,
 *   `{labelKey}`, `{commandKey}`, `{titleKey}`, `{windowName}` is *not* one.
 *
 * Two things the shim did not have, both demanded by strings the app already
 * writes:
 *
 * - **Formats.** `{cost:currency}` runs `Intl.NumberFormat`. This exists so
 *   that `masks.value.cloudCost` - the only money in the app - is formatted in
 *   one place and cannot be assembled with a `$` anywhere else.
 * - **Plurals.** A catalogue entry may be an object of plural forms rather than
 *   a string. See `selectForm`.
 *
 * And one thing neither had: **context params**, for the placeholder whose
 * call site cannot supply it. See `provideContextParam`; there is one, and the
 * note there says why it is not a concatenation instead.
 *
 * A missing key never renders `undefined`: in dev it renders the key itself and
 * shouts on the console, and in a build it degrades to the key's last segment
 * as words, which is legible if wrong.
 */

import { en } from './en.js'

/** The active locale. One catalogue ships today; see `settings.language.description`. */
export const LOCALE = 'en'

/** Every catalogue that exists, by locale tag. The Settings dialog's Language row reads this. */
export const CATALOGUES = Object.freeze({ en })

/* ------------------------------------------------------------------ */
/* Flattening                                                          */
/* ------------------------------------------------------------------ */

/**
 * `{masks: {value: {cloudCost: '…'}}}` → `{'masks.value.cloudCost': '…'}`.
 *
 * A plural-forms object is a **leaf**, not a branch: it is recognised by
 * carrying at least one of the CLDR category names, and is stored whole.
 *
 * @param {Record<string, any>} tree
 * @param {string} [prefix]
 * @param {Record<string, string|Record<string, string>>} [into]
 * @returns {Record<string, string|Record<string, string>>}
 */
export function flatten(tree, prefix = '', into = {}) {
  for (const [name, value] of Object.entries(tree)) {
    const key = prefix ? `${prefix}.${name}` : name
    if (typeof value === 'string' || isPluralForms(value)) into[key] = value
    else if (value && typeof value === 'object') flatten(value, key, into)
  }
  return into
}

const PLURAL_CATEGORIES = ['zero', 'one', 'two', 'few', 'many', 'other']

/** @param {unknown} value */
function isPluralForms(value) {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return false
  return PLURAL_CATEGORIES.some((category) => typeof value[category] === 'string')
}

/** The live catalogue, flattened once at module load. */
const CATALOGUE = flatten(en)

/** @returns {string[]} every key in the catalogue, sorted. For the completeness test. */
export function catalogueKeys() {
  return Object.keys(CATALOGUE).sort()
}

/** @param {string} key @returns {boolean} */
export function hasKey(key) {
  return Object.hasOwn(CATALOGUE, key)
}

/* ------------------------------------------------------------------ */
/* Plurals                                                             */
/* ------------------------------------------------------------------ */

const pluralRules = new Intl.PluralRules(LOCALE)

/**
 * Choose a plural form.
 *
 * The count comes from `params.count` unless the entry names another param
 * with `select` - `notice.run.finished` counts pages, `editor.readout.
 * reviewPending` counts a total, and renaming their params to `count` would
 * mean editing call sites in two other tasks' files to suit the catalogue.
 *
 * **`zero` is honoured even though English has no `zero` category.** It is the
 * reason plurals are here at all: `pages.row.name` is read out for every row in
 * the Pages list and `count` is 0 on nearly all of them, so "0 regions need
 * review" would be the sentence a screen-reader user hears most often in the
 * app. An entry that declares `zero` gets it at exactly 0; one that does not
 * falls through to the normal rules.
 *
 * @param {Record<string, string>} forms
 * @param {Record<string, unknown>} params
 * @returns {string}
 */
export function selectForm(forms, params) {
  const name = typeof forms.select === 'string' ? forms.select : 'count'
  const count = params?.[name]
  if (typeof count !== 'number' || !Number.isFinite(count)) return forms.other ?? ''
  if (count === 0 && typeof forms.zero === 'string') return forms.zero
  return forms[pluralRules.select(count)] ?? forms.other ?? ''
}

/* ------------------------------------------------------------------ */
/* Formats                                                             */
/* ------------------------------------------------------------------ */

const currency = new Intl.NumberFormat(LOCALE, {
  style: 'currency',
  currency: 'USD',
  minimumFractionDigits: 2,
  // Cloud pricing is quoted per request in thousandths
  // (NB2 @1K is $0.067). Rounding it to cents would print $0.07 and
  // overstate every estimate the cost dialog shows.
  maximumFractionDigits: 3,
})

/**
 * Bytes, as a person reads them: `{bytes:memory}`.
 *
 * Base 1024 and one decimal place at most, because every figure this formats is
 * an *estimate* of what a model is holding - a session's size is the weights
 * plus an allocator's arena, and printing `534,773,760 B` would dress a
 * rounded number up as a measured one. Under a megabyte it says `< 1 MB`
 * rather than a number of kilobytes: the tab is about hundreds of megabytes,
 * and a row reading `0.3 MB` invites arithmetic nobody wants to do.
 *
 * `Intl.NumberFormat` and not `toFixed`, so the decimal separator is the
 * locale's.
 *
 * @param {unknown} value
 */
function memory(value) {
  if (typeof value !== 'number' || !Number.isFinite(value) || value < 0) return ''
  const mb = value / (1024 * 1024)
  if (mb < 1) return '< 1 MB'
  const gb = mb / 1024
  const [amount, unit] = gb >= 1 ? [gb, 'GB'] : [mb, 'MB']
  const digits = amount < 10 ? 1 : 0
  const number = new Intl.NumberFormat(LOCALE, {
    minimumFractionDigits: 0,
    maximumFractionDigits: digits,
  }).format(amount)
  return `${number} ${unit}`
}

const FORMATS = {
  /** @param {unknown} value */
  currency: (value) => (typeof value === 'number' ? currency.format(value) : String(value ?? '')),
  memory,
}

/* ------------------------------------------------------------------ */
/* Interpolation                                                       */
/* ------------------------------------------------------------------ */

/** `{name}` or `{name:format}`. */
const PLACEHOLDER = /\{(\w+)(?::(\w+))?\}/g

/** A param whose *name* ends in `Key` carries a key, not a value. */
function isKeyParam(name) {
  return name.endsWith('Key')
}

/* ------------------------------------------------------------------ */
/* Context params                                                      */
/* ------------------------------------------------------------------ */

/**
 * A param a string needs that its **call site has no way to supply**.
 *
 * There is exactly one today and it is the reason this exists.
 * `tools.hint.cloneHeal` names the modifier held to pick Clone / heal's
 * source, and that modifier is now a setting - but the string is rendered by
 * `ToolWindow.svelte` as `t(spec.hintKey)`, over a key `src/lib/editor/
 * tools.js` chose, and neither of those two knows or should know about a
 * preference. The alternatives were worse in kind: a second English sentence
 * assembled outside the catalogue, or a key per modifier chosen by a table
 * that would have to live in the editor.
 *
 * The direction of the dependency is the point. `i18n` imports nothing and
 * still does: a provider is **pushed in** by whoever owns the value - see
 * `src/lib/state/session.svelte.js` - so the catalogue stays a catalogue.
 *
 * A getter rather than a value, because the value changes while the app runs.
 * Read during interpolation, which happens inside whatever effect called
 * `t()`, so a provider that reads a rune makes every string using it redraw
 * when the setting moves - which is the whole behaviour asked for.
 *
 * An unprovided placeholder is untouched, exactly as before: `{modifier}` on
 * screen is a caller who forgot, and that is still the report.
 *
 * @type {Record<string, () => unknown>}
 */
const CONTEXT = {}

/**
 * Supply a context param. Idempotent - a second call replaces the getter - so
 * a module that registers at import time is safe to import twice.
 *
 * @param {string} name
 * @param {() => unknown} get
 */
export function provideContextParam(name, get) {
  CONTEXT[name] = get
}

/**
 * @param {string} template
 * @param {Record<string, unknown>} params
 * @returns {string}
 */
function interpolate(template, params) {
  return template.replace(PLACEHOLDER, (whole, name, format) => {
    if (!Object.hasOwn(params, name)) {
      const get = CONTEXT[name]
      if (!get) return whole
      const supplied = get()
      return supplied === undefined || supplied === null ? '' : String(supplied)
    }
    const value = params[name]
    if (value === undefined || value === null) return ''
    if (format) return FORMATS[format] ? FORMATS[format](value) : String(value)
    if (isKeyParam(name) && typeof value === 'string') return t(value)
    return String(value)
  })
}

/* ------------------------------------------------------------------ */
/* Missing keys                                                        */
/* ------------------------------------------------------------------ */

const DEV = Boolean(import.meta.env?.DEV)

/** Shout once per key, not once per render - a missing key in a list is one bug. */
const shouted = new Set()

/**
 * `pages.status.cleanedNeedsReview` → `Cleaned needs review`. The production
 * fallback: wrong, but legible and never `undefined`.
 *
 * @param {string} key
 * @returns {string}
 */
export function humanise(key) {
  const last = key.slice(key.lastIndexOf('.') + 1)
  const words = last
    .replace(/([a-z0-9])([A-Z])/g, '$1 $2')
    .replace(/[-_]+/g, ' ')
    .toLowerCase()
    .trim()
  return words.charAt(0).toUpperCase() + words.slice(1)
}

/** @param {string} key */
function missing(key) {
  if (DEV) {
    if (!shouted.has(key)) {
      shouted.add(key)
      console.error(`i18n: no catalogue entry for "${key}"`)
    }
    // Loud on purpose: the raw key on screen is unmistakable in a browser pass.
    return `[${key}]`
  }
  return humanise(key)
}

/* ------------------------------------------------------------------ */
/* t                                                                   */
/* ------------------------------------------------------------------ */

/**
 * Translate a key. See the module header for the contract.
 *
 * @param {string} key
 * @param {Record<string, unknown>} [params]
 * @returns {string}
 */
export function t(key, params) {
  if (typeof key !== 'string' || key === '') return ''
  const entry = CATALOGUE[key]
  if (entry === undefined) return missing(key)

  const template = typeof entry === 'string' ? entry : selectForm(entry, params ?? {})
  // Always interpolated, even with no params: a context param is supplied by
  // whoever owns the value rather than by the call site (see `CONTEXT`), and a
  // template with neither is returned unchanged by the same pass.
  return interpolate(template, params ?? {})
}
