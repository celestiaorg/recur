import { describe, expect, it } from 'vitest'
import {
  applyThreadResolved,
  applyThreadResolvedHolds,
  canDeletePRComment,
  canEditPRComment,
  canReactPRComment,
  canResolveLineNote,
  commentCountForPath,
  commentMatchesThread,
  commentSource,
  commentsForPath,
  isDroppedComment,
  isLineNote,
  lineThreadKey,
  isLineNoteRoot,
  isPendingComment,
  isTimelineRoot,
  keepPendingReplies,
  lineNotesOnRow,
  lineNotesCoveringRow,
  lineNoteRange,
  reviewChildren,
  sameCommentPath,
  sameThreadReply,
  setThreadEditorDraft,
  settlePostedReply,
  threadEditorDraft,
  threadReplies,
  unresolvedLineNotesByPath,
} from './comment-thread'

describe('isTimelineRoot', () => {
  it('drops the description and nested line notes', () => {
    expect(isTimelineRoot({ kind: 'description' })).toBe(false)
    expect(isTimelineRoot({ parentId: 'gh-review-1' })).toBe(false)
    expect(isTimelineRoot({ inReplyToId: 'gh-inline-1' })).toBe(false)
    expect(isTimelineRoot({ kind: 'hidden' })).toBe(false)
    expect(isTimelineRoot({ kind: 'comment' })).toBe(true)
    expect(isTimelineRoot({ kind: 'action' })).toBe(true)
    expect(isTimelineRoot(null)).toBe(false)
  })
})

describe('reviewChildren', () => {
  it('returns notes for that review, oldest first', () => {
    const items = [
      { id: 'b', parentId: 'r1', at: '2026-08-21T11:31:00Z' },
      { id: 'a', parentId: 'r1', at: '2026-08-21T11:30:00Z' },
      { id: 'c', parentId: 'r2', at: '2026-08-21T11:30:00Z' },
      { id: 'd', parentId: 'r1', inReplyToId: 'a', at: '2026-08-21T11:32:00Z' },
    ]
    expect(reviewChildren(items, 'r1').map((c) => c.id)).toEqual(['a', 'b'])
    expect(reviewChildren(items, '')).toEqual([])
  })
})

describe('threadReplies', () => {
  it('returns replies for that line note', () => {
    const items = [
      { id: 'r', inReplyToId: 'gh-inline-1', at: '2026-08-21T11:32:00Z' },
      { id: 'q', inReplyToId: '1', at: '2026-08-21T11:31:00Z' },
      { id: 'x', inReplyToId: 'gh-inline-2', at: '2026-08-21T11:31:00Z' },
    ]
    expect(threadReplies(items, 'gh-inline-1').map((c) => c.id)).toEqual(['q', 'r'])
  })
})

describe('isLineNote', () => {
  it('detects file-anchored notes and replies', () => {
    expect(isLineNote({ id: 'gh-inline-1' })).toBe(true)
    expect(isLineNote({ path: 'a.go' })).toBe(true)
    expect(isLineNote({ inReplyToId: 'gh-inline-1' })).toBe(true)
    expect(isLineNote({ id: 'c1' })).toBe(false)
    expect(isLineNote(null)).toBe(false)
  })
})

describe('lineNotesOnRow', () => {
  const hmm = { id: 'gh-inline-1', path: 'src/auth/oauth.ts', line: 12, side: 'RIGHT', bodyMd: 'hmm' }
  const left = { id: 'gh-inline-2', path: 'src/auth/oauth.ts', line: 12, side: 'LEFT', bodyMd: 'old' }
  const reply = { id: 'gh-inline-3', path: 'src/auth/oauth.ts', line: 12, side: 'RIGHT', inReplyToId: 'gh-inline-1', bodyMd: 'thanks' }
  const other = { id: 'gh-inline-4', path: 'src/other.ts', line: 12, side: 'RIGHT', bodyMd: 'nope' }

  it('matches a RIGHT note on the added/head line', () => {
    expect(sameCommentPath('src/auth/oauth.ts', 'oauth.ts')).toBe(true)
    expect(isLineNoteRoot(hmm)).toBe(true)
    expect(isLineNoteRoot(reply)).toBe(false)
    expect(lineNotesOnRow([hmm, left, reply, other], 'src/auth/oauth.ts', { old: '', new: '12', type: 'add' }).map((c) => c.id)).toEqual(['gh-inline-1'])
    expect(lineNotesOnRow([hmm, left], 'src/auth/oauth.ts', { old: '12', new: '', type: 'del' }).map((c) => c.id)).toEqual(['gh-inline-2'])
    expect(lineNotesOnRow([hmm], 'src/auth/oauth.ts', { old: '12', new: '12', type: 'ctx' }, 'right').map((c) => c.id)).toEqual(['gh-inline-1'])
  })

  it('marks every row in a GitHub multiline range', () => {
    const span = { id: 'gh-inline-5', path: 'src/auth/oauth.ts', line: 12, startLine: 11, side: 'RIGHT', bodyMd: 'span' }
    expect(lineNoteRange(span)).toEqual({ start: 11, end: 12 })
    expect(lineNotesCoveringRow([span], 'src/auth/oauth.ts', { old: '11', new: '11', type: 'ctx' }).map((c) => c.id)).toEqual(['gh-inline-5'])
    expect(lineNotesCoveringRow([span], 'src/auth/oauth.ts', { old: '', new: '12', type: 'add' }).map((c) => c.id)).toEqual(['gh-inline-5'])
    expect(lineNotesCoveringRow([span], 'src/auth/oauth.ts', { old: '10', new: '10', type: 'ctx' })).toEqual([])
    expect(lineNotesOnRow([span], 'src/auth/oauth.ts', { old: '11', new: '11', type: 'ctx' })).toEqual([])
    expect(lineNotesOnRow([span], 'src/auth/oauth.ts', { old: '', new: '12', type: 'add' }).map((c) => c.id)).toEqual(['gh-inline-5'])
  })

  it('does not anchor outdated GitHub comments to the current row', () => {
    const stale = { id: 'gh-inline-9', path: 'src/auth/oauth.ts', line: 12, startLine: 7, side: 'RIGHT', outdated: true, bodyMd: 'stale' }
    expect(isLineNoteRoot(stale)).toBe(false)
    expect(lineNotesOnRow([stale], 'src/auth/oauth.ts', { old: '', new: '12', type: 'add' })).toEqual([])
    expect(lineNotesCoveringRow([stale], 'src/auth/oauth.ts', { old: '', new: '12', type: 'add' })).toEqual([])
    expect(unresolvedLineNotesByPath([stale], ['src/auth/oauth.ts']).size).toBe(0)
  })
})

describe('unresolvedLineNotesByPath', () => {
  const open = { id: 'gh-inline-1', path: 'src/auth/oauth.ts', line: 12, bodyMd: 'hmm' }
  const later = { id: 'gh-inline-2', path: 'src/auth/oauth.ts', line: 4, bodyMd: 'earlier line' }
  const resolved = { id: 'gh-inline-3', path: 'src/hooks/useOAuthSession.ts', line: 8, resolved: true, bodyMd: 'done' }
  const reply = { id: 'gh-inline-4', path: 'src/auth/oauth.ts', line: 12, inReplyToId: 'gh-inline-1', bodyMd: 'thanks' }

  it('groups open roots, skips resolved threads and replies', () => {
    const byPath = unresolvedLineNotesByPath(
      [open, later, resolved, reply],
      ['src/auth/oauth.ts', 'src/hooks/useOAuthSession.ts'],
    )
    expect([...byPath.keys()]).toEqual(['src/auth/oauth.ts'])
    expect(byPath.get('src/auth/oauth.ts')?.map((c) => c.id)).toEqual(['gh-inline-2', 'gh-inline-1'])
    expect(commentsForPath(byPath, 'src/auth/oauth.ts')).toHaveLength(2)
    expect(commentCountForPath(byPath, 'src/auth', true)).toBe(2)
    expect(commentCountForPath(byPath, 'src/hooks/useOAuthSession.ts')).toBe(0)
  })
})

describe('pending replies', () => {
  const pending = { id: 'c-pending-1', inReplyToId: 'gh-inline-1', bodyMd: 'thanks' }
  const echoed = { id: 'gh-inline-9', inReplyToId: '1', bodyMd: 'thanks' }

  it('identifies optimistic ids', () => {
    expect(isPendingComment(pending)).toBe(true)
    expect(isPendingComment(echoed)).toBe(false)
  })

  it('pairs a pending reply with the GH echo on the same thread', () => {
    expect(sameThreadReply(pending, echoed)).toBe(true)
    expect(sameThreadReply(pending, { ...echoed, bodyMd: 'later' })).toBe(false)
    expect(sameThreadReply(pending, { ...echoed, inReplyToId: 'gh-inline-2' })).toBe(false)
  })

  it('keeps a pending reply until GitHub echoes it', () => {
    const root = { id: 'gh-inline-1', bodyMd: 'hmm' }
    expect(keepPendingReplies([root, pending], [root])).toEqual([root, pending])
    expect(keepPendingReplies([root, pending], [root, echoed])).toEqual([root, echoed])
  })

  it('does not drop a second identical pending when an older thanks is already on the thread', () => {
    const older = { id: 'gh-inline-8', inReplyToId: 'gh-inline-1', bodyMd: 'thanks' }
    const nextPending = { id: 'c-pending-2', inReplyToId: 'gh-inline-1', bodyMd: 'thanks' }
    expect(keepPendingReplies([older, nextPending], [older])).toEqual([older, nextPending])
  })

  it('swaps the pending row for the posted comment', () => {
    const root = { id: 'gh-inline-1', bodyMd: 'hmm' }
    expect(settlePostedReply([root, pending], pending.id, echoed)).toEqual([root, echoed])
    expect(settlePostedReply([root, pending, echoed], pending.id, echoed)).toEqual([root, echoed])
    expect(settlePostedReply([root, pending], pending.id, null)).toEqual([root])
  })
})

describe('comment permissions', () => {
  const note = { id: 'gh-inline-1', path: 'a.ts', author: 'Avery Rourke', authorLogin: 'avery', bodyMd: 'hmm' }
  const writer = { source: 'gh', viewerLogin: 'avery', viewerCanWrite: true }
  const other = { source: 'gh', viewerLogin: 'pat', viewerCanWrite: false }
  const writeOnly = { source: 'gh', viewerLogin: 'pat', viewerCanWrite: true }

  it('lets the author edit; writers can delete a line note they did not write', () => {
    expect(commentSource(note)).toBe('hmm')
    expect(canEditPRComment(writer, note)).toBe(true)
    expect(canEditPRComment(other, note)).toBe(false)
    expect(canDeletePRComment(writer, note)).toBe(true)
    expect(canDeletePRComment(writeOnly, note)).toBe(true)
    expect(canDeletePRComment(other, note)).toBe(false)
    expect(isDroppedComment({ id: 'gh-inline-2', inReplyToId: 'gh-inline-1' }, 'gh-inline-1')).toBe(true)
  })

  it('hides edit and delete on pending replies', () => {
    const pending = { id: 'c-pending-1', path: 'a.ts', authorLogin: 'avery' }
    expect(canEditPRComment(writer, pending)).toBe(false)
    expect(canDeletePRComment(writer, pending)).toBe(false)
  })

  it('lets you delete a standalone conversation comment, but not a submitted review', () => {
    const conv = { id: 'gh-comment-IC_1', authorLogin: 'avery', bodyMd: 'looks good' }
    const other = { id: 'gh-comment-IC_2', authorLogin: 'sam', bodyMd: 'nit' }
    const review = { id: 'gh-review-PRR_1', authorLogin: 'avery', bodyMd: 'ship it' }
    expect(canDeletePRComment(writer, conv)).toBe(true)
    expect(canDeletePRComment(writeOnly, other)).toBe(true)
    expect(canDeletePRComment({ source: 'gh', viewerLogin: 'pat', viewerCanWrite: false }, other)).toBe(false)
    expect(canDeletePRComment(writer, review)).toBe(false)
    expect(canDeletePRComment(writer, { id: 'gh-description', kind: 'description', authorLogin: 'avery' })).toBe(false)
  })

  it('lets any signed-in viewer react on conversation items', () => {
    const conv = { id: 'gh-comment-IC_1', authorLogin: 'avery' }
    const reviewAction = { id: 'gh-review-action-1', kind: 'action', verb: 'approved' }
    const emptyReview = { id: 'gh-review-1', kind: 'comment', verb: 'approved' }
    const reviewBody = { id: 'gh-review-1', kind: 'comment', verb: 'approved', bodyMd: 'ship it' }
    const merge = { id: 'gh-event-1', kind: 'action' }
    const commit = { id: 'gh-event-c', kind: 'action', commitSha: 'abc' }
    const pending = { id: 'c-pending-1' }
    expect(canReactPRComment(writer, conv)).toBe(true)
    expect(canReactPRComment(other, conv)).toBe(true)
    expect(canReactPRComment(writer, reviewAction)).toBe(false)
    expect(canReactPRComment(writer, emptyReview)).toBe(false)
    expect(canReactPRComment(writer, reviewBody)).toBe(true)
    expect(canReactPRComment(writer, { id: 'gh-description', kind: 'description' })).toBe(true)
    expect(canReactPRComment(writer, merge)).toBe(false)
    expect(canReactPRComment(writer, commit)).toBe(false)
    expect(canReactPRComment(writer, pending)).toBe(false)
    expect(canReactPRComment({ source: 'gh' }, conv)).toBe(false)
    expect(canReactPRComment({ source: 'local' }, conv)).toBe(true)
  })
})

describe('canResolveLineNote', () => {
  const note = { id: 'gh-inline-1', path: 'a.ts', threadId: 'PRRT_1' }
  const writer = { source: 'gh', viewerLogin: 'avery', authorLogin: 'loopbot', viewerCanWrite: true }
  const author = { source: 'gh', viewerLogin: 'loopbot', authorLogin: 'loopbot', viewerCanWrite: false }
  const reader = { source: 'gh', viewerLogin: 'pat', authorLogin: 'loopbot', viewerCanWrite: false }

  it('allows writers and the PR author when GitHub has a thread id', () => {
    expect(lineThreadKey(note)).toBe('PRRT_1')
    expect(canResolveLineNote(writer, note)).toBe(true)
    expect(canResolveLineNote(author, note)).toBe(true)
    expect(canResolveLineNote(reader, note)).toBe(false)
  })

  it('needs a GitHub thread id, but local notes always resolve', () => {
    expect(canResolveLineNote(writer, { ...note, threadId: '' })).toBe(false)
    expect(canResolveLineNote({ source: 'local' }, { id: 'c1', path: 'a.ts' })).toBe(true)
    expect(canResolveLineNote(writer, { id: 'c1' })).toBe(false)
  })
})

describe('thread resolved holds', () => {
  const root = { id: 'gh-inline-1', threadId: 'PRRT_1', resolved: false }
  const reply = { id: 'gh-inline-2', threadId: 'PRRT_1', inReplyToId: 'gh-inline-1', resolved: false }
  const other = { id: 'gh-inline-3', threadId: 'PRRT_2', resolved: false }

  it('matches a thread by id or threadId', () => {
    expect(commentMatchesThread(root, 'PRRT_1')).toBe(true)
    expect(commentMatchesThread(root, 'gh-inline-1')).toBe(true)
    expect(commentMatchesThread(reply, 'PRRT_1')).toBe(true)
    expect(commentMatchesThread(other, 'PRRT_1')).toBe(false)
  })

  it('paints every note on that thread', () => {
    expect(applyThreadResolved([root, reply, other], 'PRRT_1', true)).toEqual([
      { ...root, resolved: true },
      { ...reply, resolved: true },
      other,
    ])
  })

  it('reapplies a hold over a stale GetPR snapshot', () => {
    const stale = [
      { ...root, resolved: false },
      { ...reply, resolved: false },
      other,
    ]
    expect(applyThreadResolvedHolds(stale, { PRRT_1: true })).toEqual([
      { ...root, resolved: true },
      { ...reply, resolved: true },
      other,
    ])
  })
})

describe('thread editor drafts', () => {
  it('survives unmount and drops when empty', () => {
    const id = 'gh-inline-draft-1'
    setThreadEditorDraft(id, { replyOpen: true, reply: 'half a thought', editingId: '', edit: '' })
    expect(threadEditorDraft(id)).toEqual({ replyOpen: true, reply: 'half a thought', editingId: '', edit: '' })
    setThreadEditorDraft(id, { replyOpen: false, reply: '', editingId: 'c2', edit: 'typo fix' })
    expect(threadEditorDraft(id)).toEqual({ replyOpen: false, reply: '', editingId: 'c2', edit: 'typo fix' })
    setThreadEditorDraft(id, { replyOpen: false, reply: '', editingId: '', edit: '' })
    expect(threadEditorDraft(id)).toBeUndefined()
  })
})
