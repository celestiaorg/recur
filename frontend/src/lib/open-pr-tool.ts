// Display helpers for mcp__recur__open_pr chat cards.
// Keep these pure so the "no JSON dump / friendly label" contracts stay testable
// without mounting AgentChat.

export function isOpenPRTool(tool: string | undefined | null): boolean {
  return !!tool?.endsWith('__open_pr') || tool === 'open_pr'
}

export type OpenPRCard = {
  title: string
  draft: boolean
  number: number
  source: 'gh' | 'local' | ''
  promoted: boolean
  pushed: boolean
  already: boolean
  failed: boolean
  pending: boolean
}

export function parseOpenPRInput(raw: unknown): { title: string; draft: boolean } {
  if (!raw || typeof raw !== 'object') return { title: '', draft: false }
  const o = raw as Record<string, unknown>
  return {
    title: typeof o.title === 'string' ? o.title.trim() : '',
    draft: o.draft === true,
  }
}

export function parseOpenPRCard(
  raw: unknown,
  result: string,
  flags?: { pending?: boolean; error?: boolean },
): OpenPRCard {
  const input = parseOpenPRInput(raw)
  const r = (result ?? '').trim()
  const pending = !!flags?.pending
  const failed = !!flags?.error || (!pending && /^open_pr:/i.test(r))
  const number = Number(r.match(/#(\d+)/)?.[1] || 0) || 0
  const draft = input.draft || /GitHub draft PR/i.test(r)
  let source: OpenPRCard['source'] = ''
  // Local fallback copy mentions "GitHub create failed" — check Local first.
  if (/Opened Local PR|already open \(Local\)/i.test(r)) source = 'local'
  else if (/GitHub/i.test(r)) source = 'gh'
  else if (/Local/i.test(r)) source = 'local'
  return {
    title: input.title,
    draft,
    number,
    source,
    promoted: /promoted from Local/i.test(r),
    pushed: /Pushed new commits/i.test(r),
    already: /already open/i.test(r),
    failed,
    pending,
  }
}

/** Collapsed-row title — never the args JSON (title+body+\n). */
export function openPRLabel(card: OpenPRCard): string {
  if (card.pending) return card.title ? `Opening ${card.title}` : 'Opening pull request'
  if (card.failed) return 'Open PR failed'
  const n = card.number > 0 ? ` #${card.number}` : ''
  if (card.pushed) return `Pushed to PR${n}`
  if (card.already) return `PR${n} already open`
  if (card.source === 'gh' && card.draft) return `Opened GitHub draft PR${n}`
  if (card.source === 'gh') return `Opened GitHub PR${n}`
  if (card.source === 'local') return `Opened Local PR${n}`
  return n ? `Opened PR${n}` : 'Opened pull request'
}
