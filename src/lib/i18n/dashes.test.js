/**
 * No em-dash or en-dash in anything the user reads.
 *
 * A house rule about the prose itself rather than about the machinery: the
 * copy joins its clauses with a full stop, a colon, a comma or a middle dot,
 * never with an em-dash or an en-dash. It was asked for more than once and kept coming back,
 * one row at a time, which is what a test is for.
 *
 * Two surfaces, because the app writes user-visible text in two places:
 *
 * 1. **The catalogue.** Every string value in `en`, walked recursively so the
 *    plural-form objects (`{zero, one, other}`) are covered as strings and not
 *    skipped as containers. Comments in `en.js` are prose about the copy and
 *    are not user-visible, so they are not scanned here.
 * 2. **The dialogs.** `src/lib/dialogs/*.svelte` build a little text inline -
 *    a label joined to a status, a reason appended to a name - and those joins
 *    never reach the catalogue. The source is read with the comments stripped
 *    by `keyscan.js#stripComments`, the same strip the key scan uses, because
 *    the doc comments in these files are written in ordinary prose and use
 *    em-dashes freely.
 *
 * The dialog half is a source scan, so it is a heuristic: a dash inside a
 * string that a component only ever passes to `t()` would fail it too. That is
 * the safe direction - the fix is to write the join without a dash either way.
 */

import { readFileSync, readdirSync, statSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import { dirname, join } from 'node:path'
import { describe, expect, it } from 'vitest'

import { en } from './en.js'
import { stripComments } from './keyscan.js'

const HERE = dirname(fileURLToPath(import.meta.url))
const SRC = join(HERE, '../..')

/**
 * Every `.svelte` file under `src`, recursively: the dialogs were where the
 * dashes were first found, but the Pages list, the review pill and the update
 * dialog each built one of their own.
 *
 * @param {string} dir
 * @returns {string[]}
 */
function svelteFiles(dir) {
  const out = []
  for (const name of readdirSync(dir)) {
    const path = join(dir, name)
    if (statSync(path).isDirectory()) out.push(...svelteFiles(path))
    else if (name.endsWith('.svelte')) out.push(path)
  }
  return out
}

/** U+2014 (em) and U+2013 (en). A hyphen is fine; these two are not. */
const DASH = /[\u2014\u2013]/

/**
 * Every string value in the catalogue, as `key → value`.
 *
 * @param {Record<string, unknown>} node
 * @param {string} [prefix]
 * @returns {Array<[string, string]>}
 */
function stringValues(node, prefix = '') {
  /** @type {Array<[string, string]>} */
  const out = []
  for (const [name, value] of Object.entries(node)) {
    const key = prefix ? `${prefix}.${name}` : name
    if (typeof value === 'string') out.push([key, value])
    else if (value && typeof value === 'object')
      out.push(...stringValues(/** @type {Record<string, unknown>} */ (value), key))
  }
  return out
}

const VALUES = stringValues(en)

describe('the copy carries no em-dash or en-dash', () => {
  it('walks the whole catalogue, plural forms included', () => {
    // A silent zero would make the assertion below pass vacuously, and the
    // plural objects are the half most easily walked past.
    expect(VALUES.length).toBeGreaterThan(400)
    expect(VALUES.map(([key]) => key)).toContain('export.note.gutter.other')
  })

  it('has no dash in any catalogue string', () => {
    const offenders = VALUES.filter(([, value]) => DASH.test(value)).map(
      ([key, value]) => `  ${key}   ${value}`,
    )
    expect(offenders, `dashes in the catalogue:\n${offenders.join('\n')}`).toEqual([])
  })

  it('has no dash in the text any component builds itself', () => {
    const files = svelteFiles(SRC)
    expect(files.length).toBeGreaterThan(5)

    /** @type {string[]} */
    const offenders = []
    for (const name of files) {
      // `stripComments` collapses a block comment to a single space, so the
      // line numbers no longer line up with the file. The line itself is
      // enough to find: it is quoted here rather than located.
      const source = stripComments(readFileSync(name, 'utf8'))
      for (const line of source.split('\n')) {
        if (DASH.test(line)) offenders.push(`  ${name}   ${line.trim()}`)
      }
    }
    expect(offenders, `dashes in dialog source:\n${offenders.join('\n')}`).toEqual([])
  })
})
