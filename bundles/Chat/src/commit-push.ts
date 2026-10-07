import type { PromptContext, PromptResult } from '@recur/sdk'

export default function (_ctx: PromptContext): PromptResult {
  return {
    prompt:
      'Commit all changes, then push to the remote tracking branch (create/set ' +
      'upstream if needed). Write a clear message from the diff. Do not amend ' +
      'unless I ask. If there’s nothing to commit, still push any unpushed ' +
      'commits; if everything is already pushed, say so.',
    display: 'Commit all changes and push',
  }
}
