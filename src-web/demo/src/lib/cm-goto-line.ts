/**
 * Recur-styled go-to-line dialog — replaces stock CM showDialog chrome.
 * Same parse rules as @codemirror/search's gotoLine (+N / -N / N% / N:col).
 */
import { EditorSelection, type SelectionRange } from '@codemirror/state'
import { EditorView, showDialog, type Command } from '@codemirror/view'
import { icons, type IconDef } from './icons'

function el<K extends keyof HTMLElementTagNameMap>(
  tag: K,
  attrs: Record<string, string | boolean | undefined> = {},
  children: (Node | string)[] = [],
): HTMLElementTagNameMap[K] {
  const node = document.createElement(tag)
  for (const [k, v] of Object.entries(attrs)) {
    if (v == null || v === false) continue
    if (v === true) node.setAttribute(k, '')
    else node.setAttribute(k, String(v))
  }
  for (const child of children) {
    node.append(typeof child === 'string' ? document.createTextNode(child) : child)
  }
  return node
}

function svgIcon(name: keyof typeof icons): SVGSVGElement {
  const def: IconDef = icons[name]
  const svg = document.createElementNS('http://www.w3.org/2000/svg', 'svg')
  svg.setAttribute('viewBox', def.viewBox ?? '0 0 24 24')
  svg.setAttribute('fill', 'none')
  svg.setAttribute('stroke', 'currentColor')
  svg.setAttribute('stroke-width', String(def.strokeWidth ?? 2))
  svg.setAttribute('stroke-linecap', 'round')
  svg.setAttribute('stroke-linejoin', 'round')
  svg.setAttribute('aria-hidden', 'true')
  svg.innerHTML = def.body
  return svg
}

function parseGotoTarget(
  raw: string,
  state: EditorView['state'],
): SelectionRange | null {
  const match = /^([+-])?(\d+)?(:\d+)?(%)?$/.exec(raw.trim())
  if (!match) return null
  const startLine = state.doc.lineAt(state.selection.main.head)
  const [, sign, ln, cl, percent] = match
  const col = cl ? +cl.slice(1) : 0
  let line = ln ? +ln : startLine.number
  if (ln && percent) {
    let pc = line / 100
    if (sign) pc = pc * (sign === '-' ? -1 : 1) + startLine.number / state.doc.lines
    line = Math.round(state.doc.lines * pc)
  } else if (ln && sign) {
    line = line * (sign === '-' ? -1 : 1) + startLine.number
  }
  const docLine = state.doc.line(Math.max(1, Math.min(state.doc.lines, line)))
  return EditorSelection.cursor(docLine.from + Math.max(0, Math.min(col, docLine.length)))
}

/** Open the Recur go-to-line dialog (⌥⌘G). */
export const recurGotoLine: Command = (view) => {
  const { state } = view
  const cur = String(state.doc.lineAt(state.selection.main.head).number)
  const { close, result } = showDialog(view, {
    class: 'cm-recur-goto',
    top: true,
    focus: 'input[name=line]',
    content: (_view, done) => {
      const form = el('form', { class: 'cm-recur-goto-form' })
      const input = el('input', {
        type: 'text',
        class: 'cm-recur-search-input cm-recur-goto-input',
        name: 'line',
        value: cur,
        placeholder: 'Line',
        'aria-label': 'Go to line',
        spellcheck: 'false',
        autocomplete: 'off',
      }) as HTMLInputElement
      input.value = cur

      const goBtn = el('button', {
        type: 'submit',
        class: 'cm-recur-search-textbtn cm-recur-goto-go',
        title: 'Go',
      }, ['Go'])

      const closeBtn = el('button', {
        type: 'button',
        class: 'obs-icon-btn cm-recur-search-close',
        title: 'Close (Escape)',
        'aria-label': 'Close',
      }, [svgIcon('x')])
      closeBtn.addEventListener('click', () => done())

      form.append(
        el('span', { class: 'cm-recur-search-glyph', 'aria-hidden': 'true' }, [svgIcon('hash')]),
        el('span', { class: 'cm-recur-goto-label' }, ['Go to line']),
        input,
        goBtn,
        closeBtn,
      )
      return form
    },
  })

  void result.then((form) => {
    if (!form) {
      view.dispatch({ effects: close })
      return
    }
    const raw = (form.elements.namedItem('line') as HTMLInputElement | null)?.value ?? ''
    const selection = parseGotoTarget(raw, view.state)
    if (!selection) {
      view.dispatch({ effects: close })
      return
    }
    view.dispatch({
      effects: [close, EditorView.scrollIntoView(selection.from, { y: 'center' })],
      selection,
    })
  })
  return true
}
