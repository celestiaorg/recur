import type { PromptContext, PromptResult } from '@recur/sdk'

export default function (ctx: PromptContext): PromptResult {
  const problem = (ctx.args ?? '').trim()
  if (!problem) return { prompt: '', display: 'Pass the diagnostic to fix' }
  return {
    prompt:
      `Fix this language server diagnostic:\n\n${problem}\n\n` +
      'Verify it against the current code, then make the minimal correct fix.',
    display: `Fix ${problem.split('\n')[0]}`,
  }
}
