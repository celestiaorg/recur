/**
 * Apple Generic RGB → sRGB, same matrix / gamma as jibsen/tmcolorconv.
 * Used at port time (`convert-tmtheme.mjs`); Recur paints the baked sRGB hex.
 */
/** @type {number[][]} */
const GENERIC_RGB_TO_SRGB = [
  [1.0252482, -0.0265428, 0.0012946],
  [0.019397, 0.9480316, 0.0325714],
  [-0.0017702, -0.0014426, 1.0032128],
]

/** TextMate default Generic RGB gamma (`461/256`, not the rounded 1.8). */
const GENERIC_RGB_GAMMA = 461 / 256

/** @param {string | undefined} name */
export function isGenericRgbColorSpace(name) {
  if (!name) return false
  const n = String(name).trim().toLowerCase()
  return n === 'generic rgb' || n === 'apple generic rgb'
}

/** @param {number} c */
function srgbCompand(c) {
  if (c <= 0.0031308) return 12.92 * c
  return 1.055 * Math.pow(c, 1 / 2.4) - 0.055
}

/** @param {string} rgb */
function expandHex3(rgb) {
  return `${rgb[0]}${rgb[0]}${rgb[1]}${rgb[1]}${rgb[2]}${rgb[2]}`
}

/** Convert one `#rgb` / `#rrggbb` / `#rrggbbaa` Generic RGB hex to sRGB. Alpha is kept.
 * @param {string} hex
 */
export function genericRgbHexToSrgb(hex) {
  const raw = String(hex).trim()
  const m = raw.match(/^#([0-9a-f]{3,8})$/i)
  if (!m) return hex
  let body = m[1]
  let alpha = ''
  if (body.length === 3) {
    body = expandHex3(body)
  } else if (body.length === 4) {
    alpha = `${body[3]}${body[3]}`
    body = expandHex3(body.slice(0, 3))
  } else if (body.length === 8) {
    alpha = body.slice(6)
    body = body.slice(0, 6)
  } else if (body.length !== 6) {
    return hex
  }
  const vin = [0, 2, 4].map(/** @param {number} i */ (i) => parseInt(body.slice(i, i + 2), 16) / 255)
  const linear = vin.map(/** @param {number} c */ (c) => Math.pow(c, GENERIC_RGB_GAMMA))
  const srgb = GENERIC_RGB_TO_SRGB.map(
    /** @param {number[]} row */ (row) => row[0] * linear[0] + row[1] * linear[1] + row[2] * linear[2],
  )
  const rgb = srgb
    .map(/** @param {number} c */ (c) => {
      const n = Math.round(srgbCompand(c) * 255)
      return Math.max(0, Math.min(255, n)).toString(16).padStart(2, '0')
    })
    .join('')
  return `#${rgb}${alpha}`.toUpperCase()
}

/** @param {Record<string, unknown> | null | undefined} preview */
export function convertPreview(preview) {
  if (!preview || typeof preview !== 'object') return preview
  /** @type {Record<string, unknown>} */
  const out = {}
  for (const [key, value] of Object.entries(preview)) {
    out[key] = typeof value === 'string' ? genericRgbHexToSrgb(value) : value
  }
  return out
}

const HEX_COLOR_RE = /^#([0-9a-f]{3}|[0-9a-f]{4}|[0-9a-f]{6}|[0-9a-f]{8})$/i

/** @param {unknown} value */
function convertMaybeHex(value) {
  if (typeof value !== 'string' || !HEX_COLOR_RE.test(value.trim())) return value
  return genericRgbHexToSrgb(value)
}

/** @param {Record<string, unknown> | null | undefined} settings */
function convertSettingsHex(settings) {
  if (!settings || typeof settings !== 'object') return
  for (const [key, value] of Object.entries(settings)) {
    settings[key] = convertMaybeHex(value)
  }
}

/** Rewrite every hex in a VS Code-shaped theme from Generic RGB to sRGB (in place).
 * @param {any} json
 */
export function convertThemeJson(json) {
  if (json.colors) {
    for (const [key, value] of Object.entries(json.colors)) {
      json.colors[key] = convertMaybeHex(value)
    }
  }
  if (json.gutterSettings) convertSettingsHex(json.gutterSettings)
  for (const entry of json.tokenColors ?? []) convertSettingsHex(entry.settings)
  for (const entry of json.settings ?? []) convertSettingsHex(entry.settings)
  return json
}

/** Convert a Generic RGB theme JSON to sRGB in place and stamp `colorSpaceName`.
 * No-op when the file is already sRGB (or has no Generic RGB stamp).
 * @param {any} json
 */
export function bakeGenericRgbThemeToSrgb(json) {
  if (!isGenericRgbColorSpace(json.colorSpaceName)) return false
  convertThemeJson(json)
  json.colorSpaceName = 'sRGB'
  return true
}
