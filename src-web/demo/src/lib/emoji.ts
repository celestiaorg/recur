/** GitHub gemoji aliases (`:rocket:` → 🚀). Generated from github/gemoji db/emoji.json. */
import { get, writable } from 'svelte/store'
import shortcodes from './emoji-shortcodes.json'
import { prepareFuzzy, fuzzyFilterPrepared, type FuzzyItem } from './fuzzy'

export const EMOJI_SHORTCODES = shortcodes as Record<string, string>

export type EmojiHit = { name: string; emoji: string }

export type EmojiSuggest = {
  el: HTMLTextAreaElement | HTMLInputElement
  start: number
  query: string
  hits: EmojiHit[]
  sel: number
  set: (next: string) => void
} | null

export const emojiSuggest = writable<EmojiSuggest>(null)

/** GitHub-style `:name:` — letters, digits, underscore, plus, hyphen. */
const SHORTCODE_RE = /:([a-z0-9_+-]+):/gi

export function emojiForShortcode(name: string): string | undefined {
  return EMOJI_SHORTCODES[name.toLowerCase()]
}

/** Replace completed `:name:` tokens. Leaves unknown names and code spans alone when `skipCode` is set. */
export function expandEmojiShortcodes(src: string, opts?: { skipCode?: boolean }): string {
  if (!src || !src.includes(':')) return src
  if (!opts?.skipCode) return replaceShortcodes(src)
  return src.split(/(```[\s\S]*?```|`[^`]+`)/).map((part, i) => (i % 2 === 1 ? part : replaceShortcodes(part))).join('')
}

function replaceShortcodes(src: string): string {
  return src.replace(SHORTCODE_RE, (m, name: string) => EMOJI_SHORTCODES[name.toLowerCase()] ?? m)
}

/** Expand shortcodes and shift `caret` so it stays after the same logical point. */
export function expandEmojiShortcodesAt(src: string, caret: number): { text: string; caret: number } | null {
  if (!src.includes(':')) return null
  let nextCaret = caret
  const text = src.replace(SHORTCODE_RE, (m, name: string, offset: number) => {
    const emoji = EMOJI_SHORTCODES[name.toLowerCase()]
    if (!emoji) return m
    if (offset < caret) nextCaret += emoji.length - m.length
    return emoji
  })
  if (text === src) return null
  return { text, caret: Math.max(0, nextCaret) }
}

export type EmojiQuery = { start: number; query: string }

/** In-progress `:name` at the caret. Empty query is a lone `:` at a word boundary. */
export function emojiQueryAt(text: string, caret: number): EmojiQuery | null {
  if (caret < 1 || caret > text.length) return null
  let i = caret - 1
  while (i >= 0 && /[a-zA-Z0-9_+-]/.test(text[i]!)) i--
  if (i < 0 || text[i] !== ':') return null
  if (i > 0 && /[A-Za-z0-9_:]/.test(text[i - 1]!)) return null
  const query = text.slice(i + 1, caret)
  if (query.includes(':')) return null
  return { start: i, query }
}

let prepared: FuzzyItem<EmojiHit>[] | null = null

function preparedShortcodes(): FuzzyItem<EmojiHit>[] {
  if (!prepared) {
    prepared = prepareFuzzy(
      Object.entries(EMOJI_SHORTCODES).map(([name, emoji]) => ({ name, emoji })),
      (x) => x.name,
    )
  }
  return prepared
}

/** GitHub's default `:` list (minus `1234`), plus rocket and fire. */
export const DEFAULT_EMOJI_NAMES = ['+1', '-1', '100', '1st_place_medal', 'rocket', 'fire'] as const

function defaultEmojiHits(): EmojiHit[] {
  return DEFAULT_EMOJI_NAMES.flatMap((name) => {
    const emoji = EMOJI_SHORTCODES[name]
    return emoji ? [{ name, emoji }] : []
  })
}

export function suggestEmoji(query: string, limit = 8): EmojiHit[] {
  const q = query.trim()
  if (!q) return defaultEmojiHits()
  return fuzzyFilterPrepared(preparedShortcodes(), q, limit).map((h) => h.item)
}

export function applyEmojiSuggestion(state: NonNullable<EmojiSuggest>, hit: EmojiHit) {
  const caret = state.el.selectionStart ?? state.start + 1 + state.query.length
  const next = state.el.value.slice(0, state.start) + hit.emoji + state.el.value.slice(caret)
  const pos = state.start + hit.emoji.length
  state.set(next)
  emojiSuggest.set(null)
  requestAnimationFrame(() => {
    state.el.setSelectionRange(pos, pos)
    state.el.focus()
  })
}

/** `{@attach}` — expand `:name:` as you type, and offer a typeahead for `:partial`. */
export function emojiShortcodes(set: (next: string) => void) {
  return (el: HTMLElement) => {
    if (!(el instanceof HTMLTextAreaElement) && !(el instanceof HTMLInputElement)) return
    let escaped: { start: number; query: string } | null = null

    const syncSuggest = (text: string, caret: number) => {
      const q = emojiQueryAt(text, caret)
      if (!q || (escaped && escaped.start === q.start && escaped.query === q.query)) {
        emojiSuggest.set(null)
        return
      }
      escaped = null
      const hits = suggestEmoji(q.query)
      if (!hits.length) {
        emojiSuggest.set(null)
        return
      }
      emojiSuggest.update((cur) => {
        const same = cur?.el === el && cur.query === q.query && cur.start === q.start
        const sel = same ? Math.min(cur.sel, hits.length - 1) : 0
        return { el, start: q.start, query: q.query, hits, sel, set }
      })
    }

    const onInput = () => {
      const caret = el.selectionStart ?? el.value.length
      const hit = expandEmojiShortcodesAt(el.value, caret)
      if (hit) {
        set(hit.text)
        requestAnimationFrame(() => {
          el.setSelectionRange(hit.caret, hit.caret)
          syncSuggest(hit.text, hit.caret)
        })
        return
      }
      syncSuggest(el.value, caret)
    }

    const onKey = (e: KeyboardEvent) => {
      const open = get(emojiSuggest)
      if (!open || open.el !== el) return
      if (e.key === 'ArrowDown') {
        e.preventDefault()
        e.stopImmediatePropagation()
        emojiSuggest.update((s) => s && s.el === el ? { ...s, sel: Math.min(s.sel + 1, s.hits.length - 1) } : s)
        return
      }
      if (e.key === 'ArrowUp') {
        e.preventDefault()
        e.stopImmediatePropagation()
        emojiSuggest.update((s) => s && s.el === el ? { ...s, sel: Math.max(s.sel - 1, 0) } : s)
        return
      }
      if ((e.key === 'Enter' || e.key === 'Tab') && !e.metaKey && !e.ctrlKey) {
        const hit = open.hits[open.sel]
        if (!hit) return
        e.preventDefault()
        e.stopImmediatePropagation()
        applyEmojiSuggestion(open, hit)
        return
      }
      if (e.key === 'Escape') {
        e.preventDefault()
        e.stopImmediatePropagation()
        escaped = { start: open.start, query: open.query }
        emojiSuggest.set(null)
      }
    }

    const onBlur = (e: Event) => {
      const next = e instanceof FocusEvent ? e.relatedTarget : null
      if (next instanceof Node && document.querySelector('.emoji-suggest')?.contains(next)) return
      queueMicrotask(() => {
        if (document.activeElement === el) return
        emojiSuggest.update((s) => (s?.el === el ? null : s))
      })
    }

    el.addEventListener('input', onInput)
    window.addEventListener('keydown', onKey, true)
    el.addEventListener('blur', onBlur)
    return () => {
      el.removeEventListener('input', onInput)
      window.removeEventListener('keydown', onKey, true)
      el.removeEventListener('blur', onBlur)
      emojiSuggest.update((s) => (s?.el === el ? null : s))
    }
  }
}

