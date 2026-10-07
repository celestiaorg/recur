import { describe, expect, test } from 'vitest'
import {
  countLspProblems,
  groupLspProblemsByPath,
  lspProblemsForWorkspace,
  type LspProblem,
} from './lsp-problems'

function problem(partial: Partial<LspProblem> & Pick<LspProblem, 'uri' | 'path' | 'message'>): LspProblem {
  return {
    severity: 'error',
    line: 1,
    character: 0,
    endLine: 1,
    endCharacter: 1,
    ...partial,
  }
}

describe('lspProblemsForWorkspace', () => {
  const a = problem({ uri: 'file:///a/x.go', path: 'x.go', message: 'A', workspaceId: 'ws-a', sessionId: 's1' })
  const b = problem({ uri: 'file:///b/x.go', path: 'x.go', message: 'B', workspaceId: 'ws-b', sessionId: 's2', severity: 'warning' })

  test('restricts the list, counts, and path badges to one workspace', () => {
    const all = [a, b]
    const mine = lspProblemsForWorkspace(all, 'ws-b')
    expect(mine).toEqual([b])
    expect(countLspProblems(mine)).toEqual({ errors: 0, warnings: 1, total: 1 })
    expect(groupLspProblemsByPath(mine).get('x.go')).toEqual({
      errors: 0,
      warnings: 1,
      count: 1,
      severity: 'warning',
    })
  })

  test('leaves the list unchanged when workspaceId is empty', () => {
    const all = [a, b]
    expect(lspProblemsForWorkspace(all, '')).toBe(all)
  })
})
