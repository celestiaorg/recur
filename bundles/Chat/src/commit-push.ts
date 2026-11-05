import type { PromptContext, PromptResult } from '@recur/sdk'

export default function (ctx: PromptContext): PromptResult {
  const style = ctx.conventionalCommits
    ? ' Use Conventional Commits (`type(scope): summary`, scope optional).'
    : ''
  return {
    prompt:
      'Commit all changes, then push to the remote tracking branch (create/set ' +
      'upstream if needed). Write a clear message from the diff.' +
      style +
      ' Do not amend unless I ask. If there’s nothing to commit, still push any ' +
      'unpushed commits; if everything is already pushed, say so.',
    display: 'Commit all changes and push',
  }
}
