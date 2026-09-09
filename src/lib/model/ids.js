/**
 * Deterministic id generation for the model and its consumers (tests,
 * Task 3's mock engine). No `Math.random`, no `Date.now` - each factory
 * owns its own counter, seedable at construction so callers get
 * reproducible ids across runs.
 */

/**
 * @param {string} [prefix] - id prefix, e.g. 'page' -> 'page-1', 'page-2', ...
 * @param {number} [start] - first counter value; seed this for deterministic tests
 * @returns {() => string} a function that returns a new monotonic id on every call
 */
export function createIdFactory(prefix = 'id', start = 1) {
  let next = start
  return () => `${prefix}-${next++}`
}
