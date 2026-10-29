import { System } from '@wailsio/runtime'

/** Modifier / special keys that need optical sizing vs Latin letters. */
const MOD = /[⌘⌥⇧⌃↵⏎↩←→↑↓⎋⌫]/u

/**
 * Wails `CmdOrCtrlKey`: Command on macOS, Control elsewhere.
 * `System.IsMac()` reads the injected `app.Env.Info()` snapshot (no IPC).
 */
export function isCmdOrCtrl(e: { metaKey: boolean; ctrlKey: boolean }): boolean {
  return System.IsMac() ? e.metaKey : e.ctrlKey
}

/**
 * Whether an event is the backquote key (the terminal-dock bindings, ⌃` / ⌃⇧`).
 *
 * `e.code` is the reliable signal: Shift turns ` into ~, so matching e.key alone
 * silently drops the shifted binding. e.key is still checked as a fallback for
 * layouts where backquote isn't on the physical Backquote position.
 */
export function isBackquote(e: KeyboardEvent): boolean {
  return e.code === 'Backquote' || e.key === '`' || e.key === '~'
}

/**
 * Physical A–Z key. Option+letter produces ®/å/etc in WKWebView, so `e.key` is
 * not the letter; `e.code` (`KeyR`) still is. Prefer `e.code` whenever it is
 * `KeyA`–`KeyZ` so a Dvorak `KeyR` (`e.key === 'p'`) cannot match both letters.
 * `e.key` is only the fallback when `code` is not a letter key.
 *
 * AltGraph (Windows/Linux AltGr) reports as Ctrl+Alt, so `isCmdOrCtrl` is true
 * there — refuse composed characters or AltGr+E/`€` would fire the ⌥⌘E chord.
 * macOS has no AltGr; Option *is* the composing modifier and WKWebView may
 * report AltGraph for it, so the veto is off on Mac.
 */
export function isLetterKey(
  e: Pick<KeyboardEvent, 'code' | 'key'> & { getModifierState?: (k: string) => boolean },
  letter: string,
): boolean {
  const L = letter.toLowerCase()
  if (L.length !== 1 || L < 'a' || L > 'z') return false
  if (!System.IsMac() && e.getModifierState?.('AltGraph') && e.key.toLowerCase() !== L) return false
  if (/^Key[A-Z]$/.test(e.code)) return e.code.toLowerCase() === `key${L}`
  return e.key.toLowerCase() === L
}

/** 1–9 for palette rows. `shiftDigits` maps DigitN while Shift is down (⌘⇧R). */
export function paletteRowDigit(e: KeyboardEvent, opts?: { shiftDigits?: boolean }): number | null {
  if (e.metaKey || e.ctrlKey || e.altKey) return null
  if (/^[1-9]$/.test(e.key)) return Number(e.key)
  if (opts?.shiftDigits && /^Digit[1-9]$/.test(e.code)) return Number(e.code.slice(5))
  return null
}

export type ShortcutPart = { text: string; mod: boolean }

/** Split a shortcut string so modifier glyphs can be styled separately. */
export function shortcutParts(s: string): ShortcutPart[] {
  const out: ShortcutPart[] = []
  for (const ch of s) {
    const mod = MOD.test(ch)
    const last = out[out.length - 1]
    if (last && !last.mod && !mod) last.text += ch
    else out.push({ text: ch, mod })
  }
  return out
}
