/**
 * Pure color conversion, normalization and formatting helpers.
 *
 * All functions operate on standard web color spaces:
 * - HSB/HSV: H in [0, 360), S in [0, 100], B in [0, 100]
 * - RGB: R, G, B in [0, 255]
 * - HEX: lowercase #rrggbb (with support for parsing 3-digit shorthand #rgb)
 */

/**
 * Clamp a number to [min, max].
 * @param {number} value
 * @param {number} min
 * @param {number} max
 * @returns {number}
 */
export function clamp(value, min, max) {
  return Math.max(min, Math.min(max, value))
}

/**
 * Convert HSB (Hue 0-360, Saturation 0-100, Brightness 0-100) to RGB (0-255).
 * @param {number} h
 * @param {number} s
 * @param {number} b
 * @returns {{ r: number, g: number, b: number }}
 */
export function hsbToRgb(h, s, b) {
  const normH = (((h % 360) + 360) % 360)
  const normS = clamp(s, 0, 100) / 100
  const normB = clamp(b, 0, 100) / 100

  const c = normB * normS
  const x = c * (1 - Math.abs(((normH / 60) % 2) - 1))
  const m = normB - c

  let r1 = 0
  let g1 = 0
  let b1 = 0

  if (normH >= 0 && normH < 60) {
    r1 = c
    g1 = x
    b1 = 0
  } else if (normH >= 60 && normH < 120) {
    r1 = x
    g1 = c
    b1 = 0
  } else if (normH >= 120 && normH < 180) {
    r1 = 0
    g1 = c
    b1 = x
  } else if (normH >= 180 && normH < 240) {
    r1 = 0
    g1 = x
    b1 = c
  } else if (normH >= 240 && normH < 300) {
    r1 = x
    g1 = 0
    b1 = c
  } else {
    r1 = c
    g1 = 0
    b1 = x
  }

  return {
    r: Math.round((r1 + m) * 255),
    g: Math.round((g1 + m) * 255),
    b: Math.round((b1 + m) * 255),
  }
}

/**
 * Convert RGB (0-255) to HSB (Hue 0-360, Saturation 0-100, Brightness 0-100).
 * @param {number} r
 * @param {number} g
 * @param {number} b
 * @returns {{ h: number, s: number, b: number }}
 */
export function rgbToHsb(r, g, b) {
  const normR = clamp(r, 0, 255) / 255
  const normG = clamp(g, 0, 255) / 255
  const normB = clamp(b, 0, 255) / 255

  const max = Math.max(normR, normG, normB)
  const min = Math.min(normR, normG, normB)
  const delta = max - min

  let h = 0
  if (delta !== 0) {
    if (max === normR) {
      h = 60 * (((normG - normB) / delta) % 6)
    } else if (max === normG) {
      h = 60 * ((normB - normR) / delta + 2)
    } else {
      h = 60 * ((normR - normG) / delta + 4)
    }
  }

  h = Math.round(((h % 360) + 360) % 360)
  if (h === 360) h = 0

  const s = max === 0 ? 0 : Math.round((delta / max) * 100)
  const bVal = Math.round(max * 100)

  return { h, s, b: bVal }
}

/**
 * Convert RGB (0-255) to a lowercase 6-digit hex string (#rrggbb).
 * @param {number} r
 * @param {number} g
 * @param {number} b
 * @returns {string}
 */
export function rgbToHex(r, g, b) {
  const clampByte = (v) => clamp(Math.round(v), 0, 255)
  const hexR = clampByte(r).toString(16).padStart(2, '0')
  const hexG = clampByte(g).toString(16).padStart(2, '0')
  const hexB = clampByte(b).toString(16).padStart(2, '0')
  return `#${hexR}${hexG}${hexB}`.toLowerCase()
}

/**
 * Parse a hex string to RGB (0-255) components, supporting 3 or 6 hex digits.
 * @param {string} hex
 * @returns {{ r: number, g: number, b: number }|null}
 */
export function hexToRgb(hex) {
  const raw = String(hex ?? '').trim().replace(/^#/, '')
  if (/^[0-9a-f]{6}$/i.test(raw)) {
    return {
      r: parseInt(raw.slice(0, 2), 16),
      g: parseInt(raw.slice(2, 4), 16),
      b: parseInt(raw.slice(4, 6), 16),
    }
  }
  if (/^[0-9a-f]{3}$/i.test(raw)) {
    return {
      r: parseInt(raw[0] + raw[0], 16),
      g: parseInt(raw[1] + raw[1], 16),
      b: parseInt(raw[2] + raw[2], 16),
    }
  }
  return null
}

/**
 * Convert a hex string to HSB.
 * @param {string} hex
 * @returns {{ h: number, s: number, b: number }|null}
 */
export function hexToHsb(hex) {
  const rgb = hexToRgb(hex)
  if (!rgb) return null
  return rgbToHsb(rgb.r, rgb.g, rgb.b)
}

/**
 * Convert HSB directly to a lowercase hex string.
 * @param {number} h
 * @param {number} s
 * @param {number} b
 * @returns {string}
 */
export function hsbToHex(h, s, b) {
  const rgb = hsbToRgb(h, s, b)
  return rgbToHex(rgb.r, rgb.g, rgb.b)
}

/**
 * Validate and format hex color text during user input.
 * Accepts exact 6 hex digits (with optional leading #).
 * @param {string} text
 * @returns {string|null}
 */
export function hexOnInput(text) {
  const raw = String(text ?? '').trim().replace(/^#/, '')
  return /^[0-9a-f]{6}$/i.test(raw) ? `#${raw.toLowerCase()}` : null
}

/**
 * Validate and format hex color text on commit (blur or Enter).
 * Accepts 6-digit hex or 3-digit shorthand.
 * @param {string} text
 * @returns {string|null}
 */
export function hexOnCommit(text) {
  const raw = String(text ?? '').trim().replace(/^#/, '')
  if (/^[0-9a-f]{6}$/i.test(raw)) {
    return `#${raw.toLowerCase()}`
  }
  if (/^[0-9a-f]{3}$/i.test(raw)) {
    return `#${[...raw].map((d) => d + d).join('').toLowerCase()}`
  }
  return null
}

/**
 * Check if the text in a hex field cannot become a valid color.
 * @param {string} text
 * @returns {boolean}
 */
export function hexInvalid(text) {
  const raw = String(text ?? '').trim()
  if (raw === '') return false
  return hexOnCommit(raw) === null
}
