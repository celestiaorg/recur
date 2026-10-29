/**
 * TextMate highlighting for settings script editors, tokenized against a
 * chrome-paired engine so colours match that chrome — not the global editor
 * theme's colour map.
 */
import { RangeSetBuilder, type Extension } from '@codemirror/state'
import { Decoration, EditorView, ViewPlugin, type DecorationSet, type ViewUpdate } from '@codemirror/view'
import { lineReplaceFromChanges } from './cm-tm-delta'
import { selectionRanges } from './cm-tm-deco'
import { getChromePairedThemeId } from './tm-registry'
import type { TmEngine } from './tm-engine'
import {
  TM_FONT,
  compositeCssColor,
  decodeBackground,
  decodeFontStyle,
  decodeForeground,
  shouldPaintTokenBackground,
  tokenForegroundReadable,
  type ParsedTmTheme,
} from './tm-theme'

let seq = 0

function documentBgs(parsed: ParsedTmTheme): string[] {
  const bg = parsed.colors['editor.background'] ?? '#1e1e1e'
  const out = [bg]
  if (parsed.rootBackgrounds.source) out.push(parsed.rootBackgrounds.source)
  if (parsed.rootBackgrounds.text) out.push(parsed.rootBackgrounds.text)
  return out
}

function paneBackground(parsed: ParsedTmTheme, scopeName: string): string {
  const editorBg = parsed.colors['editor.background'] ?? '#1e1e1e'
  if (scopeName === 'text' || scopeName.startsWith('text.')) {
    return parsed.rootBackgrounds.text || editorBg
  }
  return parsed.rootBackgrounds.source || editorBg
}

function viewportLines(view: EditorView): { from: number; to: number } {
  const doc = view.state.doc
  const from = doc.lineAt(view.viewport.from).number
  const to = doc.lineAt(view.viewport.to).number
  return { from, to }
}

function overlapsSelection(
  from: number,
  to: number,
  ranges: ReadonlyArray<{ from: number; to: number }>,
): boolean {
  for (const r of ranges) {
    if (from < r.to && to > r.from) return true
  }
  return false
}

function styleForMeta(
  meta: number,
  paintBackground: boolean,
  colorMap: string[],
  bgs: string[],
  paneBg: string,
): { style: string } | null {
  const fg = decodeForeground(meta)
  const bg = decodeBackground(meta)
  const fs = decodeFontStyle(meta)
  const fgColor = fg > 0 ? colorMap[fg] : undefined
  const bgColor = bg > 0 ? colorMap[bg] : undefined
  const paintTokenBg = paintBackground && bg > 0 && shouldPaintTokenBackground(bgColor, bgs)
  const surface = paintTokenBg && bgColor ? compositeCssColor(bgColor, paneBg) : paneBg
  const styles: string[] = []
  if (fg > 0 && fgColor && tokenForegroundReadable(fgColor, surface)) {
    styles.push(`color: ${fgColor}`)
  }
  if (paintTokenBg && bgColor) styles.push(`background-color: ${bgColor}`)
  if (fs & TM_FONT.Italic) styles.push('font-style: italic')
  if (fs & TM_FONT.Bold) styles.push('font-weight: bold')
  if (fs & TM_FONT.Underline) styles.push('text-decoration: underline')
  if (fs & TM_FONT.Strikethrough) styles.push('text-decoration: line-through')
  if (styles.length === 0) return null
  return { style: styles.join(';') }
}

/** Viewport highlighter bound to a chrome-paired TmEngine. */
export function miniScriptHighlight(
  scopeName: string,
  engine: TmEngine,
  parsed: ParsedTmTheme,
  themeId: string,
): Extension {
  const bgs = documentBgs(parsed)
  const paneBg = paneBackground(parsed, scopeName)
  const docId = `mini${++seq}`

  return ViewPlugin.fromClass(
    class {
      decorations: DecorationSet = Decoration.none
      private opened = false
      private dead = false

      constructor(readonly view: EditorView) {
        engine.openDoc(docId, scopeName, view.state.doc.toString())
        this.opened = true
        this.paint(view)
      }

      update(update: ViewUpdate) {
        if (!this.opened || this.dead) return
        if (update.docChanged) {
          this.decorations = this.decorations.map(update.changes)
          const replace = lineReplaceFromChanges(
            update.startState.doc,
            update.state.doc,
            update.changes,
          )
          if (replace) engine.editDoc(docId, replace)
        }
        // Skip paint while a sibling has re-themed the shared engine; still
        // map marks / editDoc so a doc change in that window stays aligned.
        if (getChromePairedThemeId() !== themeId) return
        if (update.docChanged || update.viewportChanged || update.selectionSet || !engine.hasDoc(docId)) {
          this.paint(update.view)
        }
      }

      destroy() {
        this.dead = true
        if (this.opened) engine.releaseDoc(docId)
      }

      private paint(view: EditorView) {
        if (getChromePairedThemeId() !== themeId) return
        if (!engine.hasDoc(docId)) {
          engine.openDoc(docId, scopeName, view.state.doc.toString())
        }
        const { from, to } = viewportLines(view)
        engine.tokenizeViewport(docId, from, to, 50)
        const colorMap = engine.getColorMap()
        const sel = selectionRanges(view)
        const memo = new Map<number, { style: string } | null>()
        const specFor = (meta: number, paintBg: boolean) => {
          const key = meta * 2 + (paintBg ? 1 : 0)
          let spec = memo.get(key)
          if (spec === undefined) {
            spec = styleForMeta(meta, paintBg, colorMap, bgs, paneBg)
            memo.set(key, spec)
          }
          return spec
        }
        const builder = new RangeSetBuilder<Decoration>()
        const last = Math.min(to, view.state.doc.lines)
        for (let n = Math.max(1, from); n <= last; n++) {
          const tokens = engine.tokensForLine(docId, n)
          if (!tokens) continue
          const line = view.state.doc.line(n)
          const textLen = line.text.length
          for (let i = 0; i < tokens.length; i += 2) {
            const start = Math.min(tokens[i], textLen)
            const meta = tokens[i + 1]
            const end = Math.min(i + 2 < tokens.length ? tokens[i + 2] : textLen, textLen)
            if (end <= start) continue
            const fromPos = line.from + start
            const toPos = line.from + end
            const paintBg = !(sel.length > 0 && overlapsSelection(fromPos, toPos, sel))
            const spec = specFor(meta, paintBg)
            if (!spec) continue
            builder.add(fromPos, toPos, Decoration.mark({ attributes: spec }))
          }
        }
        this.decorations = builder.finish()
      }
    },
    { decorations: (v) => v.decorations },
  )
}
