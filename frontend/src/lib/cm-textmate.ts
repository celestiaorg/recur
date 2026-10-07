/**
 * CodeMirror 6 TextMate highlighter (v2 — see TM-REDESIGN.md).
 *
 * VS Code's shape: the first frame is covered by SYNCHRONOUS viewport
 * tokenization on the main thread (accurate when the state chain reaches the
 * viewport, heuristic guessed-start otherwise), the rest of the file fills in
 * during idle time, and decorations are a viewport-scoped pure function of
 * the token cache — rebuilt in ~0.3ms, never parked, never patched.
 *
 * Scroll policy (VS Code's AttachedViewHandler): warm regions rebuild inline;
 * cold regions render plain and get one debounced tokenize pass when the
 * scroll settles, so fast flings never compete with the scroller for frames.
 */
import { Facet, type Extension, type Text } from '@codemirror/state'
import {
  Decoration,
  EditorView,
  ViewPlugin,
  type DecorationSet,
  type ViewUpdate,
} from '@codemirror/view'
import { textmateLanguageData } from './cm-lang-data'
import { buildDecorationsFromTokens } from './cm-tm-deco'
import { lineReplaceFromChanges } from './cm-tm-delta'
import { tmDocFingerprint, tmDocKey, type TmDocFingerprint } from './cm-tm-doc'
import { textmateFolding } from './cm-tm-fold'
import { tmPaintTokenBackgrounds } from './cm-tm-paint'
import { tmEnsureFill } from './tm-fill'
import {
  getColorMapEpoch,
  hasGrammar,
  loadGrammar,
  onColorMapChange,
  peekTmEngine,
} from './tm-registry'
import type { TmEngine } from './tm-engine'
import { tmRootContentExtension } from './tm-theme'

const tmScopeFacet = Facet.define<string, string>({
  combine: (v) => v[v.length - 1] ?? '',
})

/** Debounced cold-scroll tokenize pass (VS Code: 50ms RunOnceScheduler). */
const SCROLL_PASS_MS = 50
/** Small docs always tokenize inline — even a guessed start is ~1ms. */
const CHEAP_DOC_LINES = 1500
/** Accurate resume within this many extra lines counts as cheap. */
const INLINE_RESUME_SLACK = 120
/** No TM at all above this (VS Code disables at 300k lines). */
const MAX_TM_LINES = 200_000
/**
 * Safety cap for a degenerate first-layout viewport (whole document). Bounds
 * the work of one turn; the window is anchored on the visible lines so the
 * truncated tail is always off-screen.
 */
const MAX_BUILD_LINES = 1500

let docSeq = 0
function nextDocId(): string {
  return `tm${++docSeq}`
}

function fingerprintOf(doc: Text): TmDocFingerprint {
  return tmDocFingerprint((n) => doc.line(n).text, doc.lines, doc.length)
}

type Win = { from: number; to: number }

/**
 * The lines CodeMirror has rendered. Decorations MUST cover exactly this —
 * `visibleRanges` is only the on-screen subset of the rendered viewport, and
 * decorating just that leaves rendered lines above/below the fold unstyled
 * (blank bands while scrolling, and a theme change repainting only part of
 * the screen). Main's highlighter used `view.viewport` for the same reason.
 */
function viewportWindow(view: EditorView): Win {
  const doc = view.state.doc
  const lineAt = (pos: number) => doc.lineAt(Math.max(0, Math.min(pos, doc.length))).number
  const from = lineAt(view.viewport.from)
  const to = lineAt(view.viewport.to)
  if (to - from < MAX_BUILD_LINES) return { from, to }
  // A first layout (no measured heights yet) can report the whole document as
  // the viewport. Cap the span so one turn never builds marks for a 100k-line
  // file — but anchor the cap on the lines actually on screen, because
  // everything outside the window stays unstyled until the window moves.
  let anchor = from
  if (view.visibleRanges.length > 0) anchor = lineAt(view.visibleRanges[0].from)
  const start = Math.max(from, Math.min(anchor - (MAX_BUILD_LINES >> 2), to - MAX_BUILD_LINES + 1))
  return { from: start, to: Math.min(to, start + MAX_BUILD_LINES - 1) }
}

function hasNonEmptySelection(view: EditorView): boolean {
  for (const r of view.state.selection.ranges) {
    if (!r.empty) return true
  }
  return false
}

function tmPlugin(scopeName: string): Extension {
  return ViewPlugin.fromClass(
    class {
      decorations: DecorationSet = Decoration.none
      private dead = false
      private docId: string
      private opened = false
      private mapEpoch = getColorMapEpoch()
      private stopFill: (() => void) | null = null
      private scrollTimer: ReturnType<typeof setTimeout> | undefined
      private redrawQueued = false
      private prevSelNonEmpty = false
      private unsubMap: (() => void) | null = null
      /** Line span the current decoration set was built for. */
      private builtFrom = 0
      private builtTo = -1

      constructor(readonly view: EditorView) {
        this.docId = view.state.facet(tmDocKey) || nextDocId()
        this.mountOrLoad(view)
        // Safety net for a view that receives no transaction after a theme
        // change (its own reconfigure normally repaints in the same turn).
        // A live view whose own transaction already handled the change will
        // have caught up by the time this runs; this covers views that get no
        // transaction of their own (an inactive split pane).
        this.unsubMap = onColorMapChange(() => {
          if (this.dead) return
          requestAnimationFrame(() => {
            if (this.dead || this.mapEpoch === getColorMapEpoch()) return
            this.mapEpoch = getColorMapEpoch()
            this.remount(this.view)
            this.redrawSoon()
          })
        })
      }

      update(update: ViewUpdate) {
        const key = update.state.facet(tmDocKey)
        if (key && key !== this.docId) {
          this.switchDoc(key, update.view)
          return
        }
        const engine = peekTmEngine()
        if (update.docChanged) {
          // Keep existing marks aligned until the rebuild below replaces them.
          this.decorations = this.decorations.map(update.changes)
          if (engine && this.opened) {
            const replace = lineReplaceFromChanges(
              update.startState.doc,
              update.state.doc,
              update.changes,
            )
            if (replace) engine.editDoc(this.docId, replace)
          }
        }
        if (!engine) return
        // Theme change: the engine dropped every document, so re-open this
        // one and tokenize it fresh — the same reset a close-and-reopen does.
        // Checked before the `opened` guard so a view that never finished
        // mounting still recovers.
        const mapEpoch = getColorMapEpoch()
        if (mapEpoch !== this.mapEpoch) {
          this.mapEpoch = mapEpoch
          this.remount(update.view)
          return
        }
        if (!this.opened) return
        const paintBg = update.state.facet(tmPaintTokenBackgrounds)
        const selNonEmpty = paintBg && hasNonEmptySelection(update.view)
        const selectionChanged = update.selectionSet && (selNonEmpty || this.prevSelNonEmpty)
        if (selectionChanged) this.prevSelNonEmpty = selNonEmpty

        if (update.docChanged || selectionChanged) {
          // "Stabilized" equivalents — tokenize inline in this same turn.
          // Keystrokes are 1–3 lines (early-stop); theme swaps re-tokenize the
          // viewport before the browser paints the new colour-map CSS.
          this.paintViewport(update.view, true)
          return
        }
        // Rebuild whenever the rendered span differs from what the current
        // marks were built for. Watching `viewportChanged` alone is not
        // enough: the constructor paints before the first measure, so the
        // window can widen in an update that reports no viewport change, and
        // every line CM renders past our last build would sit unstyled.
        const win = viewportWindow(update.view)
        if (win.from !== this.builtFrom || win.to !== this.builtTo) {
          this.paintViewport(update.view, false)
        }
      }

      destroy() {
        this.dead = true
        if (this.scrollTimer != null) clearTimeout(this.scrollTimer)
        this.stopFill?.()
        this.stopFill = null
        this.unsubMap?.()
        if (this.opened) peekTmEngine()?.releaseDoc(this.docId)
      }

      private mount(view: EditorView) {
        const engine = peekTmEngine()
        if (!engine || this.opened) return
        const doc = view.state.doc
        if (doc.lines > MAX_TM_LINES) return
        if (!engine.reopenDoc(this.docId, scopeName, fingerprintOf(doc))) {
          engine.openDoc(this.docId, scopeName, doc.toString())
        }
        this.opened = true
        this.paintViewport(view, true)
      }

      private mountOrLoad(view: EditorView) {
        if (peekTmEngine() && hasGrammar(scopeName)) {
          // Grammar resident: tokenize + decorate the viewport synchronously
          // so the very first frame is styled (no flash by construction).
          this.mount(view)
          return
        }
        // Not compiled yet, or dropped by a bundle reload.
        void loadGrammar(scopeName).then((ok) => {
          if (this.dead || !ok) return
          this.mount(this.view)
          this.redrawSoon()
        })
      }

      /**
       * Re-open this document from scratch (theme change). Everything the
       * engine held for it is gone, so drop our own derived state too and
       * paint again as if the file had just been opened.
       */
      private remount(view: EditorView) {
        const engine = peekTmEngine()
        // Only give back a ref the engine still tracks — the document may have
        // been dropped outright (theme change, reload from disk).
        if (this.opened && engine?.hasDoc(this.docId)) engine.releaseDoc(this.docId)
        this.stopFill?.()
        this.stopFill = null
        this.opened = false
        this.decorations = Decoration.none
        this.builtFrom = 0
        this.builtTo = -1
        this.prevSelNonEmpty = false
        this.mountOrLoad(view)
      }

      private switchDoc(key: string, view: EditorView) {
        const engine = peekTmEngine()
        if (this.opened && engine) engine.releaseDoc(this.docId)
        this.stopFill?.()
        this.stopFill = null
        this.docId = key
        this.opened = false
        this.decorations = Decoration.none
        this.builtFrom = 0
        this.builtTo = -1
        this.mountOrLoad(view)
      }

      /**
       * Rebuild viewport decorations from the token cache; tokenize first when
       * allowed (or cheap). Synchronous — runs inside the update turn.
       */
      private paintViewport(view: EditorView, mayTokenize: boolean) {
        const engine = peekTmEngine()
        if (!engine || !this.opened) return
        // The document can be dropped underneath a live view (reload from disk
        // discards the buffer). Re-open from the current text instead of
        // painting an empty set for a document the engine no longer knows.
        if (!engine.hasDoc(this.docId)) {
          this.remount(view)
          return
        }
        const win = viewportWindow(view)
        if (!engine.hasFreshTokens(this.docId, win.from, win.to)) {
          const inline =
            mayTokenize ||
            view.state.doc.lines <= CHEAP_DOC_LINES ||
            engine.canResumeAccurately(this.docId, win.from, INLINE_RESUME_SLACK)
          if (inline) {
            engine.tokenizeViewport(this.docId, win.from, win.to)
          } else {
            this.scheduleScrollPass()
          }
        }
        try {
          this.decorations = buildDecorationsFromTokens(
            view,
            (n) => engine.tokensForLine(this.docId, n),
            win.from,
            win.to,
            scopeName,
          )
          this.builtFrom = win.from
          this.builtTo = win.to
        } catch {
          /* keep previous marks */
        }
        this.ensureFill(engine, view)
      }

      /** One tokenize pass when the scroll settles (never during the fling). */
      private scheduleScrollPass() {
        if (this.scrollTimer != null) clearTimeout(this.scrollTimer)
        this.scrollTimer = setTimeout(() => {
          this.scrollTimer = undefined
          if (this.dead) return
          this.paintViewport(this.view, true)
          this.redrawSoon()
        }, SCROLL_PASS_MS)
      }

      /** Keep the idle sweep alive while the doc has untokenized lines. */
      private ensureFill(engine: TmEngine, view: EditorView) {
        if (this.stopFill) return
        if (engine.filledThrough(this.docId) >= view.state.doc.lines) return
        this.stopFill = tmEnsureFill(engine, this.docId, (changed, done) => {
          if (this.dead) return
          if (done) {
            this.stopFill?.()
            this.stopFill = null
          }
          if (!changed) return
          const win = viewportWindow(this.view)
          if (changed.to < win.from || changed.from > win.to) return
          this.paintViewport(this.view, false)
          this.redrawSoon()
        })
      }

      /** Ask CM to re-read plugin decorations once the current turn is over. */
      private redrawSoon() {
        if (this.redrawQueued) return
        this.redrawQueued = true
        requestAnimationFrame(() => {
          this.redrawQueued = false
          if (this.dead) return
          try {
            this.view.dispatch({})
          } catch {
            try {
              this.view.requestMeasure()
            } catch {
              /* view gone */
            }
          }
        })
      }
    },
    {
      decorations: (v) => v.decorations,
    },
  )
}

/**
 * LanguageSupport-shaped extension: TextMate highlighting for `scopeName`.
 * Does not register Lezer syntaxHighlighting — paint is decoration-only.
 * Folding uses TM start/stop markers when present, else indentation.
 * Comment tokens and indent patterns come from bundle [[languageData]].
 */
export function textmateLanguage(scopeName: string): Extension {
  return [
    tmScopeFacet.of(scopeName),
    tmPlugin(scopeName),
    textmateFolding(scopeName),
    textmateLanguageData(scopeName),
    tmRootContentExtension(scopeName),
  ]
}
