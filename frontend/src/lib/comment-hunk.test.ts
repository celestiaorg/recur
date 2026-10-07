import { describe, expect, it } from 'vitest'
import { clipCommentHunk, commentHunkLineLabel, commentHunkPathLabel, parseDiffHunk } from './comment-hunk'

describe('parseDiffHunk', () => {
  it('is empty for a blank hunk', () => {
    expect(parseDiffHunk('')).toEqual([])
    expect(parseDiffHunk('   ')).toEqual([])
  })

  it('numbers add and del rows from the @@ header', () => {
    const hunk = [
      '@@ -16,2 +16,2 @@ Enable deposit',
      '-VITE_ENABLE_FORWARDING_DEPOSIT=false',
      '+VITE_ENABLE_FORWARDING_DEPOSIT=true',
    ].join('\n')
    expect(parseDiffHunk(hunk)).toEqual([
      { type: 'del', old: '16', new: '', text: 'VITE_ENABLE_FORWARDING_DEPOSIT=false' },
      { type: 'add', old: '', new: '16', text: 'VITE_ENABLE_FORWARDING_DEPOSIT=true' },
    ])
  })

  it('keeps context lines on both sides', () => {
    const hunk = ['@@ -1,3 +1,3 @@', ' keep', '-old', '+new'].join('\n')
    expect(parseDiffHunk(hunk)).toEqual([
      { type: 'ctx', old: '1', new: '1', text: 'keep' },
      { type: 'del', old: '2', new: '', text: 'old' },
      { type: 'add', old: '', new: '2', text: 'new' },
    ])
  })
})

describe('clipCommentHunk', () => {
  const long = [
    '@@ -4,9 +4,9 @@ provider registry',
    ' import { secrets } from \'./vault\'',
    ' const providers = []',
    ' function register() {',
    '   return providers',
    ' }',
    ' const unused = 1',
    ' const unused2 = 2',
    ' const unused3 = 3',
    '-const secret = process.env.OAUTH_CLIENT_SECRET',
    '+const secret = await secrets.get(\'oauth/client_secret\')',
  ].join('\n')

  it('keeps three context rows plus the commented line', () => {
    const clipped = clipCommentHunk(parseDiffHunk(long), { line: 12, side: 'RIGHT' })
    expect(clipped.map((l) => l.text)).toEqual([
      'const unused2 = 2',
      'const unused3 = 3',
      'const secret = process.env.OAUTH_CLIENT_SECRET',
      'const secret = await secrets.get(\'oauth/client_secret\')',
    ])
    expect(clipped.at(-1)?.target).toBe(true)
    expect(clipped.at(-1)?.new).toBe('12')
    expect(clipped.some((l) => l.text.includes('function register'))).toBe(false)
  })

  it('anchors a LEFT comment on the deleted line', () => {
    const clipped = clipCommentHunk(parseDiffHunk(long), { line: 12, side: 'LEFT', context: 1 })
    expect(clipped.map((l) => l.text)).toEqual([
      'const unused3 = 3',
      'const secret = process.env.OAUTH_CLIENT_SECRET',
    ])
    expect(clipped.at(-1)?.type).toBe('del')
    expect(clipped.at(-1)?.target).toBe(true)
  })

  it('falls back to the last hunk row when the line is missing', () => {
    const clipped = clipCommentHunk(parseDiffHunk(long), { line: 99, side: 'RIGHT', context: 0 })
    expect(clipped).toHaveLength(1)
    expect(clipped[0].target).toBe(true)
    expect(clipped[0].text).toContain('secrets.get')
  })

  it('keeps a multiline range plus context above the start', () => {
    const clipped = clipCommentHunk(parseDiffHunk(long), { line: 12, side: 'RIGHT', startLine: 10, context: 1 })
    expect(clipped.map((l) => l.text)).toEqual([
      'const unused = 1',
      'const unused2 = 2',
      'const unused3 = 3',
      'const secret = process.env.OAUTH_CLIENT_SECRET',
      'const secret = await secrets.get(\'oauth/client_secret\')',
    ])
    expect(clipped.filter((l) => l.inRange).map((l) => l.text)).toEqual([
      'const unused2 = 2',
      'const unused3 = 3',
      'const secret = process.env.OAUTH_CLIENT_SECRET',
      'const secret = await secrets.get(\'oauth/client_secret\')',
    ])
    expect(clipped.at(-1)?.target).toBe(true)
  })
})

describe('commentHunkPathLabel', () => {
  it('appends the line when GitHub gave one', () => {
    expect(commentHunkLineLabel(12)).toBe('12')
    expect(commentHunkLineLabel(12, 10)).toBe('10-12')
    expect(commentHunkPathLabel('src/auth/oauth.ts', 12)).toBe('src/auth/oauth.ts:12')
    expect(commentHunkPathLabel('src/auth/oauth.ts', 12, 10)).toBe('src/auth/oauth.ts:10-12')
    expect(commentHunkPathLabel('src/auth/oauth.ts', 0)).toBe('src/auth/oauth.ts')
    expect(commentHunkPathLabel('', 12)).toBe('')
    expect(commentHunkPathLabel('b.go', 9, 7, true)).toBe('b.go:7-9 (outdated)')
  })
})
