import type { ColorModeIndex, IconBase, IconColor } from './types'

/** One icon definition from the compiled file-icons/atom database. */
export default class Icon {
  index: number
  icon: string
  color: IconColor
  match: RegExp
  priority = 1
  matchPath = false
  interpreter: RegExp | null = null
  scope: RegExp | null = null
  lang: RegExp | null = null
  signature: RegExp | null = null

  constructor(index: number, data: IconBase) {
    this.index = index
    ;[this.icon, this.color, this.match] = data
    this.priority = data[3] || 1
    this.matchPath = data[4] || false
    this.interpreter = data[5] || null
    this.scope = data[6] || null
    this.lang = data[7] || null
    this.signature = data[8] || null
  }

  getClass(colorMode: ColorModeIndex = null): string {
    if (colorMode === null || !this.color[colorMode]) return this.icon
    return `${this.icon} ${this.color[colorMode]}`
  }
}
