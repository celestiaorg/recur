/** @vitest-environment happy-dom */
import { describe, expect, it } from 'vitest'
import type { Project } from '../../bindings/recur/internal/domain/models'
import { defaultRun, filterRuns, projectRuns, runLabel, orderRuns } from './run-commands'

function proj(extra: Partial<Project> = {}): Project {
  return {
    id: 'p',
    name: 'p',
    pic: 'P',
    color: 'var(--cyan)',
    open: true,
    idle: false,
    repoPath: '/repo',
    baseBranch: 'main',
    setupCmd: '',
    teardownCmd: '',
    appURL: '',
    prOnGithub: false,
    isGit: true,
    ...extra,
  } as Project
}

describe('projectRuns', () => {
  it('returns empty when no runCommands', () => {
    expect(projectRuns(proj())).toEqual([])
  })

  it('reads explicit runCommands', () => {
    expect(projectRuns(proj({
      runCommands: [
        { name: 'dev', cmd: 'wails3 task dev' },
        { name: 'test', cmd: 'go test' },
      ],
    }))).toEqual([
      { name: 'dev', cmd: 'wails3 task dev' },
      { name: 'test', cmd: 'go test' },
    ])
  })
})

describe('defaultRun', () => {
  it('uses pinnedRun when present', () => {
    const p = proj({
      runCommands: [
        { name: 'dev', cmd: 'a' },
        { name: 'test', cmd: 'b' },
      ],
      pinnedRun: 'test',
    })
    expect(defaultRun(p)).toEqual({ name: 'test', cmd: 'b' })
  })

  it('falls back to first when pin is missing', () => {
    const p = proj({
      runCommands: [{ name: 'dev', cmd: 'a' }, { name: 'test', cmd: 'b' }],
      pinnedRun: 'nope',
    })
    expect(defaultRun(p)?.name).toBe('dev')
  })
})

describe('runLabel', () => {
  it('says Run/Stop plus the command name', () => {
    expect(runLabel('Run', 'dev')).toBe('Run dev')
    expect(runLabel('Stop', 'test')).toBe('Stop test')
  })
})

describe('filterRuns', () => {
  const list = [
    { name: 'dev', cmd: 'wails3 task dev' },
    { name: 'test:go', cmd: 'wails3 task test:go' },
    { name: 'tidy', cmd: 'wails3 task common:go:mod:tidy' },
  ]

  it('keeps file order when the query is empty', () => {
    expect(filterRuns(list, '')).toEqual(list)
    expect(filterRuns(list, '   ')).toEqual(list)
  })

  it('matches a name substring and hides the rest', () => {
    expect(filterRuns(list, 'tid').map((r) => r.name)).toEqual(['tidy'])
  })

  it('matches a command substring', () => {
    expect(filterRuns(list, 'test:go').map((r) => r.name)).toEqual(['test:go'])
  })
})

describe('orderRuns', () => {
  const list = [{ name: 'dev' }, { name: 'test' }, { name: 'lint' }]

  it('keeps file order when nothing is pinned', () => {
    expect(orderRuns(list, '')).toEqual(list)
  })

  it('moves the pinned name to the front', () => {
    expect(orderRuns(list, 'test').map((r) => r.name)).toEqual(['test', 'dev', 'lint'])
  })
})
