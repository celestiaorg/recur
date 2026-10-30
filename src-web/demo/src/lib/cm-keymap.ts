import { defaultKeymap, addCursorAbove, addCursorBelow } from '@codemirror/commands'
import type { KeyBinding } from '@codemirror/view'

/**
 * Stock CodeMirror add-cursor is Mod-Alt-Arrow: Recur's workspace cycle
 * (Wails CmdOrCtrl+OptionOrAlt+Up/Down: ⌥⌘ on macOS, Ctrl+Alt elsewhere).
 * Drop those two keys and bind ⌃⇧↑ / ⌃⇧↓ on every platform (Sublime select-lines;
 * avoids VoiceOver's Control+Option modifier). These two must sit *before*
 * `defaultKeymap`: on macOS `standardKeymap` registers Ctrl-ArrowUp/Down with a
 * `shift` handler, which canonicalises to the same Shift-Ctrl-Arrow slot and
 * would otherwise win as selectPageUp/Down. Option-drag still places extra
 * carets (rectangular selection).
 */
export const recurDefaultKeymap: readonly KeyBinding[] = [
  { key: 'Ctrl-Shift-ArrowUp', run: addCursorAbove },
  { key: 'Ctrl-Shift-ArrowDown', run: addCursorBelow },
  ...defaultKeymap.filter((b) => b.key !== 'Mod-Alt-ArrowUp' && b.key !== 'Mod-Alt-ArrowDown'),
]
