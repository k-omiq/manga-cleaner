/**
 * The engine ladder: planar fill → denoise →
 * manga-LaMa → cloud, ordered weakest/cheapest to strongest/most expensive. `stronger`/`simpler` step along it and clamp at
 * both ends - there is no engine below planar fill or above cloud.
 *
 * `decline` is a terminal pipeline outcome, not a
 * rung to escalate to or from; it lives on `Region.outcome`, not here.
 */

/** @type {ReadonlyArray<string>} */
export const RUNGS = Object.freeze(['fill', 'denoise', 'lama', 'cloud'])

const LABEL_KEYS = Object.freeze({
  fill: 'ladder.rung.fill',
  denoise: 'ladder.rung.denoise',
  lama: 'ladder.rung.lama',
  flux: 'ladder.rung.flux',
  cloud: 'ladder.rung.cloud',
  paint: 'ladder.rung.paint',
  clone: 'ladder.rung.clone',
})

/**
 * @param {string} rung
 * @returns {string} the rung one step stronger, clamped at the top ('cloud')
 */
export function stronger(rung) {
  const i = RUNGS.indexOf(rung)
  if (i === -1) return rung
  return RUNGS[Math.min(i + 1, RUNGS.length - 1)]
}

/**
 * @param {string} rung
 * @returns {string} the rung one step simpler, clamped at the bottom ('fill')
 */
export function simpler(rung) {
  const i = RUNGS.indexOf(rung)
  if (i === -1) return rung
  return RUNGS[Math.max(i - 1, 0)]
}

/**
 * @param {string} rung
 * @returns {string} i18n key for the rung's display name
 */
export function rungLabel(rung) {
  return LABEL_KEYS[rung] ?? 'ladder.rung.unknown'
}
