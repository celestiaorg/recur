/** Types for the compiled file-icons/atom icon database. */

export type IconColor = [string | null, string | null]

export type IconBase = [
  /** CSS class (e.g. "js-icon") */
  string,
  /** Colour classes: [light, dark] */
  IconColor,
  /** Basename / path match */
  RegExp,
  number?,
  boolean?,
  RegExp?,
  RegExp?,
  RegExp?,
  RegExp?,
]

export type IconDbItem = [IconBase[], number[][], Record<string, number>]

export type IconDb = [IconDbItem, IconDbItem]

export type ColorMode = 'light' | 'dark' | 'mono'

export type ColorModeIndex = 0 | 1 | null
