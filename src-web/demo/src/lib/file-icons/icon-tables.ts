import Icon from './icon'
import type { IconDb, IconDbItem } from './types'

type IconMap = Record<string, Icon>

interface IconTable {
  byName: Icon[]
  byPath: Icon[]
}

const cache = {
  directoryName: {} as IconMap,
  fileName: {} as IconMap,
}

/**
 * Name → icon lookup over the compiled file-icons/atom database.
 * Ported from upstream lib/icons/icon-tables.js (MIT).
 */
export default class IconTables {
  directoryIcons: IconTable
  fileIcons: IconTable

  constructor(data: IconDb) {
    const read = (table: IconDbItem): IconTable => {
      const [rawIcons, rawIndexes] = table
      const icons = rawIcons.map((icon, index) => new Icon(index, icon))
      const indexes = rawIndexes.map((index) => index.map((offset) => icons[offset]))
      return {
        byName: icons,
        byPath: indexes[2] ?? [],
      }
    }
    this.directoryIcons = read(data[0])
    this.fileIcons = read(data[1])
  }

  matchName(name: string, isDirectory = false): Icon | null {
    const cachedIcons = isDirectory ? cache.directoryName : cache.fileName
    const icons = isDirectory ? this.directoryIcons.byName : this.fileIcons.byName
    const hit = cachedIcons[name]
    if (hit) return hit
    for (let i = 0; i < icons.length; i++) {
      const icon = icons[i]
      if (icon.match.test(name)) {
        cachedIcons[name] = icon
        return icon
      }
    }
    return null
  }
}
