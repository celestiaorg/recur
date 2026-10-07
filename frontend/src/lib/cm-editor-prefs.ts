// CodeMirror extensions driven by editorPrefs (beyond theme/font/indent).
import {
  EditorView, ViewPlugin, lineNumbers, highlightActiveLine, highlightActiveLineGutter,
  highlightWhitespace, highlightTrailingWhitespace, highlightSpecialChars,
  scrollPastEnd, rectangularSelection, crosshairCursor, dropCursor, drawSelection,
  type ViewUpdate,
} from '@codemirror/view'
import { EditorState, type Extension } from '@codemirror/state'
import { foldGutter, bracketMatching } from '@codemirror/language'
import type { EditorPrefs, EditorWhitespace, EditorKeymap } from './stores'

/** Quiet marks — CM defaults (#aaa / #888) read as loud as code on dark themes. */
const subtleWhitespaceTheme = EditorView.theme({
  '.cm-highlightSpace': {
    backgroundImage:
      'radial-gradient(circle at 50% 55%, color-mix(in srgb, currentColor 16%, transparent) 20%, transparent 5%)',
  },
  '.cm-highlightTab': {
    backgroundImage: `url('data:image/svg+xml,<svg xmlns="http://www.w3.org/2000/svg" width="200" height="20"><path stroke="%2388888838" stroke-width="1" fill="none" d="M1 10H196L190 5M190 15L196 10M197 4L197 16"/></svg>')`,
  },
  '.cm-trailingSpace': {
    backgroundColor: 'color-mix(in srgb, #e07a5f 16%, transparent)',
  },
})

export function whitespaceExtensions(mode: EditorWhitespace): Extension {
  if (mode === 'all') return [highlightWhitespace(), subtleWhitespaceTheme]
  if (mode === 'trailing') return [highlightTrailingWhitespace(), subtleWhitespaceTheme]
  return []
}

export function specialCharsExtension(on: boolean): Extension {
  return on ? highlightSpecialChars() : []
}

export function lineNumbersExtension(
  on: boolean,
  onCopyLine?: (line: number) => void,
): Extension {
  if (!on) return []
  return lineNumbers({
    domEventHandlers: {
      click(_view, line, event) {
        event.preventDefault()
        const ln = _view.state.doc.lineAt(line.from).number
        onCopyLine?.(ln)
        return true
      },
    },
  })
}

export function activeLineExtension(on: boolean): Extension {
  if (!on) return []
  return [
    highlightActiveLine(),
    highlightActiveLineGutter(),
    // One inset on the leftmost open gutter. Findings/lint sit left of Ln,
    // so a generic `.cm-activeLineGutter` inset would double on fold too.
    EditorView.theme({
      '.cm-gutters-before': {
        '--cm-active-gutter-bar': 'var(--amber)',
      },
      '.cm-lineNumbers .cm-gutterElement.cm-activeLineGutter': {
        boxShadow: 'inset 2px 0 0 var(--cm-active-gutter-bar)',
      },
      '.cm-lint-sev-gutter:has(.cm-lint-sev) .cm-gutterElement.cm-activeLineGutter': {
        boxShadow: 'inset 2px 0 0 var(--cm-active-gutter-bar)',
      },
      '.cm-lint-sev-gutter:has(.cm-lint-sev) ~ .cm-lineNumbers .cm-gutterElement.cm-activeLineGutter': {
        boxShadow: 'none',
      },
      '.cm-findings-gutter:has(.cm-finding-mark) .cm-gutterElement.cm-activeLineGutter': {
        boxShadow: 'inset 2px 0 0 var(--cm-active-gutter-bar)',
      },
      '.cm-findings-gutter:has(.cm-finding-mark) ~ .cm-lint-sev-gutter .cm-gutterElement.cm-activeLineGutter, .cm-findings-gutter:has(.cm-finding-mark) ~ .cm-lineNumbers .cm-gutterElement.cm-activeLineGutter': {
        boxShadow: 'none',
      },
    }),
  ]
}

export function foldGutterExtension(on: boolean): Extension {
  return on ? foldGutter() : []
}

export function bracketMatchingExtension(on: boolean): Extension {
  return on ? bracketMatching() : []
}

export function scrollPastEndExtension(on: boolean): Extension {
  return on ? scrollPastEnd() : []
}

export function rectangularSelectExtension(on: boolean): Extension {
  return on ? [rectangularSelection(), crosshairCursor()] : []
}

export function dropCursorExtension(on: boolean): Extension {
  return on ? dropCursor() : []
}

export function drawSelectionExtension(blink: boolean): Extension {
  return drawSelection({ cursorBlinkRate: blink ? 1200 : 0 })
}

/**
 * Vim and Emacs modes are loaded on demand — together they are ~126 kB that the
 * default keymap never touches. Both wrap their bindings in a high precedence
 * internally, so living in their own compartment rather than inline with the
 * other feature extensions does not change which keys win.
 */
const keymapCache = new Map<EditorKeymap, Extension>()

/** The modal keymap if it is already resident (otherwise nothing, for now). */
export function keymapExtensionSync(mode: EditorKeymap): Extension {
  if (mode === 'default') return []
  return keymapCache.get(mode) ?? []
}

/** Resolves to null when there is nothing new to apply. */
export async function loadKeymapExtension(mode: EditorKeymap): Promise<Extension | null> {
  if (mode === 'default') return null
  const cached = keymapCache.get(mode)
  if (cached) return null
  try {
    const ext = mode === 'vim'
      ? (await import('@replit/codemirror-vim')).vim()
      : (await import('@replit/codemirror-emacs')).emacs()
    keymapCache.set(mode, ext)
    return ext
  } catch (err) {
    console.error(`failed to load ${mode} keymap`, err)
    return null
  }
}

/** Vertical print-margin rulers at the given 1-based columns (empty = off). */
export function rulersExtension(columns: number[]): Extension {
  const cols = [...new Set(columns.filter((c) => c > 0))].sort((a, b) => a - b)
  if (!cols.length) return []

  return [
    EditorView.theme({
      '.cm-rulerLayer': {
        position: 'absolute',
        top: '0',
        left: '0',
        right: '0',
        pointerEvents: 'none',
        zIndex: '1',
      },
      '.cm-ruler': {
        position: 'absolute',
        top: '0',
        bottom: '0',
        width: '1px',
        backgroundColor: 'rgba(150, 148, 144, 0.35)',
      },
    }),
    ViewPlugin.fromClass(class {
      layer: HTMLDivElement
      marks: HTMLDivElement[] = []
      onScroll: () => void

      constructor(readonly view: EditorView) {
        this.layer = view.scrollDOM.appendChild(document.createElement('div'))
        this.layer.className = 'cm-rulerLayer'
        this.layer.setAttribute('aria-hidden', 'true')
        for (const col of cols) {
          const el = this.layer.appendChild(document.createElement('div'))
          el.className = 'cm-ruler'
          el.dataset.col = String(col)
          this.marks.push(el)
        }
        this.onScroll = () => this.place()
        view.scrollDOM.addEventListener('scroll', this.onScroll)
        this.place()
      }

      update(u: ViewUpdate) {
        if (u.geometryChanged || u.viewportChanged || u.docChanged) this.place()
      }

      place() {
        const view = this.view
        const scroll = view.scrollDOM
        // Pin to the visible viewport (not inset:0 at doc top). Abspos children of
        // scrollDOM scroll with content, so a viewport-tall layer at y=0 only covered
        // the first screenful. Avoid height=scrollHeight — that can grow overflow.
        this.layer.style.top = `${scroll.scrollTop}px`
        this.layer.style.height = `${scroll.clientHeight}px`
        const defaultCh = view.defaultCharacterWidth
        const baseLeft = view.contentDOM.getBoundingClientRect().left
          - scroll.getBoundingClientRect().left
          + scroll.scrollLeft
        for (let i = 0; i < cols.length; i++) {
          const x = baseLeft + cols[i] * defaultCh
          this.marks[i].style.left = `${Math.round(x)}px`
        }
      }

      destroy() {
        this.view.scrollDOM.removeEventListener('scroll', this.onScroll)
        this.layer.remove()
      }
    }),
  ]
}

/** Bundle of preference-driven chrome extras (whitespace → keymap). */
export function editorFeatureExtensions(
  prefs: EditorPrefs,
  onCopyLine?: (line: number) => void,
): Extension {
  const rulers = prefs.rulerColumn > 0 ? [prefs.rulerColumn] : []
  return [
    // CM drops extra ranges unless this is on — Option/Alt-drag columns and Mod-d.
    EditorState.allowMultipleSelections.of(true),
    whitespaceExtensions(prefs.whitespace),
    specialCharsExtension(prefs.specialChars),
    lineNumbersExtension(prefs.lineNumbers, onCopyLine),
    activeLineExtension(prefs.activeLine),
    foldGutterExtension(prefs.foldGutter),
    bracketMatchingExtension(prefs.bracketMatching),
    scrollPastEndExtension(prefs.scrollPastEnd),
    rectangularSelectExtension(prefs.rectangularSelect),
    dropCursorExtension(prefs.dropCursor),
    drawSelectionExtension(prefs.cursorBlink),
    rulersExtension(rulers),
  ]
}
