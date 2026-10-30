/**
 * Recur-styled CodeMirror find/replace panel.
 * Replaces the stock CM form (cm-textfield / cm-button) with chrome that
 * tracks app theme tokens and matches observatory controls.
 */
import {
  EditorView, runScopeHandlers, type Panel, type ViewUpdate, type KeyBinding, type Command,
} from '@codemirror/view'
import { type Extension } from '@codemirror/state'
import {
  SearchQuery,
  setSearchQuery,
  getSearchQuery,
  search,
  searchKeymap,
  findNext,
  findPrevious,
  replaceNext,
  replaceAll,
  selectMatches,
  closeSearchPanel,
} from '@codemirror/search'
import { icons, type IconDef, type IconName } from './icons'
import { recurGotoLine } from './cm-goto-line'

type SearchFlag = 'caseSensitive' | 'regexp' | 'wholeWord'

function toggleSearchFlag(flag: SearchFlag): Command {
  return (view) => {
    const q = getSearchQuery(view.state)
    const next = new SearchQuery({
      search: q.search,
      replace: q.replace,
      literal: q.literal,
      caseSensitive: flag === 'caseSensitive' ? !q.caseSensitive : q.caseSensitive,
      regexp: flag === 'regexp' ? !q.regexp : q.regexp,
      wholeWord: flag === 'wholeWord' ? !q.wholeWord : q.wholeWord,
    })
    if (next.eq(q)) return false
    view.dispatch({ effects: setSearchQuery.of(next) })
    return true
  }
}

/**
 * Stock CM searchKeymap with ⌥⌘G pointed at our Recur go-to-line dialog,
 * plus find-option toggles scoped to the search panel only so they don't
 * collide with explorer ⌥⌘C / ⌥⌘R.
 */
export const recurSearchKeymap: readonly KeyBinding[] = [
  ...searchKeymap.filter((b) => b.key !== 'Mod-Alt-g'),
  { key: 'Mod-Alt-g', run: recurGotoLine },
  { key: 'Mod-Alt-c', run: toggleSearchFlag('caseSensitive'), scope: 'search-panel' },
  { key: 'Mod-Alt-r', run: toggleSearchFlag('regexp'), scope: 'search-panel' },
  { key: 'Mod-Alt-w', run: toggleSearchFlag('wholeWord'), scope: 'search-panel' },
]

const MATCH_CAP = 999

function el<K extends keyof HTMLElementTagNameMap>(
  tag: K,
  attrs: Record<string, string | boolean | ((e: Event) => void) | undefined> = {},
  children: (Node | string)[] = [],
): HTMLElementTagNameMap[K] {
  const node = document.createElement(tag)
  for (const [k, v] of Object.entries(attrs)) {
    if (v == null || v === false) continue
    if (k.startsWith('on') && typeof v === 'function') {
      node.addEventListener(k.slice(2).toLowerCase(), v as EventListener)
    } else if (v === true) {
      node.setAttribute(k, '')
    } else {
      node.setAttribute(k, String(v))
    }
  }
  for (const child of children) {
    node.append(typeof child === 'string' ? document.createTextNode(child) : child)
  }
  return node
}

/** Imperative SVG from the shared icon registry (CodeMirror panel can't use Icon.svelte). */
function svgIcon(name: IconName): SVGSVGElement {
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

function countAllMatches(
  view: EditorView,
  query: SearchQuery,
): { total: number; capped: boolean } {
  let total = 0
  let capped = false
  const cur = query.getCursor(view.state)
  for (;;) {
    const step = cur.next()
    if (step.done) break
    total++
    if (total >= MATCH_CAP) {
      capped = true
      break
    }
  }
  return { total, capped }
}

/** Index of the selection among matches (1-based), or 0 if not on a match. Stops early. */
function currentMatchIndex(view: EditorView, query: SearchQuery): number {
  const sel = view.state.selection.main
  let i = 0
  const cur = query.getCursor(view.state)
  for (;;) {
    const step = cur.next()
    if (step.done) break
    i++
    if (step.value.from === sel.from && step.value.to === sel.to) return i
    if (step.value.from > sel.to || i >= MATCH_CAP) break
  }
  return 0
}

function setPressed(btn: HTMLButtonElement, on: boolean) {
  btn.setAttribute('aria-pressed', on ? 'true' : 'false')
  btn.classList.toggle('on', on)
}

function isPressed(btn: HTMLButtonElement): boolean {
  return btn.getAttribute('aria-pressed') === 'true'
}

class RecurSearchPanel implements Panel {
  dom: HTMLElement
  readonly top = true
  readonly pos = 80

  private searchField: HTMLInputElement
  private replaceField: HTMLInputElement
  private caseBtn: HTMLButtonElement
  private reBtn: HTMLButtonElement
  private wordBtn: HTMLButtonElement
  private countEl: HTMLElement
  private replaceRow: HTMLElement
  private query: SearchQuery
  /** Cached match total: refreshed on query/doc change, reused for selection-only updates. */
  private totalCache: { key: string; total: number; capped: boolean } | null = null

  constructor(readonly view: EditorView) {
    this.query = getSearchQuery(view.state)

    this.searchField = el('input', {
      type: 'text',
      class: 'cm-recur-search-input',
      name: 'search',
      placeholder: 'Find',
      'aria-label': 'Find',
      'main-field': 'true',
      spellcheck: 'false',
      autocomplete: 'off',
    })
    this.searchField.value = this.query.search
    this.searchField.addEventListener('input', () => this.commitQuery())

    this.replaceField = el('input', {
      type: 'text',
      class: 'cm-recur-search-input',
      name: 'replace',
      placeholder: 'Replace',
      'aria-label': 'Replace',
      spellcheck: 'false',
      autocomplete: 'off',
    })
    this.replaceField.value = this.query.replace
    this.replaceField.addEventListener('input', () => this.commitQuery())

    this.caseBtn = this.makeToggle(svgIcon('case-sensitive'), 'Match case (⌥⌘C)', this.query.caseSensitive)
    this.reBtn = this.makeToggle(svgIcon('regex'), 'Regular expression (⌥⌘R)', this.query.regexp)
    this.wordBtn = this.makeToggle(svgIcon('whole-word'), 'Whole word (⌥⌘W)', this.query.wholeWord)

    this.countEl = el('span', { class: 'cm-recur-search-count', 'aria-live': 'polite' })

    const findRow = el('div', { class: 'cm-recur-search-row' }, [
      el('span', { class: 'cm-recur-search-glyph', 'aria-hidden': 'true' }, [
        svgIcon('search'),
      ]),
      this.searchField,
      this.countEl,
      el('div', { class: 'cm-recur-search-nav' }, [
        this.makeIconBtn('chevron-left', 'Previous match (⇧⌘G)', () => findPrevious(this.view)),
        this.makeIconBtn('chevron-right', 'Next match (⌘G)', () => findNext(this.view)),
      ]),
      el('div', { class: 'cm-recur-search-toggles' }, [
        this.caseBtn,
        this.reBtn,
        this.wordBtn,
      ]),
      this.makeTextBtn('All', 'Select all matches', () => selectMatches(this.view)),
      this.makeIconBtn('x', 'Close', () => closeSearchPanel(this.view), 'cm-recur-search-close'),
    ])

    this.replaceRow = el('div', { class: 'cm-recur-search-row replace' }, [
      el('span', { class: 'cm-recur-search-glyph', 'aria-hidden': 'true' }, [
        svgIcon('replace'),
      ]),
      this.replaceField,
      this.makeTextBtn('Replace', 'Replace', () => replaceNext(this.view)),
      this.makeTextBtn('Replace all', 'Replace all', () => replaceAll(this.view)),
    ])

    this.dom = el('div', { class: 'cm-recur-search' }, [findRow, this.replaceRow])
    this.dom.addEventListener('keydown', (e) => this.keydown(e))

    this.syncReplaceVisibility()
    this.refreshCount()
  }

  private makeToggle(label: string | Node, title: string, on: boolean): HTMLButtonElement {
    const btn = el('button', {
      type: 'button',
      class: 'obs-icon-btn cm-recur-search-toggle',
      title,
      'aria-label': title,
    }, [label])
    setPressed(btn, on)
    btn.addEventListener('click', () => {
      setPressed(btn, !isPressed(btn))
      this.commitQuery()
    })
    return btn
  }

  private makeIconBtn(
    name: IconName,
    title: string,
    onClick: () => void,
    extraClass = '',
  ): HTMLButtonElement {
    const btn = el('button', {
      type: 'button',
      class: `obs-icon-btn${extraClass ? ` ${extraClass}` : ''}`,
      title,
      'aria-label': title,
    }, [svgIcon(name)])
    btn.addEventListener('click', onClick)
    return btn
  }

  private makeTextBtn(label: string, title: string, onClick: () => void): HTMLButtonElement {
    const btn = el('button', {
      type: 'button',
      class: 'cm-recur-search-textbtn',
      title,
    }, [label])
    btn.addEventListener('click', onClick)
    return btn
  }

  private commitQuery() {
    const query = new SearchQuery({
      search: this.searchField.value,
      caseSensitive: isPressed(this.caseBtn),
      regexp: isPressed(this.reBtn),
      wholeWord: isPressed(this.wordBtn),
      replace: this.replaceField.value,
    })
    if (!query.eq(this.query)) {
      this.query = query
      this.view.dispatch({ effects: setSearchQuery.of(query) })
    }
    this.refreshCount()
  }

  private syncReplaceVisibility() {
    this.replaceRow.hidden = this.view.state.readOnly
    this.dom.classList.toggle('has-replace', !this.view.state.readOnly)
  }

  private totalCacheKey(query: SearchQuery): string {
    return [
      query.search,
      query.caseSensitive ? '1' : '0',
      query.regexp ? '1' : '0',
      query.wholeWord ? '1' : '0',
      String(this.view.state.doc.length),
    ].join('\0')
  }

  private refreshCount(recountTotal = false) {
    if (!this.query.valid) {
      this.totalCache = null
      this.countEl.textContent = ''
      this.countEl.dataset.empty = '1'
      this.countEl.classList.remove('none')
      return
    }
    const key = this.totalCacheKey(this.query)
    if (recountTotal || !this.totalCache || this.totalCache.key !== key) {
      const all = countAllMatches(this.view, this.query)
      this.totalCache = { key, total: all.total, capped: all.capped }
    }
    const { total, capped } = this.totalCache
    this.countEl.dataset.empty = '0'
    if (total === 0) {
      this.countEl.textContent = 'No results'
      this.countEl.classList.add('none')
      return
    }
    // Only walk matches when there are some: a zero-total query would otherwise
    // full-scan the doc on every selection change looking for a current index.
    const current = currentMatchIndex(this.view, this.query)
    this.countEl.classList.remove('none')
    const totalLabel = capped ? `${MATCH_CAP}+` : String(total)
    const cur = current > 0 ? String(current) : '-'
    this.countEl.textContent = `${cur} / ${totalLabel}`
  }

  private setQuery(query: SearchQuery) {
    this.query = query
    this.searchField.value = query.search
    this.replaceField.value = query.replace
    setPressed(this.caseBtn, query.caseSensitive)
    setPressed(this.reBtn, query.regexp)
    setPressed(this.wordBtn, query.wholeWord)
    this.refreshCount()
  }

  private keydown(e: KeyboardEvent) {
    if (runScopeHandlers(this.view, e, 'search-panel')) {
      e.preventDefault()
      e.stopPropagation()
      return
    }
    if (e.key === 'Enter' && e.target === this.searchField) {
      e.preventDefault()
      e.stopPropagation()
      ;(e.shiftKey ? findPrevious : findNext)(this.view)
    } else if (e.key === 'Enter' && e.target === this.replaceField) {
      e.preventDefault()
      e.stopPropagation()
      replaceNext(this.view)
    }
  }

  mount() {
    this.searchField.select()
  }

  update(update: ViewUpdate) {
    this.syncReplaceVisibility()
    for (const tr of update.transactions) {
      for (const effect of tr.effects) {
        if (effect.is(setSearchQuery) && !effect.value.eq(this.query)) {
          this.setQuery(effect.value)
        }
      }
    }
    if (update.docChanged) this.refreshCount(true)
    else if (update.selectionSet) this.refreshCount(false)
  }
}

export function createRecurSearchPanel(view: EditorView): Panel {
  return new RecurSearchPanel(view)
}

/** Include in editor extensions so openSearchPanel uses our panel (not stock CM). */
export function recurSearchExtension(): Extension {
  return search({
    top: true,
    createPanel: createRecurSearchPanel,
  })
}
