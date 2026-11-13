import type { PromptContext, PromptResult } from '@recur/sdk'

export default function (ctx: PromptContext): PromptResult {
  const err = (ctx.args ?? '').trim()
  if (!err) return { prompt: '', display: 'Pass the merge error to fix' }
  return {
    prompt:
      `Merging this workspace into its base failed:\n\n${err}\n\n` +
      'Find the cause and fix it so the merge can be retried. Typical fixes: update this branch ' +
      'with its base (fetch first) and resolve any conflicts, or fast-forward a local base ' +
      'branch that is behind its remote. Do not merge into or push the base branch yourself: ' +
      'Recur lands it when I click Merge again. Ask before committing, stashing or discarding ' +
      'uncommitted changes, here or in the project folder.',
    display: 'Fix the failed merge',
  }
}
