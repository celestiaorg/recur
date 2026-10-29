

/** Glyph form for palette / menus (⇧⌘U). */
export function formatKeyEquivalentDisplay(canonical: string): string {
  if (!canonical) return ''
  let out = ''
  for (const p of canonical.split('+')) {
    switch (p) {
      case 'cmd': out += '⌘'; break
      case 'opt': out += '⌥'; break
      case 'shift': out += '⇧'; break
      case 'ctrl': out += '⌃'; break
      case 'fn': out += 'fn'; break
      case 'escape': out += 'esc'; break
      case 'enter': out += '↵'; break
      case 'backspace': out += '⌫'; break
      case 'delete': out += '⌦'; break
      case 'up': out += '↑'; break
      case 'down': out += '↓'; break
      case 'left': out += '←'; break
      case 'right': out += '→'; break
      case 'tab': out += '⇥'; break
      case 'space': out += '␣'; break
      default:
        out += p.length === 1 || /^f\d{1,2}$/.test(p) ? p.toUpperCase() : p
    }
  }
  return out
}
