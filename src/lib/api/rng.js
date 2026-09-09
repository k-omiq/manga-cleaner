/**
 * Deterministic pseudo-randomness for `src/lib/api`.
 *
 * Fixture construction must produce byte-identical data on every launch, so
 * nothing here reads the clock or `Math.random`. A seed is a string (usually
 * an id, so a chapter's pages are stable no matter what else was built first)
 * or a number.
 *
 * The generator is a plain 32-bit LCG. It is not statistically good and does
 * not need to be - it only has to be stable and cheap.
 */

/**
 * FNV-1a over a string. Stable across launches, platforms and engines.
 *
 * @param {string} text
 * @returns {number} unsigned 32-bit hash
 */
export function hashString(text) {
  let hash = 2166136261
  for (let i = 0; i < text.length; i += 1) {
    hash ^= text.charCodeAt(i)
    hash = Math.imul(hash, 16777619)
  }
  return hash >>> 0
}

/**
 * @typedef {Object} Rng
 * @property {() => number} next - next float in [0, 1)
 * @property {(min: number, max: number) => number} int - inclusive integer in [min, max]
 * @property {(items: readonly any[]) => any} pick - one item, uniformly
 * @property {() => string} sha256 - a stable 64-character lowercase hex string
 */

/**
 * @param {string|number} seed
 * @returns {Rng}
 */
export function createRng(seed) {
  let state = (typeof seed === 'string' ? hashString(seed) : seed >>> 0) || 1

  const next = () => {
    state = (Math.imul(state, 1103515245) + 12345) >>> 0
    return state / 4294967296
  }

  const int = (min, max) => min + Math.floor(next() * (max - min + 1))

  return {
    next,
    int,
    pick: (items) => items[Math.floor(next() * items.length)],
    sha256: () => {
      let out = ''
      for (let i = 0; i < 8; i += 1) {
        out += ((next() * 0xffffffff) >>> 0).toString(16).padStart(8, '0')
      }
      return out
    },
  }
}
