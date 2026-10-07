import { describe, expect, it } from 'vitest'
import { commentPermalink, githubBlobUrl, githubCommitUrl, githubNodeDatabaseID } from './comment-permalink'

function encodeNode(prefix: string, typeId: number, dbId: number): string {
  const buf = new Uint8Array(8)
  const view = new DataView(buf.buffer)
  view.setUint32(0, typeId)
  view.setUint32(4, dbId)
  let bin = ''
  for (const b of buf) bin += String.fromCharCode(b)
  return `${prefix}_${btoa(bin).replace(/=+$/, '')}`
}

describe('commentPermalink', () => {
  const pr = 'https://github.com/acme/app/pull/12'

  it('prefers the stored GitHub URL', () => {
    expect(commentPermalink(pr, { id: 'c1', url: 'https://github.com/acme/app/pull/12#issuecomment-9' }))
      .toBe('https://github.com/acme/app/pull/12#issuecomment-9')
  })

  it('uses the stored #issue- permalink for the description', () => {
    expect(commentPermalink(pr, { id: 'gh-description', kind: 'description', url: `${pr}#issue-5215972950` }))
      .toBe(`${pr}#issue-5215972950`)
  })

  it('falls back to the PR page when the description has no issue id', () => {
    expect(commentPermalink(pr, { id: 'gh-description', kind: 'description' }))
      .toBe(pr)
  })

  it('derives fragments from Recur comment ids', () => {
    expect(commentPermalink(pr, { id: 'gh-inline-99' })).toBe(`${pr}#discussion_r99`)
    expect(commentPermalink(pr, { id: 'gh-comment-8' })).toBe(`${pr}#issuecomment-8`)
    expect(commentPermalink(pr, { id: 'gh-review-3' })).toBe(`${pr}#pullrequestreview-3`)
    expect(commentPermalink(pr, { id: 'gh-review-action-3' })).toBe(`${pr}#pullrequestreview-3`)
  })

  it('derives a review permalink from a Copilot GraphQL node id', () => {
    const id = encodeNode('PRR', 8, 3864739160)
    expect(commentPermalink(pr, { id: `gh-review-${id}` }))
      .toBe(`${pr}#pullrequestreview-3864739160`)
  })

  it('returns empty when there is no GitHub URL', () => {
    expect(commentPermalink('', { id: 'c1' })).toBe('')
    expect(commentPermalink(pr, { id: 'c1' })).toBe('')
    expect(commentPermalink(pr, { id: 'gh-review-PRR_c' })).toBe('')
  })
})

describe('githubCommitUrl', () => {
  it('turns a PR url into a commit url', () => {
    expect(githubCommitUrl('https://github.com/acme/app/pull/12', 'a3f73e2abcdef'))
      .toBe('https://github.com/acme/app/commit/a3f73e2abcdef')
  })

  it('strips a fragment and trailing slash', () => {
    expect(githubCommitUrl('https://github.com/acme/app/pull/12/#event-1', 'abc1234'))
      .toBe('https://github.com/acme/app/commit/abc1234')
  })

  it('returns empty without a repo or sha', () => {
    expect(githubCommitUrl('', 'abc1234')).toBe('')
    expect(githubCommitUrl('https://github.com/acme/app/pull/12', '')).toBe('')
    expect(githubCommitUrl('not a url', 'abc1234')).toBe('')
  })
})

describe('githubBlobUrl', () => {
  it('links a file and line at a ref', () => {
    expect(githubBlobUrl('https://github.com/acme/app/pull/12', 'abc1234', 'src/a b.ts', 7))
      .toBe('https://github.com/acme/app/blob/abc1234/src/a%20b.ts#L7')
  })

  it('returns empty without a repo, ref, or path', () => {
    expect(githubBlobUrl('', 'abc1234', 'a.ts')).toBe('')
    expect(githubBlobUrl('https://github.com/acme/app/pull/12', '', 'a.ts')).toBe('')
    expect(githubBlobUrl('https://github.com/acme/app/pull/12', 'abc1234', '')).toBe('')
  })
})

describe('githubNodeDatabaseID', () => {
  it('reads a numeric id', () => {
    expect(githubNodeDatabaseID('99')).toBe('99')
  })

  it('reads a next-gen node id', () => {
    expect(githubNodeDatabaseID(encodeNode('PRR', 8, 3864739160))).toBe('3864739160')
  })

  it('reads a legacy node id', () => {
    expect(githubNodeDatabaseID(btoa('012:PullRequestReview42'))).toBe('42')
  })
})
