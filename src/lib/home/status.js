/**
 * The typographic mark that goes with a rollup status. The word from
 * `progress.status.*` is always shown next to it - the mark is a second
 * channel, never the only one, and never a colour on its own.
 *
 * Glyphs are the design file's: ✓ completed, △ needs review, ● in progress,
 * · untouched. No emoji, nothing that needs a font we do not ship.
 */

/** @type {Record<string, {glyph: string, dim: boolean}>} */
const MARKS = {
  completed: { glyph: '✓', dim: false },
  review: { glyph: '△', dim: false },
  inProgress: { glyph: '●', dim: false },
  notStarted: { glyph: '·', dim: true },
}

/**
 * @param {'notStarted'|'inProgress'|'review'|'completed'} status
 * @returns {{glyph: string, dim: boolean}}
 */
export function statusMark(status) {
  return MARKS[status] ?? MARKS.notStarted
}
