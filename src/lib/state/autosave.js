/**
 * Autosave of the editor's position: reopening a chapter
 * restores the page, the scroll position, the zoom and the user's place in the
 * review set.
 *
 * One record per chapter, keyed by chapter id, all under a single storage key.
 * Plain JavaScript - the editor state module owns *when* to save; this module
 * owns *what a record is* and how it survives a corrupt or stale store.
 *
 * Deliberately **not** stored: the pending review set itself. It is derived
 * from the chapter, so it is recomputed on open - cheaper than a stale copy and
 * incapable of disagreeing with the data. What is stored is the cursor within
 * it, which is genuine session state the user would otherwise lose.
 */

import {
  readRecord,
  writeRecord,
  numberIn,
  boolOr,
  idOr,
  plainObject,
  capEntries,
} from './persist.js'

const STORAGE_KEY = 'autosave.v1'

/** Old chapters fall off rather than accumulate forever. */
const MAX_CHAPTERS = 60

/**
 * @typedef {Object} AutosaveRecord
 * @property {number} pageIndex
 * @property {number} scrollTop
 * @property {number} scrollLeft
 * @property {number} zoom
 * @property {boolean} fit
 * @property {boolean} reviewFilter
 * @property {string|null} reviewCurrentId - the review entry the user was on
 */

/**
 * Coerce anything into a usable record. Exported for testing.
 *
 * @param {unknown} raw
 * @param {{minZoom: number, maxZoom: number}} zoomRange
 * @returns {AutosaveRecord}
 */
export function sanitizeAutosave(raw, { minZoom, maxZoom }) {
  const record = plainObject(raw)
  return {
    pageIndex: numberIn(record.pageIndex, { min: 0, max: 1e6, fallback: 0 }),
    scrollTop: numberIn(record.scrollTop, { min: 0, max: 1e7, fallback: 0 }),
    scrollLeft: numberIn(record.scrollLeft, { min: 0, max: 1e7, fallback: 0 }),
    zoom: numberIn(record.zoom, { min: minZoom, max: maxZoom, fallback: 1 }),
    fit: boolOr(record.fit, true),
    reviewFilter: boolOr(record.reviewFilter, false),
    reviewCurrentId: idOr(record.reviewCurrentId, null),
  }
}

/**
 * @param {string} chapterId
 * @param {{minZoom: number, maxZoom: number}} zoomRange
 * @returns {AutosaveRecord}
 */
export function loadAutosave(chapterId, zoomRange) {
  const all = plainObject(readRecord(STORAGE_KEY, {}))
  return sanitizeAutosave(all[chapterId], zoomRange)
}

/**
 * @param {string} chapterId
 * @param {AutosaveRecord} record
 */
export function storeAutosave(chapterId, record) {
  const all = plainObject(readRecord(STORAGE_KEY, {}))
  all[chapterId] = record
  writeRecord(STORAGE_KEY, capEntries(all, MAX_CHAPTERS))
}
