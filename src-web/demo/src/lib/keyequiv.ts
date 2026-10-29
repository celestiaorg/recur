/** Bundle keyEquivalent parsing (macOS). Twin of internal/bundles/keyequiv.go. */

const MOD_ORDER = ['ctrl', 'opt', 'shift', 'cmd', 'fn'] as const

const KNOWN_MODS: Record<string, string> = {
  ctrl: 'ctrl',
  control: 'ctrl',
  opt: 'opt',
  option: 'opt',
  alt: 'opt',
  shift: 'shift',
  cmd: 'cmd',
  command: 'cmd',
  meta: 'cmd',
  fn: 'fn',
  function: 'fn',
}

const KEY_ALIASES: Record<string, string> = {
  esc: 'escape',
  return: 'enter',
  ret: 'enter',
  bs: 'backspace',
  del: 'delete',
  pgup: 'pageup',
  pgdn: 'pagedown',
  arrowup: 'up',
  arrowdown: 'down',
  arrowleft: 'left',
  arrowright: 'right',
  spacebar: 'space',
  plus: '+',
  minus: '-',
  period: '.',
  comma: ',',
  slash: '/',
  backslash: '\\',
  backtick: '`',
  grave: '`',
  bracketleft: '[',
  bracketright: ']',
  semicolon: ';',
  quote: "'",
  equal: '=',
}

const KNOWN_KEYS = new Set([
  ...'abcdefghijklmnopqrstuvwxyz'.split(''),
  ...'0123456789'.split(''),
  ...Array.from({ length: 12 }, (_, i) => `f${i + 1}`),
  'enter', 'escape', 'tab', 'space',
  'up', 'down', 'left', 'right',
  'home', 'end', 'pageup', 'pagedown',
  'delete', 'backspace',
  '+', '-', '=', '.', ',', '/', '\\', '`', '[', ']', ';', "'",
  '<', '>',
])

/** Reserved app chords (canonical). Keep in sync with internal/bundles/keyequiv.go. */
export const RESERVED_APP_CHORDS = new Set([
  'shift+cmd+/',
  'cmd+k',
  'shift+cmd+p',
  'cmd+n',
  'cmd+b',
  'cmd+,',
  'escape',
  'cmd+p',
  'cmd+o',
  'shift+cmd+o',
  'cmd+1',
  'cmd+2',
  'cmd+3',
  'cmd+l',
  'opt+cmd+left',
  'opt+cmd+right',
  'opt+cmd+up',
  'opt+cmd+down',
  'opt+cmd+b',
  'opt+shift+cmd+b',
  'ctrl+`',
  'ctrl+shift+`',
  'cmd+t',
  'cmd+w',
  'shift+cmd+t',
  'shift+cmd+backspace',
  'cmd+.',
  'cmd+\\',
  'cmd+r',
  'shift+cmd+r',
  'f5',
  'shift+cmd+v',
  'ctrl+shift+v',
  'opt+cmd+[',
  'opt+cmd+]',
  'cmd+s',
  'cmd+f',
  'cmd+g',
  'shift+cmd+g',
  'opt+cmd+g',
  'cmd+d',
  'ctrl+shift+up',
  'ctrl+shift+down',
  'cmd+/',
  'f12',
  'cmd+down',
  'opt+cmd+n',
  'shift+cmd+n',
  'enter',
  'cmd+backspace',
  'cmd+x',
  'cmd+c',
  'cmd+v',
  'opt+cmd+c',
  'opt+shift+cmd+c',
  'opt+cmd+r',
  'opt+cmd+w',
  'opt+cmd+z',
])

export function parseKeyEquivalent(s: string): string | null {
  const raw = s.trim()
  if (!raw) return null
  const parts = raw.split('+')
  const mods = new Set<string>()
  let key = ''
  for (let i = 0; i < parts.length; i++) {
    let tok = parts[i]!.trim().toLowerCase()
    if (!tok) return null
    const mod = KNOWN_MODS[tok]
    if (mod) {
      mods.add(mod)
      continue
    }
    if (i !== parts.length - 1) return null
    tok = KEY_ALIASES[tok] ?? tok
    if (!KNOWN_KEYS.has(tok)) return null
    key = tok
  }
  if (!key) return null
  const out: string[] = []
  for (const m of MOD_ORDER) {
    if (mods.has(m)) out.push(m)
  }
  out.push(key)
  return out.join('+')
}

export function isReservedAppChord(canonical: string): boolean {
  return RESERVED_APP_CHORDS.has(canonical)
}

/** Normalize a KeyboardEvent to a canonical chord, or null if not representable. */
export function chordFromKeyboardEvent(e: KeyboardEvent): string | null {
  const mods = new Set<string>()
  if (e.ctrlKey) mods.add('ctrl')
  if (e.altKey) mods.add('opt')
  if (e.shiftKey) mods.add('shift')
  if (e.metaKey) mods.add('cmd')

  let key = keyFromEvent(e)
  if (!key) return null

  // Shift is how < and > are typed; drop it so Control-< matches `ctrl+<`.
  if ((key === '<' || key === '>') && mods.has('shift')) {
    mods.delete('shift')
    if (mods.size === 0) return null
  }

  // Drop shift from letter/digit chords when it only affects case/symbol — keep
  // shift when it's a meaningful modifier with letters (cmd+shift+u).
  // Browser already gives e.key as shifted symbol for pure shift+digit; we use code for letters.
  if (mods.has('shift') && mods.size === 1 && key.length === 1 && /[a-z0-9]/.test(key)) {
    // shift+letter alone is typing — not a chord we dispatch for bundles
    return null
  }

  const out: string[] = []
  for (const m of MOD_ORDER) {
    if (mods.has(m)) out.push(m)
  }
  out.push(key)
  return out.join('+')
}

function keyFromEvent(e: KeyboardEvent): string | null {
  const code = e.code
  const withMod = e.ctrlKey || e.metaKey || e.altKey
  // Control-< is Comma+Shift on US keyboards; some browsers still report key=','.
  if (e.key === '<' || (code === 'Comma' && e.shiftKey && withMod)) return '<'
  if (e.key === '>' || (code === 'Period' && e.shiftKey && withMod)) return '>'
  if (code.startsWith('Key') && code.length === 4) {
    return code.slice(3).toLowerCase()
  }
  if (code.startsWith('Digit') && code.length === 6) {
    return code.slice(5)
  }
  if (code.startsWith('Numpad') && code.length === 7 && /\d/.test(code.slice(6))) {
    return code.slice(6)
  }
  const fromCode: Record<string, string> = {
    Enter: 'enter',
    Escape: 'escape',
    Tab: 'tab',
    Space: 'space',
    ArrowUp: 'up',
    ArrowDown: 'down',
    ArrowLeft: 'left',
    ArrowRight: 'right',
    Home: 'home',
    End: 'end',
    PageUp: 'pageup',
    PageDown: 'pagedown',
    Delete: 'delete',
    Backspace: 'backspace',
    Backslash: '\\',
    Slash: '/',
    Backquote: '`',
    BracketLeft: '[',
    BracketRight: ']',
    Semicolon: ';',
    Quote: "'",
    Comma: ',',
    Period: '.',
    Minus: '-',
    Equal: '=',
  }
  if (fromCode[code]) return fromCode[code]
  if (/^F\d{1,2}$/.test(code)) return code.toLowerCase()

  const k = e.key
  if (k === ' ') return 'space'
  if (k.length === 1) {
    const lower = k.toLowerCase()
    if (KNOWN_KEYS.has(lower)) return lower
  }
  const fromKey: Record<string, string> = {
    Enter: 'enter',
    Escape: 'escape',
    Tab: 'tab',
    ArrowUp: 'up',
    ArrowDown: 'down',
    ArrowLeft: 'left',
    ArrowRight: 'right',
    Backspace: 'backspace',
    Delete: 'delete',
  }
  return fromKey[k] ?? null
}

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

/** Effective binding for a command: parsed + non-reserved, else empty. */
export function effectiveKeyEquivalent(raw: string | undefined): string {
  if (!raw?.trim()) return ''
  const can = parseKeyEquivalent(raw)
  if (!can || isReservedAppChord(can)) return ''
  return can
}
