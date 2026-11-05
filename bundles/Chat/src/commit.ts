import type { PromptContext, PromptResult } from '@recur/sdk'

export default function (ctx: PromptContext): PromptResult {
  const style = ctx.conventionalCommits
    ? ' Use Conventional Commits (`type(scope): summary`, scope optional).'
    : ''
  return {
    prompt:
      'Commit all changes. Write a clear message from the diff.' +
      style +
      ' Do not push or amend unless I ask. If there’s nothing to commit, say so.',
    display: 'Commit all changes',
  }
}
