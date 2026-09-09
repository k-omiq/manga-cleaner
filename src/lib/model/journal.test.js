import { describe, expect, it } from 'vitest'
import {
  JOURNAL_LIMIT,
  createJournal,
  pushEntry,
  redoEntry,
  undoEntry,
  viewOf,
} from './journal.js'

/*
 * These are the same assertions `history::tests` makes in Rust, against the
 * same three rules. Two implementations run in two processes - the core owns a
 * file, the mock owns a fixture library with no file - and the point of writing
 * the tests twice is that the rules are the contract, not the storage.
 */

const delta = (label, regionId = 'r1') => ({
  label,
  op: 'region-state',
  regionId,
  before: { present: true, pageStatus: 'cleaned', region: { id: regionId } },
  after: { present: false, pageStatus: 'unclean', region: null },
})

describe('journal', () => {
  it('numbers an entry and puts the cursor at the end', () => {
    const j = createJournal()
    expect(pushEntry(j, delta('a')).seq).toBe(1)
    expect(pushEntry(j, delta('b')).seq).toBe(2)
    expect(j.cursor).toBe(2)
  })

  it('the view carries labels and no payload', () => {
    const j = createJournal()
    pushEntry(j, delta('masks.command.deleteMask'))
    const view = viewOf(j)
    expect(view).toEqual({ cursor: 1, entries: [{ seq: 1, label: 'masks.command.deleteMask' }] })
    expect(JSON.stringify(view)).not.toContain('region-state')
  })

  it('undo and redo walk the same entries', () => {
    const j = createJournal()
    pushEntry(j, delta('a'))
    pushEntry(j, delta('b'))

    expect(undoEntry(j).label).toBe('b')
    expect(undoEntry(j).label).toBe('a')
    expect(undoEntry(j)).toBe(null)
    expect(j.cursor).toBe(0)

    expect(redoEntry(j).label).toBe('a')
    expect(redoEntry(j).label).toBe('b')
    expect(redoEntry(j)).toBe(null)
  })

  it('a push after an undo drops the future', () => {
    const j = createJournal()
    pushEntry(j, delta('a'))
    pushEntry(j, delta('b'))
    undoEntry(j)
    pushEntry(j, delta('c'))

    expect(j.entries.map((e) => e.label)).toEqual(['a', 'c'])
    expect(j.cursor).toBe(2)
    expect(redoEntry(j)).toBe(null)
  })

  it('is capped, oldest out, and never reissues a number', () => {
    const j = createJournal()
    for (let n = 0; n < JOURNAL_LIMIT + 10; n += 1) pushEntry(j, delta(`e${n}`))

    expect(j.entries).toHaveLength(JOURNAL_LIMIT)
    expect(j.cursor).toBe(JOURNAL_LIMIT)
    expect(j.entries[0].label).toBe('e10')
    expect(j.nextSeq).toBe(JOURNAL_LIMIT + 11)
    // No number came back: the last entry's seq is still its position in the
    // whole history, not in the window that survived.
    expect(j.entries.at(-1).seq).toBe(JOURNAL_LIMIT + 10)
  })

  it('a delta names a mask by reference and carries no pixels', () => {
    const j = createJournal()
    const entry = pushEntry(j, delta('masks.command.deleteMask'))
    expect(entry.before.region).toEqual({ id: 'r1' })
    expect(entry.op).toBe('region-state')
    // The whole entry is small - this is the property the cap is sized against.
    expect(JSON.stringify(entry).length).toBeLessThan(400)
  })
})
