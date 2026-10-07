// Bundle [[snippet]] catalog: Tab expansion + optional keyEquivalent wrap.
import { snippet, type CompletionSource } from '@codemirror/autocomplete'
import type { EditorView } from '@codemirror/view'
import { BundleService } from '../../bindings/recur/internal/app'
import type { SnippetInfo } from '../../bindings/recur/internal/app'
import { commandMatches } from './commands'
import { effectiveKeyEquivalent, formatKeyEquivalentDisplay } from './keyequiv'
import { get } from 'svelte/store'
import { settings } from './stores'

export type RegisteredSnippet = {
  id: string
  label: string
  tabTrigger: string
  scope?: string
  fileGlob?: string
  body: string
  order: number
  bundleName?: string
  keyEquivalentRaw?: string
  keyEquivalent?: string
  keyEquivalentDisplay?: string
}

function fromInfo(s: SnippetInfo): RegisteredSnippet {
  const raw = s.keyEquivalent || ''
  const keyEquivalent = effectiveKeyEquivalent(raw)
  return {
    id: s.id,
    label: s.name,
    tabTrigger: s.tabTrigger || '',
    scope: s.scope || undefined,
    fileGlob: s.fileGlob || undefined,
    body: s.body || '',
    order: s.order ?? 0,
    bundleName: s.bundleName?.trim() || undefined,
    keyEquivalentRaw: raw || undefined,
    keyEquivalent: keyEquivalent || undefined,
    keyEquivalentDisplay: keyEquivalent ? formatKeyEquivalentDisplay(keyEquivalent) : undefined,
  }
}

export function disabledSnippetIds(): Set<string> {
  const list = get(settings)?.disabledBundleSnippets
  if (!list?.length) return new Set()
  return new Set(list.filter(Boolean))
}

function filterEnabled(list: RegisteredSnippet[]): RegisteredSnippet[] {
  const disabled = disabledSnippetIds()
  if (disabled.size === 0) return list
  return list.filter((s) => !disabled.has(s.id))
}

function sortSnippets(list: RegisteredSnippet[]): RegisteredSnippet[] {
  return [...list].sort((a, b) => {
    if (a.order !== b.order) return a.order - b.order
    const an = a.bundleName || ''
    const bn = b.bundleName || ''
    if (an !== bn) return an.localeCompare(bn)
    return a.label.localeCompare(b.label)
  })
}

let snippetCaches = new Map<string, { items: RegisteredSnippet[]; at: number }>()
const CACHE_MS = 2000
const CACHE_MAX = 16

async function loadBundleSnippets(scope: string, path: string): Promise<RegisteredSnippet[]> {
  const key = `${scope}\0${path}`
  const hit = snippetCaches.get(key)
  if (hit && Date.now() - hit.at < CACHE_MS) {
    return hit.items
  }
  try {
    const list = await BundleService.ListSnippets(scope, path)
    const items = (list ?? []).map(fromInfo).filter((s) => commandMatches(s, scope, path))
    snippetCaches.set(key, { items, at: Date.now() })
    if (snippetCaches.size > CACHE_MAX) {
      const oldest = snippetCaches.keys().next().value
      if (oldest !== undefined) snippetCaches.delete(oldest)
    }
    return items
  } catch {
    return hit?.items ?? []
  }
}

export function invalidateSnippetCache() {
  snippetCaches.clear()
}

/** Cached matching snippets for this file (empty if this path is not warmed). */
export function cachedSnippets(scope: string, path: string): RegisteredSnippet[] {
  const hit = snippetCaches.get(`${scope}\0${path}`)
  if (!hit) return []
  return filterEnabled(hit.items)
}

/** Tab-trigger glyph for menus (`ife⇥`), empty when the snippet is chord-only. */
export function snippetTriggerHint(tabTrigger: string): string {
  return tabTrigger.trim() ? `${tabTrigger.trim()}⇥` : ''
}

type EditorSnippetHandler = (sn: RegisteredSnippet) => void
let editorSnippetHandler: EditorSnippetHandler | null = null

export function setEditorSnippetHandler(fn: EditorSnippetHandler | null) {
  editorSnippetHandler = fn
}

/** Insert a bundle snippet at the focused editor caret / selection. */
export function runEditorSnippet(sn: RegisteredSnippet): void {
  if (!editorSnippetHandler) {
    throw new Error('No editor is focused — open a file in the Observatory first')
  }
  editorSnippetHandler(sn)
}

export async function listSnippets(scope: string, path: string): Promise<RegisteredSnippet[]> {
  return sortSnippets(filterEnabled(await loadBundleSnippets(scope, path)))
}

export async function listAllSnippetsUnfiltered(): Promise<RegisteredSnippet[]> {
  try {
    const list = await BundleService.ListSnippets('', '')
    return sortSnippets((list ?? []).map(fromInfo))
  } catch {
    return []
  }
}

/** Warm the 2s cache so Tab keymap can expand synchronously. */
export function prefetchSnippets(scope: string, path: string): void {
  void loadBundleSnippets(scope, path)
}

/** Range `insertSnippet` should replace: selection, else word/line/document, else caret. */
export function commandSnippetRange(
  selFrom: number,
  selTo: number,
  inputMode: string | undefined,
  word: { from: number; to: number } | null,
  line: { from: number; to: number } | null,
  docLen: number,
): { from: number; to: number } {
  if (selFrom !== selTo) return { from: selFrom, to: selTo }
  if (inputMode === 'word' && word) return { from: word.from, to: word.to }
  if (inputMode === 'line' && line) return { from: line.from, to: line.to }
  if (inputMode === 'document') return { from: 0, to: docLen }
  return { from: selFrom, to: selTo }
}

/** Word immediately before `pos` matching tabTrigger rules (letter/_ then word chars). */
export function triggerWordBefore(doc: string, pos: number): { from: number; to: number; word: string } | null {
  let i = pos
  while (i > 0) {
    const c = doc[i - 1]!
    if (!/[A-Za-z0-9_]/.test(c)) break
    i--
  }
  if (i === pos) return null
  const word = doc.slice(i, pos)
  if (!/^[A-Za-z_][A-Za-z0-9_]*$/.test(word)) return null
  return { from: i, to: pos, word }
}

/**
 * CM has no `$` escape (`\{`/`\}` only), so `$1` / `${…}` in a wrap selection
 * cannot be neutralized in the template. applySnippet swaps those for a
 * sentinel before substituteSnippetVars and restores them after snippet().
 */
export function snippetNeedsShield(s: string): boolean {
  return /[#$]\{|\$\d|[{}]/.test(s)
}

export function isoDate(d = new Date()): string {
  const y = d.getFullYear()
  const m = String(d.getMonth() + 1).padStart(2, '0')
  const day = String(d.getDate()).padStart(2, '0')
  return `${y}-${m}-${day}`
}

export function substituteSnippetVars(body: string, selectedText: string, now = new Date()): string {
  const date = isoDate(now)
  return body
    .replace(/\$\{TM_SELECTED_TEXT\}/g, selectedText)
    .replace(/\$TM_SELECTED_TEXT\b/g, selectedText)
    .replace(/\$\{ISO_DATE\}/g, date)
    .replace(/\$ISO_DATE\b/g, date)
}

/**
 * CodeMirror's snippet() only parses `${…}` / `#{…}`, not VS Code `$1` / `$0`.
 * Bare `$n` (and empty `${n}`) inherit the first `${n:default}` so mirrors
 * like `</$1>` keep the default text at every site.
 */
export function toCodeMirrorSnippet(body: string): string {
  const defaults = new Map<number, string>()
  const withDefault = /\$\{(\d+):([^{}]*)\}/g
  let m: RegExpExecArray | null
  while ((m = withDefault.exec(body))) {
    const n = Number(m[1])
    if (!defaults.has(n)) defaults.set(n, m[2] ?? '')
  }
  return body.replace(/(?<!\\)\$\{(\d+)\}|(?<!\\)\$(?!\{)(\d+)/g, (all, braced, bare) => {
    const n = Number(braced ?? bare)
    const d = defaults.get(n)
    if (d !== undefined) return `\${${n}:${d}}`
    return `\${${n}}`
  })
}

export function applySnippet(
  view: EditorView,
  sn: RegisteredSnippet,
  from: number,
  to: number,
  selectedText?: string,
): boolean {
  const sel = selectedText ?? view.state.doc.sliceString(from, to)
  const shield = snippetNeedsShield(sel)
  const token = shield ? `RSEL${Math.random().toString(36).slice(2, 10)}` : sel
  const body = toCodeMirrorSnippet(substituteSnippetVars(sn.body, token))
  try {
    snippet(body)(view, null as never, from, to)
  } catch {
    view.dispatch({
      changes: { from, to, insert: body },
      selection: { anchor: from + body.length },
    })
    return true
  }
  if (shield) replaceSnippetToken(view, token, sel)
  return true
}

function replaceSnippetToken(view: EditorView, token: string, text: string): void {
  const doc = view.state.doc.toString()
  const changes: { from: number; to: number; insert: string }[] = []
  for (let i = doc.indexOf(token); i >= 0; i = doc.indexOf(token, i + token.length)) {
    changes.push({ from: i, to: i + token.length, insert: text })
  }
  if (changes.length) view.dispatch({ changes })
}

/**
 * Expand a tab-trigger at the cursor. Returns true when Tab was consumed
 * (single match applied, or a multi-match picker opened).
 */
export function tryExpandSnippet(
  view: EditorView,
  scope: string,
  path: string,
  onPickMany: (matches: RegisteredSnippet[], from: number, to: number) => void,
): boolean {
  const sel = view.state.selection.main
  if (sel.from !== sel.to) return false
  const line = view.state.doc.lineAt(sel.head)
  const found = triggerWordBefore(line.text, sel.head - line.from)
  if (!found) return false
  const from = line.from + found.from
  const to = line.from + found.to
  const matches = cachedSnippets(scope, path).filter((s) => s.tabTrigger === found.word)
  if (!matches.length) return false
  if (matches.length === 1) {
    return applySnippet(view, matches[0]!, from, to, '')
  }
  onPickMany(matches, from, to)
  return true
}

/** Tab-trigger snippets whose trigger starts with `prefix` (case-insensitive). */
export function snippetsMatchingPrefix(
  snippets: RegisteredSnippet[],
  prefix: string,
  explicit: boolean,
): RegisteredSnippet[] {
  const withTrig = snippets.filter((s) => s.tabTrigger)
  if (!prefix) return explicit ? withTrig : []
  const lower = prefix.toLowerCase()
  return withTrig.filter((s) => s.tabTrigger.toLowerCase().startsWith(lower))
}

/**
 * Completion source for the editor autocomplete tooltip. Uses the 2s snippet
 * cache (and loads on a miss). Disabled ids are omitted. Snippets without a
 * tabTrigger (keyEquivalent-only wrap) stay out of the list.
 */
export function bundleSnippetCompletionSource(
  getScopePath: () => { scope: string; path: string },
): CompletionSource {
  return async (context) => {
    const found = context.matchBefore(/[A-Za-z_][A-Za-z0-9_]*/)
    if (!found && !context.explicit) return null
    const prefix = found?.text ?? ''
    if (!prefix && !context.explicit) return null
    const { scope, path } = getScopePath()
    const matches = snippetsMatchingPrefix(
      filterEnabled(await loadBundleSnippets(scope, path)),
      prefix,
      context.explicit,
    )
    if (!matches.length) return null
    return {
      from: found?.from ?? context.pos,
      validFor: /^[A-Za-z_][A-Za-z0-9_]*$/,
      options: matches.map((sn) => ({
        label: sn.tabTrigger,
        detail: sn.label,
        type: 'snippet',
        boost: 99,
        apply: (view, _completion, from, to) => {
          applySnippet(view, sn, from, to, '')
        },
      })),
    }
  }
}
