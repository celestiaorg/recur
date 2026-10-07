import type { PromptContext, PromptResult } from '@recur/sdk'

export default function (_ctx: PromptContext): PromptResult {
  return {
    prompt:
      'Commit all changes. Write a clear message from the diff. Do not push or ' +
      'amend unless I ask. If there’s nothing to commit, say so.',
    display: 'Commit all changes',
  }
}
