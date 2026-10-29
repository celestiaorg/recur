/**
 * CodeMirror folding for TextMate languages (no Lezer tree).
 *
 * Prefer grammar `foldingStartMarker` / `foldingStopMarker` when present; otherwise
 * fall back to indentation folding (same default VS Code uses for most languages).
 */
import { foldService } from '@codemirror/language'
import type { EditorState, Extension } from '@codemirror/state'
import { getFoldMarkerSources, type TmFoldMarkers } from './tm-registry'

export type CompiledFoldMarkers = { start: RegExp; stop: RegExp }

export function compileFoldMarkers(src: TmFoldMarkers | null): CompiledFoldMarkers | null {
  if (!src) return null
  try {
    return { start: new RegExp(src.startSource), stop: new RegExp(src.stopSource) }
  } catch {
    return null
  }
}

function lineIndent(text: string, tabSize: number): number {
  let n = 0
  for (let i = 0; i < text.length; i++) {
    const ch = text[i]
    if (ch === ' ') n++
    else if (ch === '\t') n += tabSize
    else break
  }
  return n
}

function matches(re: RegExp, text: string): boolean {
  re.lastIndex = 0
  return re.test(text)
}

/** Indent-based fold: deeper-indented block after `lineStart`. */
export function foldByIndent(
  state: EditorState,
  lineStart: number,
): { from: number; to: number } | null {
  const line = state.doc.lineAt(lineStart)
  if (!/\S/.test(line.text)) return null
  const startIndent = lineIndent(line.text, state.tabSize)
  let foldTo: number | null = null
  for (let n = line.number + 1; n <= state.doc.lines; n++) {
    const next = state.doc.line(n)
    if (!/\S/.test(next.text)) continue
    if (lineIndent(next.text, state.tabSize) > startIndent) {
      foldTo = next.to
    } else {
      break
    }
  }
  if (foldTo == null) return null
  return { from: line.to, to: foldTo }
}

/**
 * TM marker fold: start line matches `start`, scan with a depth stack until the
 * matching `stop`. Collapses inner lines (keeps start + stop lines visible).
 */
export function foldByMarkers(
  state: EditorState,
  lineStart: number,
  start: RegExp,
  stop: RegExp,
): { from: number; to: number } | null {
  const line = state.doc.lineAt(lineStart)
  if (!matches(start, line.text)) return null
  let depth = 1
  for (let n = line.number + 1; n <= state.doc.lines; n++) {
    const text = state.doc.line(n).text
    if (matches(start, text)) depth++
    if (matches(stop, text)) {
      depth--
      if (depth === 0) {
        if (n <= line.number + 1) return null
        return { from: line.to, to: state.doc.line(n - 1).to }
      }
    }
  }
  return null
}

/** `foldService` for a TextMate scope (markers captured at language load). */
export function textmateFolding(scopeName: string): Extension {
  const markers = compileFoldMarkers(getFoldMarkerSources(scopeName))
  return foldService.of((state, lineStart, _lineEnd) => {
    if (markers) {
      const byMark = foldByMarkers(state, lineStart, markers.start, markers.stop)
      if (byMark) return byMark
    }
    return foldByIndent(state, lineStart)
  })
}
