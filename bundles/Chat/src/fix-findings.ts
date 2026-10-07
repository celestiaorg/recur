import type { PromptContext, PromptResult } from '@recur/sdk'

export default function (_ctx: PromptContext): PromptResult {
  return {
    prompt:
      'Open reviewer findings remain for this workspace. Call mcp__recur__list_findings ' +
      'to load them. Operator notes on a finding are instructions — follow them. For each ' +
      'open finding: verify it against the current code, then either fix it with minimal ' +
      'correct edits and mark it resolved via mcp__recur__update_finding, or mark it ' +
      'invalid with a clear reason if the claim is wrong or out of scope.',
    display: 'Fix open reviewer findings',
  }
}
