import { PRsService } from '../../bindings/recur/internal/app'
import { get, writable } from 'svelte/store'
import { mentionQueryAt } from './agentchat'
import { prepareFuzzy, fuzzyFilterPrepared } from './fuzzy'
import { prPeopleWithReviews, type PRPerson } from './pr-participants'
import { prs, selectedPRId } from './stores'

export { mentionQueryAt }

export type UserMentionSuggest = {
  el: HTMLTextAreaElement | HTMLInputElement
  start: number
  query: string
  hits: PRPerson[]
  sel: number
  set: (next: string) => void
} | null

export const userMentionSuggest = writable<UserMentionSuggest>(null)

export function mentionLogin(p: PRPerson): string {
  return (p.login ?? '').trim().replace(/^@/, '')
}

export function ghPRMentionPeople(): PRPerson[] {
  const id = get(selectedPRId)
  if (!id) return []
  const pr = get(prs).find((p) => p.id === id)
  if (pr?.source !== 'gh') return []
  return prPeopleWithReviews(pr).filter((p) => mentionLogin(p))
}

export function suggestUserMentions(people: PRPerson[], query: string, limit = 8): PRPerson[] {
  const withLogin = people.filter((p) => mentionLogin(p))
  const q = query.trim()
  if (!q) return withLogin.slice(0, limit)
  return fuzzyFilterPrepared(
    prepareFuzzy(withLogin, (p) => `${mentionLogin(p)} ${p.name ?? ''}`.trim()),
    q,
    limit,
  ).map((h) => h.item)
}

export function mergeMentionHits(local: PRPerson[], remote: PRPerson[] | null | undefined, limit = 8): PRPerson[] {
  const out: PRPerson[] = []
  const seen = new Set<string>()
  for (const p of [...local, ...(remote ?? [])]) {
    const login = mentionLogin(p).toLowerCase()
    if (!login || seen.has(login)) continue
    seen.add(login)
    out.push(p)
    if (out.length >= limit) break
  }
  return out
}

export function applyUserMention(state: NonNullable<UserMentionSuggest>, hit: PRPerson) {
  const login = mentionLogin(hit)
  if (!login) return
  const caret = state.el.selectionStart ?? state.start + 1 + state.query.length
  const now = mentionQueryAt(state.el.value, caret)
  if (!now || now.start !== state.start || now.query !== state.query) {
    userMentionSuggest.set(null)
    return
  }
  const insert = `@${login} `
  const next = state.el.value.slice(0, now.start) + insert + state.el.value.slice(now.end)
  const pos = now.start + insert.length
  state.set(next)
  userMentionSuggest.set(null)
  requestAnimationFrame(() => {
    state.el.setSelectionRange(pos, pos)
    state.el.focus()
  })
}

/** `{@attach}` — GitHub `@login` typeahead. */
export function userMentions(set: (next: string) => void) {
  return (el: HTMLElement) => {
    if (!(el instanceof HTMLTextAreaElement) && !(el instanceof HTMLInputElement)) return
    let escaped: { start: number; query: string } | null = null
    let seq = 0
    let timer: ReturnType<typeof setTimeout> | undefined

    const showHits = (q: { start: number; query: string }, hits: PRPerson[]) => {
      if (!hits.length) {
        userMentionSuggest.set(null)
        return
      }
      userMentionSuggest.update((cur) => {
        const same = cur?.el === el && cur.query === q.query && cur.start === q.start
        let sel = 0
        if (same) {
          const login = mentionLogin(cur.hits[cur.sel] ?? {})
          const idx = login
            ? hits.findIndex((h) => mentionLogin(h).toLowerCase() === login.toLowerCase())
            : -1
          sel = idx >= 0 ? idx : 0
        }
        return { el, start: q.start, query: q.query, hits, sel, set }
      })
    }

    const syncSuggest = (text: string, caret: number) => {
      const q = mentionQueryAt(text, caret)
      if (!q || (escaped && escaped.start === q.start && escaped.query === q.query)) {
        seq++
        clearTimeout(timer)
        userMentionSuggest.set(null)
        return
      }
      escaped = null
      const local = suggestUserMentions(ghPRMentionPeople(), q.query)
      showHits(q, local)
      const mySeq = ++seq
      const prId = get(selectedPRId)
      clearTimeout(timer)
      if (!prId) return
      timer = setTimeout(() => {
        void PRsService.SearchPRMentionableUsers(prId, q.query)
          .then((remote) => {
            if (mySeq !== seq) return
            const now = mentionQueryAt(el.value, el.selectionStart ?? el.value.length)
            if (!now || now.start !== q.start || now.query !== q.query) return
            const rows = Array.isArray(remote) ? remote : []
            showHits(q, mergeMentionHits(suggestUserMentions(ghPRMentionPeople(), q.query), rows))
          })
          .catch(() => {})
      }, 150)
    }

    const onInput = () => {
      syncSuggest(el.value, el.selectionStart ?? el.value.length)
    }

    const onSel = () => {
      const open = get(userMentionSuggest)
      if (!open || open.el !== el) return
      syncSuggest(el.value, el.selectionStart ?? el.value.length)
    }

    const onKey = (e: KeyboardEvent) => {
      const open = get(userMentionSuggest)
      if (!open || open.el !== el) return
      if (e.key === 'ArrowDown') {
        e.preventDefault()
        e.stopImmediatePropagation()
        userMentionSuggest.update((s) => s && s.el === el ? { ...s, sel: Math.min(s.sel + 1, s.hits.length - 1) } : s)
        return
      }
      if (e.key === 'ArrowUp') {
        e.preventDefault()
        e.stopImmediatePropagation()
        userMentionSuggest.update((s) => s && s.el === el ? { ...s, sel: Math.max(s.sel - 1, 0) } : s)
        return
      }
      if ((e.key === 'Enter' || e.key === 'Tab') && !e.metaKey && !e.ctrlKey) {
        const hit = open.hits[open.sel]
        if (!hit) return
        e.preventDefault()
        e.stopImmediatePropagation()
        applyUserMention(open, hit)
        return
      }
      if (e.key === 'Escape') {
        e.preventDefault()
        e.stopImmediatePropagation()
        seq++
        clearTimeout(timer)
        escaped = { start: open.start, query: open.query }
        userMentionSuggest.set(null)
      }
    }

    const onBlur = (e: Event) => {
      const next = e instanceof FocusEvent ? e.relatedTarget : null
      if (next instanceof Node && document.querySelector('.user-mention')?.contains(next)) return
      queueMicrotask(() => {
        if (document.activeElement === el) return
        seq++
        clearTimeout(timer)
        userMentionSuggest.update((s) => (s?.el === el ? null : s))
      })
    }

    el.addEventListener('input', onInput)
    document.addEventListener('selectionchange', onSel)
    window.addEventListener('keydown', onKey, true)
    el.addEventListener('blur', onBlur)
    return () => {
      seq++
      clearTimeout(timer)
      el.removeEventListener('input', onInput)
      document.removeEventListener('selectionchange', onSel)
      window.removeEventListener('keydown', onKey, true)
      el.removeEventListener('blur', onBlur)
      userMentionSuggest.update((s) => (s?.el === el ? null : s))
    }
  }
}
