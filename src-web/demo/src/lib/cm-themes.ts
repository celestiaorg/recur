// Editor colour themes from the bundle TextMate themes (Recur + Themes packs).
// Only the chrome colours (background, gutter, caret, selection) apply: the
// demo does not tokenize code.
import type { Extension } from '@codemirror/state'
import { EditorView } from '@codemirror/view'
import { parseThemeJson, tmChromeExtension, type ParsedTmTheme } from './tm-theme'
import { service } from './api'
import type { ThemeInfo } from './types'

const themes = new Map<string, ParsedTmTheme>()

export async function loadEditorThemes() {
  const list: ThemeInfo[] = await service.ListThemes()
  for (const t of list) {
    const parsed = parseThemeJson(await service.ReadTheme(t.id), t.name)
    if (t.uiTheme === 'light' || t.uiTheme === 'dark') parsed.uiTheme = t.uiTheme
    themes.set(t.id, parsed)
  }
}

export function resolveEditorThemeKey(key: string): string {
  return themes.has(key) ? key : 'recur'
}

/** dark | light for the editor theme (independent of app chrome). */
export function editorThemeKind(key: string): 'dark' | 'light' {
  return themes.get(key)?.uiTheme === 'light' ? 'light' : 'dark'
}

/** Color theme only — pair with editorChromeExtension for layout prefs. */
export function editorThemeExtension(key: string): Extension {
  const parsed = themes.get(key)
  return parsed ? tmChromeExtension(parsed) : EditorView.darkTheme.of(true)
}

/** Layout chrome shared by every color theme (size/font/height — not colors). */
export function editorChromeExtension(
  fontSize = 12,
  fontFamily = "'JetBrains Mono', ui-monospace, 'SF Mono', Menlo, monospace",
  lineHeight = 1.55,
  thickCaret = false,
): Extension {
  // 2px matches the native caret (diff view); 3px is Settings → Thick caret.
  const caretW = thickCaret ? '3px' : '2px'
  const caretNudge = thickCaret ? '-1.5px' : '-1px'
  // Note: &dark/&light only work in EditorView.baseTheme(); using them in
  // EditorView.theme() throws and blanks the editor.
  return EditorView.theme({
    '&': { height: '100%', maxHeight: '100%', minHeight: '0', fontSize: `${fontSize}px` },
    '.cm-scroller': {
      fontFamily,
      lineHeight: String(lineHeight),
      overflow: 'auto',
      minHeight: '0',
      overscrollBehavior: 'contain',
    },
    '.cm-content': { padding: '8px 0' },
    '.cm-cursor, .cm-dropCursor': { borderLeftWidth: caretW, marginLeft: caretNudge },
    '&.cm-focused': { outline: 'none' },
  })
}
