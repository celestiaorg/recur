import type { PromptCompleteContext, PromptCompleteResult } from '@recur/sdk'

export default function (ctx: PromptCompleteContext): PromptCompleteResult {
  const all = ctx.diagnostics ?? []
  if (!all.length) return { hits: [], message: 'No language server problems in this workspace' }
  const terms = ctx.args.toLowerCase().split(/\s+/).filter(Boolean)
  const hits = all
    .filter((d) => terms.every((t) => d.toLowerCase().includes(t)))
    .slice(0, 25)
    .map((d) => {
      const m = /^(.*?:\d+:\d+) (\w+)(?: \([^)]*\))?: (.*)$/.exec(d)
      return m
        ? { insert: d, label: m[3], detail: m[1], badge: m[2] }
        : { insert: d, label: d }
    })
  return { hits }
}
