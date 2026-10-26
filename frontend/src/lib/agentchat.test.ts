import { afterEach, describe, expect, test, vi } from 'vitest'
import {
  FILE_AT_RE,
  AGENT_ERROR_PREFIX,
  CHAT_RETRY_DISPLAY,
  addTool,
  addUser,
  attachToolResult,
  agentErrorDetail,
  agentErrorTitle,
  stepRowKey,
  isTaskTool,
  isSubagentLaunchMeta,
  displayUserChatText,
  attachmentMarker,
  pendingSendBubbleBody,
  sameUserChatBody,
  extractFileMentions,
  extractTerminalMentions,
  extractArtifactMentions,
  fileMentionLabel,
  fileMentionToken,
  dirFileMentionPath,
  isDirFileMention,
  terminalMentionToken,
  terminalMentionLabel,
  artifactMentionToken,
  artifactMentionLabel,
  uniqueArtifactIdForTitle,
  reservedAtName,
  atNameIsFileMention,
  finishTurn,
  formatAskUserDisplayText,
  lastAssistantTextInTurn,
  stillOpenFindingsForTurn,
  currentTurnHasEnded,
  dropLiveAfterFooter,
  shouldRefreshTurnStamp,
  shouldIgnoreStaleIdle,
  shouldSkipIdleTranscriptReset,
  endTurn,
  optionChoiceText,
  formatFileMentionChip,
  formatFileMentionRef,
  isAgentErrorReply,
  mapChatItems,
  mentionQueryAt,
  mergePersistedWithLive,
  retainOptimisticUsers,
  parseFileMentionChip,
  prepareFileMentionInsert,
  removeFileMentionToken,
  repairFileMentionBoundaries,
  skillQueryAt,
  stripFileMentions,
  stripComposerMarkers,
  applyCompact,
  applyNotice,
  applyFallback,
  applyStream,
  noteThinkingTokens,
  dropOpenCompact,
  chatImageURL,
  draftsFromImagePaths,
  dropOptimisticUser,
  mergeRejectedSteerDraft,
  mergeRejectedSteerAttachments,
} from './agentchat'
import type { WorkspaceStep } from './api'
import { transcriptMessageBlockLocal } from './caches/chat-transcript-ui'
import type { AgentMsg } from '../components/AgentChat.svelte'

describe('mergePersistedWithLive', () => {
  const history: AgentMsg[] = [
    { me: true, body: 'first question' },
    { body: 'first answer' },
    { me: true, body: 'second question' },
    { body: 'second answer' },
  ]

  test('keeps persisted when live is empty', () => {
    expect(mergePersistedWithLive(history, [])).toEqual(history)
  })

  test('keeps live when persisted is empty', () => {
    const live: AgentMsg[] = [{ body: 'streaming…', streaming: true }]
    expect(mergePersistedWithLive([], live)).toEqual(live)
  })

  test('grafts a stream-only tail onto persisted history', () => {
    const live: AgentMsg[] = [{ body: 'mid-stream…', streaming: true }]
    expect(mergePersistedWithLive(history, live)).toEqual([
      ...history,
      { body: 'mid-stream…', streaming: true },
    ])
  })

  test('grafts pending tools and streaming text onto history', () => {
    const live: AgentMsg[] = [
      { tool: 'Read', toolId: 't1', resultPending: true },
      { body: 'after the read…', streaming: true },
    ]
    expect(mergePersistedWithLive(history, live)).toEqual([...history, ...live])
  })

  test('keeps live when it already has the full conversation', () => {
    const live: AgentMsg[] = [
      ...history,
      { me: true, body: 'third question' },
      { body: 'working…', streaming: true },
    ]
    expect(mergePersistedWithLive(history, live)).toEqual(live)
  })

  test('preserves an optimistic user bubble the store has not recorded yet', () => {
    const live: AgentMsg[] = [{ me: true, body: 'just sent', optimistic: true }]
    expect(mergePersistedWithLive(history, live)).toEqual([
      ...history,
      { me: true, body: 'just sent', optimistic: true },
    ])
  })

  test('keeps a resend of the same text as the previous turn', () => {
    const live: AgentMsg[] = [{ me: true, body: 'second question', optimistic: true }]
    expect(mergePersistedWithLive(history, live)).toEqual([
      ...history,
      { me: true, body: 'second question', optimistic: true },
    ])
  })

  test('does not duplicate a user bubble already at the end of persisted', () => {
    const live: AgentMsg[] = [
      { me: true, body: 'second question' },
      { body: 'mid-stream…', streaming: true },
    ]
    expect(mergePersistedWithLive(history, live)).toEqual([
      ...history,
      { body: 'mid-stream…', streaming: true },
    ])
  })

  test('does not duplicate a pending tool already in the store snapshot', () => {
    const persisted: AgentMsg[] = [
      ...history,
      { me: true, body: 'third' },
      { tool: 'Read', toolInput: 'app.go', resultPending: true },
    ]
    const live: AgentMsg[] = [
      { tool: 'Read', toolInput: 'app.go', resultPending: true, toolId: 't1' },
      { body: 'mid-stream…', streaming: true },
    ]
    expect(mergePersistedWithLive(persisted, live)).toEqual([
      ...history,
      { me: true, body: 'third' },
      { tool: 'Read', toolInput: 'app.go', resultPending: true, toolId: 't1' },
      { body: 'mid-stream…', streaming: true },
    ])
  })

  test('keeps settled live rows that post-date the persisted snapshot', () => {
    const persisted: AgentMsg[] = [...history, { me: true, body: 'third' }]
    const live: AgentMsg[] = [
      { tool: 'Read', toolInput: 'app.go', result: 'ok' },
      { body: 'mid-stream…', streaming: true },
    ]
    expect(mergePersistedWithLive(persisted, live)).toEqual([
      ...history,
      { me: true, body: 'third' },
      { tool: 'Read', toolInput: 'app.go', result: 'ok' },
      { body: 'mid-stream…', streaming: true },
    ])
  })

  test('overlays a live tool result onto a still-pending store snapshot', () => {
    const persisted: AgentMsg[] = [
      ...history,
      { me: true, body: 'third' },
      { tool: 'Read', toolInput: 'app.go', resultPending: true },
    ]
    const live: AgentMsg[] = [
      { tool: 'Read', toolInput: 'app.go', result: 'ok' },
      { body: 'mid-stream…', streaming: true },
    ]
    expect(mergePersistedWithLive(persisted, live)).toEqual([
      ...history,
      { me: true, body: 'third' },
      { tool: 'Read', toolInput: 'app.go', result: 'ok' },
      { body: 'mid-stream…', streaming: true },
    ])
  })

  test('does not duplicate history when live is one row behind the store snapshot', () => {
    // Fetch response beat onSessionStep: live has history + optimistic user,
    // persisted already has that user plus a tool. Appending all of live would
    // render the conversation twice.
    const live: AgentMsg[] = [
      ...history,
      { me: true, body: 'third', optimistic: true },
    ]
    const persisted: AgentMsg[] = [
      ...history,
      { me: true, body: 'third' },
      { tool: 'Read', toolInput: 'app.go', resultPending: true },
    ]
    expect(mergePersistedWithLive(persisted, live)).toEqual(persisted)
  })

  test('aligns an optimistic current send when live includes conversation history', () => {
    const prior: AgentMsg[] = [
      { me: true, body: 'first' },
      { body: 'answer' },
      { turnEnd: true, durationMs: 1 },
    ]
    const marked = `${attachmentMarker('img_a')}review this`
    const persisted: AgentMsg[] = [
      ...prior,
      { me: true, body: marked },
      { body: 'I will fix it' },
      { tool: 'Read', resultPending: true },
    ]
    const live: AgentMsg[] = [
      ...prior,
      { me: true, body: marked, optimistic: true },
    ]
    expect(mergePersistedWithLive(persisted, live)).toEqual(persisted)
    expect(mergePersistedWithLive(
      [...persisted, { turnEnd: true, durationMs: 1 }],
      live,
    )).toEqual([...persisted, { turnEnd: true, durationMs: 1 }])
  })

  test('keeps an optimistic repeated steer while session.user is pending', () => {
    const persisted: AgentMsg[] = [
      { me: true, body: 'please stop' },
      { tool: 'Read', result: 'ok' },
    ]
    const live: AgentMsg[] = [
      ...persisted,
      { me: true, body: 'please stop', optimistic: true, pendingId: 'new-steer' },
    ]
    expect(mergePersistedWithLive(persisted, live)).toEqual(live)
  })

  test('appends live streaming after a shared history prefix', () => {
    const live: AgentMsg[] = [
      ...history,
      { me: true, body: 'third', optimistic: true },
      { body: 'mid-stream…', streaming: true },
    ]
    const persisted: AgentMsg[] = [
      ...history,
      { me: true, body: 'third' },
      { tool: 'Read', toolInput: 'app.go', resultPending: true },
      { tool: 'Grep', toolInput: 'TODO', resultPending: true },
    ]
    expect(mergePersistedWithLive(persisted, live)).toEqual([
      ...persisted,
      { body: 'mid-stream…', streaming: true },
    ])
  })

  test('does not duplicate a settled current-turn row already in the store snapshot', () => {
    // Store is one tool ahead; live still has the optimistic user + thinking.
    // After dropping the optimistic user, thinking must not be appended again.
    const think: AgentMsg = { thinking: true, body: 'weighing options' }
    const live: AgentMsg[] = [
      ...history,
      { me: true, body: 'third', optimistic: true },
      think,
    ]
    const persisted: AgentMsg[] = [
      ...history,
      { me: true, body: 'third' },
      think,
      { tool: 'Read', toolInput: 'app.go', resultPending: true },
    ]
    expect(mergePersistedWithLive(persisted, live)).toEqual(persisted)
  })

  test('drops queued follow-ups from the grafted tail', () => {
    const live: AgentMsg[] = [
      { body: 'mid-stream…', streaming: true },
      { me: true, body: 'follow-up', queued: true, pendingId: 'p1' },
    ]
    expect(mergePersistedWithLive(history, live)).toEqual([
      ...history,
      { body: 'mid-stream…', streaming: true },
    ])
  })

  test('does not overlay a new dispatch onto an earlier turn with the same display', () => {
    const prompt = 'Please review the changes in this worktree and file any real issues'
    const persisted: AgentMsg[] = [
      { me: true, body: prompt },
      { body: "I'll review the committed Tetris changes" },
      { turnEnd: true, durationMs: 1 },
      { me: true, body: prompt },
    ]
    const live: AgentMsg[] = [
      { me: true, body: prompt },
      { thinking: true, body: 'Reviewing the diff', streaming: true },
    ]
    expect(mergePersistedWithLive(persisted, live)).toEqual([
      { me: true, body: prompt },
      { body: "I'll review the committed Tetris changes" },
      { turnEnd: true, durationMs: 1 },
      { me: true, body: prompt },
      { thinking: true, body: 'Reviewing the diff', streaming: true },
    ])
  })

  test('heap remount does not duplicate streaming text when the store is ahead', () => {
    // Seeded live is the full transcript (warm remount). Store snapshot has
    // the same turn plus a later tool and settled the live streaming row.
    // Concatenating live's stream onto persisted doubled bubbles until idle.
    const persisted: AgentMsg[] = [
      ...history,
      { me: true, body: 'third' },
      { thinking: true, body: 'plan' },
      { tool: 'Read', toolInput: 'app.go', resultPending: true },
      { body: 'hello world' },
      { tool: 'Grep', toolInput: 'TODO', resultPending: true },
    ]
    const live: AgentMsg[] = [
      ...history,
      { me: true, body: 'third' },
      { thinking: true, body: 'plan' },
      { tool: 'Read', toolInput: 'app.go', resultPending: true, toolId: 't1' },
      { body: 'hello world', streaming: true },
    ]
    expect(mergePersistedWithLive(persisted, live)).toEqual([
      ...history,
      { me: true, body: 'third' },
      { thinking: true, body: 'plan' },
      { tool: 'Read', toolInput: 'app.go', resultPending: true, toolId: 't1' },
      { body: 'hello world', streaming: true },
      { tool: 'Grep', toolInput: 'TODO', resultPending: true },
    ])
  })

  test('does not duplicate a queued-flush user that persist kept with attachment markers', () => {
    const marked = `${attachmentMarker('img_a')}pr settings alignment`
    const persisted: AgentMsg[] = [
      { me: true, body: marked },
      { tool: 'Read', toolInput: 'app.go', resultPending: true },
    ]
    const live: AgentMsg[] = [{ me: true, body: 'pr settings alignment', optimistic: true }]
    expect(mergePersistedWithLive(persisted, live)).toEqual(persisted)
  })

  test('turn-end resync does not replay the queued user after the footer', () => {
    const marked = `${attachmentMarker('img_a')}pr settings alignment`
    const persisted: AgentMsg[] = [
      { me: true, body: marked },
      { tool: 'Read', toolInput: 'app.go', result: 'ok' },
      { tool: 'Grep', toolInput: 'TODO', result: 'none' },
      { body: 'fixed' },
      { turnEnd: true, durationMs: 1 },
    ]
    const live: AgentMsg[] = [
      { me: true, body: 'pr settings alignment', optimistic: true },
      { me: true, body: marked },
      { tool: 'Read', toolInput: 'app.go', result: 'ok' },
      { body: 'fixed' },
    ]
    const got = mergePersistedWithLive(persisted, live)
    expect(got.filter((m) => m.me)).toHaveLength(1)
    expect(got.filter((m) => m.me)[0]?.body).toBe(marked)
    expect(got.some((m) => m.turnEnd)).toBe(true)
  })

  test('keeps an optimistic resend after the previous turn already has output', () => {
    const persisted: AgentMsg[] = [
      { me: true, body: 'first' },
      { body: 'answer 1' },
      { turnEnd: true, durationMs: 1 },
      { me: true, body: 'again' },
      { body: 'answer 2' },
      { turnEnd: true, durationMs: 1 },
    ]
    const live: AgentMsg[] = [
      { me: true, body: 'again', optimistic: true },
      { body: 'new response', streaming: true },
    ]
    expect(mergePersistedWithLive(persisted, live)).toEqual([
      ...persisted,
      { me: true, body: 'again', optimistic: true },
      { body: 'new response', streaming: true },
    ])
  })

  test('overlays a live tool result when the live tail still has the optimistic user', () => {
    const persisted: AgentMsg[] = [
      ...history,
      { me: true, body: 'third' },
      { tool: 'Read', toolInput: 'app.go', resultPending: true },
    ]
    const live: AgentMsg[] = [
      { me: true, body: 'third', optimistic: true },
      { tool: 'Read', toolInput: 'app.go', toolId: 't1', result: 'ok' },
      { body: 'mid-stream…', streaming: true },
    ]
    expect(mergePersistedWithLive(persisted, live)).toEqual([
      ...history,
      { me: true, body: 'third' },
      { tool: 'Read', toolInput: 'app.go', toolId: 't1', result: 'ok', resultPending: false },
      { body: 'mid-stream…', streaming: true },
    ])
  })

  test('drops a lone settled replay of the last persisted user', () => {
    const persisted: AgentMsg[] = [
      { me: true, body: 'first' },
      { body: 'answer 1' },
      { turnEnd: true, durationMs: 1 },
      { me: true, body: 'second question' },
      { body: 'answer 2' },
      { turnEnd: true, durationMs: 1 },
    ]
    const live: AgentMsg[] = [{ me: true, body: 'second question' }]
    expect(mergePersistedWithLive(persisted, live)).toEqual(persisted)
  })

  test('overlays a live tool result after assistant text already started', () => {
    const persisted: AgentMsg[] = [
      ...history,
      { me: true, body: 'third' },
      { body: 'I will read the file' },
      { tool: 'Read', toolInput: 'app.go', resultPending: true },
    ]
    const live: AgentMsg[] = [
      { me: true, body: 'third', optimistic: true },
      { body: 'I will read the file' },
      { tool: 'Read', toolInput: 'app.go', toolId: 't1', result: 'ok' },
    ]
    expect(mergePersistedWithLive(persisted, live)).toEqual([
      ...history,
      { me: true, body: 'third' },
      { body: 'I will read the file' },
      { tool: 'Read', toolInput: 'app.go', toolId: 't1', result: 'ok', resultPending: false },
    ])
  })

  test('does not overlay a resend onto a completed turn that read the same file', () => {
    const persisted: AgentMsg[] = [
      { me: true, body: 'first' },
      { body: 'answer' },
      { turnEnd: true, durationMs: 1 },
      { me: true, body: 'review again' },
      { tool: 'Read', toolInput: 'app.go', toolId: 'old', result: 'old contents' },
      { body: 'done' },
      { turnEnd: true, durationMs: 1 },
    ]
    const live: AgentMsg[] = [
      { me: true, body: 'review again', optimistic: true },
      { tool: 'Read', toolInput: 'app.go', toolId: 'new', result: 'new contents' },
      { body: 'new response', streaming: true },
    ]
    expect(mergePersistedWithLive(persisted, live)).toEqual([...persisted, ...live])
  })
})

describe('retainOptimisticUsers', () => {
  test('keeps an optimistic user the store has not recorded yet', () => {
    const live: AgentMsg[] = [{ me: true, body: 'review the auth flow', optimistic: true }]
    expect(retainOptimisticUsers([], live)).toEqual(live)
  })

  test('drops the optimistic user once the store has the same text', () => {
    const mapped: AgentMsg[] = [{ me: true, body: 'review the auth flow' }]
    const live: AgentMsg[] = [{ me: true, body: 'review the auth flow', optimistic: true }]
    expect(retainOptimisticUsers(mapped, live)).toEqual(mapped)
  })

  test('drops optimistic caption when persist kept the attachment marker', () => {
    const marked = `${attachmentMarker('img_a')}review the auth flow`
    const mapped: AgentMsg[] = [{ me: true, body: marked }]
    const live: AgentMsg[] = [{ me: true, body: 'review the auth flow', optimistic: true }]
    expect(retainOptimisticUsers(mapped, live)).toEqual(mapped)
  })

  test('idle overwrite does not append the optimistic user after the turn footer', () => {
    const marked = `${attachmentMarker('img_a')}review the auth flow`
    const mapped: AgentMsg[] = [
      { me: true, body: marked },
      { body: 'done' },
      { turnEnd: true, durationMs: 1 },
    ]
    const live: AgentMsg[] = [
      { me: true, body: 'review the auth flow', optimistic: true },
      { me: true, body: marked },
      { body: 'done' },
    ]
    expect(retainOptimisticUsers(mapped, live)).toEqual(mapped)
  })

  test('does not keep live stream rows on an idle overwrite', () => {
    const live: AgentMsg[] = [
      { me: true, body: 'review the auth flow', optimistic: true },
      { body: 'working…', streaming: true },
    ]
    expect(retainOptimisticUsers([], live)).toEqual([
      { me: true, body: 'review the auth flow', optimistic: true },
    ])
  })

  test('keeps a repeated steer separated only by tools', () => {
    const mapped: AgentMsg[] = [
      { me: true, body: 'please stop' },
      { tool: 'Read', result: 'ok' },
      { me: true, body: 'please stop' },
      { turnEnd: true, durationMs: 1 },
    ]
    expect(retainOptimisticUsers(mapped, [])).toEqual(mapped)
  })

  test('does not drop a new image send that shares a caption with older text', () => {
    const marked = `${attachmentMarker('new_image')}review this`
    const mapped: AgentMsg[] = [
      { me: true, body: 'review this' },
      { body: 'done' },
      { turnEnd: true, durationMs: 1 },
    ]
    const live: AgentMsg[] = [
      {
        me: true,
        body: marked,
        optimistic: true,
        attachments: [{ id: 'new_image', name: 'shot.png', mime: 'image/png', dataUrl: 'data:image/png;base64,xx' }],
      },
    ]
    expect(retainOptimisticUsers(mapped, live)).toEqual([...mapped, live[0]])
  })

  test('keeps a settled image send that shares a caption with an earlier user', () => {
    const marked = `${attachmentMarker('img_a')}review this`
    const mapped: AgentMsg[] = [
      { me: true, body: 'review this' },
      { tool: 'Read', result: 'ok' },
      { me: true, body: marked },
      { turnEnd: true, durationMs: 1 },
    ]
    expect(retainOptimisticUsers(mapped, [])).toEqual(mapped)
  })

  test('keeps a new plain-text send after a completed image turn with the same caption', () => {
    const marked = `${attachmentMarker('img_a')}review this`
    const mapped: AgentMsg[] = [
      { me: true, body: marked },
      { body: 'done' },
      { turnEnd: true, durationMs: 1 },
    ]
    const live: AgentMsg[] = [{ me: true, body: 'review this', optimistic: true }]
    expect(retainOptimisticUsers(mapped, live)).toEqual([...mapped, live[0]])
  })

  test('keeps a new plain-text send when live still holds the completed image history', () => {
    const marked = `${attachmentMarker('img_a')}review this`
    const mapped: AgentMsg[] = [
      { me: true, body: marked },
      { body: 'done' },
      { turnEnd: true, durationMs: 1 },
    ]
    const live: AgentMsg[] = [...mapped, { me: true, body: 'review this', optimistic: true }]
    expect(retainOptimisticUsers(mapped, live)).toEqual([...mapped, live[live.length - 1]])
  })

  test('keeps a new plain-text send after an open image turn with the same caption', () => {
    const marked = `${attachmentMarker('img_a')}review this`
    const mapped: AgentMsg[] = [
      { me: true, body: marked },
      { tool: 'Read', result: 'ok' },
    ]
    const live: AgentMsg[] = [
      ...mapped,
      { me: true, body: 'review this', optimistic: true, pendingId: 'new-steer' },
    ]
    expect(retainOptimisticUsers(mapped, live)).toEqual(live)
    expect(mergePersistedWithLive(mapped, live)).toEqual(live)
  })
})

describe('optionChoiceText', () => {
  test('strips HTML tags from seeded escalation options', () => {
    expect(optionChoiceText('Reject with <code style="font-family:var(--mono)">429</code>')).toBe(
      'Reject with 429',
    )
  })

  test('leaves comparison prose that a naive <[^>]+> strip would eat', () => {
    expect(optionChoiceText('count < 5 or when rate > 2/s')).toBe('count < 5 or when rate > 2/s')
    expect(optionChoiceText('Cap at < 100 req/min')).toBe('Cap at < 100 req/min')
  })

  test('leaves already-plain option labels alone', () => {
    expect(optionChoiceText('Token bucket with refill')).toBe('Token bucket with refill')
  })
})

describe('formatAskUserDisplayText', () => {
  test('mirrors Go formatAskUserDisplay', () => {
    expect(formatAskUserDisplayText('Blue', '')).toBe('Blue')
    expect(formatAskUserDisplayText('', 'navy preferred')).toBe('navy preferred')
    expect(formatAskUserDisplayText('Blue', 'navy preferred')).toBe('Blue — navy preferred')
    expect(formatAskUserDisplayText('  ', '  ')).toBe('(empty)')
  })
})

describe('pendingSendBubbleBody', () => {
  test('keeps attachment markers when display_text is empty', () => {
    const marked = `${attachmentMarker('img_a')}pr settings alignment`
    expect(pendingSendBubbleBody('', marked)).toBe(marked)
    expect(pendingSendBubbleBody(undefined, marked)).toBe(marked)
  })

  test('prefers a prompt-action display_text label', () => {
    expect(pendingSendBubbleBody('Fix open reviewer findings', 'WIRE do not show')).toBe(
      'Fix open reviewer findings',
    )
  })
})

describe('sameUserChatBody', () => {
  test('treats a plain caption as the marked wire of the same send', () => {
    const marked = `${attachmentMarker('img_a')}pr settings alignment`
    expect(sameUserChatBody('pr settings alignment', marked)).toBe(true)
  })

  test('keeps two same-caption sends with different images distinct', () => {
    expect(
      sameUserChatBody(
        `${attachmentMarker('img_a')}pr settings alignment`,
        `${attachmentMarker('img_b')}pr settings alignment`,
      ),
    ).toBe(false)
  })
})

describe('displayUserChatText ask_user resume', () => {
  test('collapses full resume wire to the Answer line', () => {
    const wire = [
      'The user answered your earlier ask_user question.',
      '',
      'Question: Which color?',
      'Context: pick one',
      'Answer: Blue',
      '',
      'Continue with this answer. Do not re-ask the same question unless you still need clarification.',
    ].join('\n')
    expect(displayUserChatText(wire)).toBe('Blue')
  })

  test('includes Details when present', () => {
    const wire = [
      'The user answered your earlier ask_user question.',
      '',
      'Question: Which color?',
      'Answer: Blue',
      'Details: navy preferred',
      '',
      'Continue with this answer. Do not re-ask the same question unless you still need clarification.',
    ].join('\n')
    expect(displayUserChatText(wire)).toBe('Blue — navy preferred')
  })
})

describe('displayUserChatText coder dispatch', () => {
  test('collapses the findings-fix wire to the prompt-action label', () => {
    const wire = [
      'You are the Coder agent. A reviewer found issues with the current diff. ',
      'Fix every finding below with minimal, correct edits. For each finding, call ',
      'mcp__recur__update_finding with its id: status resolved if you fixed it, or ',
      'invalid with a clear reason if it is wrong/out of scope. Then call mcp__recur__done.',
      '',
      'Original task: create a cli game of pong',
      '',
      'Reviewer findings to fix:',
      '1. id=F-1 [medium] pong/game.py:135 — paddle centering mismatch',
    ].join('\n')
    expect(displayUserChatText(wire)).toBe('Fix open reviewer findings')
  })

  test('leaves a bare task string alone', () => {
    expect(displayUserChatText('create a cli game of pong')).toBe('create a cli game of pong')
  })
})

describe('formatFileMentionChip', () => {
  test('formats plain, line, and span chips', () => {
    expect(formatFileMentionChip('src/a.ts')).toBe('src/a.ts')
    expect(formatFileMentionChip('src/a.ts', 12)).toBe('src/a.ts:12')
    expect(formatFileMentionChip('src/a.ts', 12, 40)).toBe('src/a.ts:12-40')
    expect(formatFileMentionChip('src/a.ts', 12, undefined, 5, 18)).toBe('src/a.ts:12:5-12:18')
    expect(formatFileMentionChip('src/a.ts', 12, 13, 5, 20)).toBe('src/a.ts:12:5-13:20')
    expect(formatFileMentionChip('src/a.ts', 12, 12, 5, 18)).toBe('src/a.ts:12:5-12:18')
    expect(formatFileMentionChip('src/a.ts', 12, undefined, 5, 5)).toBe('src/a.ts:12:5-12:5')
  })

  test('ignores non-positive or missing lines', () => {
    expect(formatFileMentionChip('src/a.ts', 0)).toBe('src/a.ts')
    expect(formatFileMentionChip('src/a.ts', -1, 10)).toBe('src/a.ts')
    expect(formatFileMentionChip('src/a.ts', 12, 12)).toBe('src/a.ts:12')
    expect(formatFileMentionChip('src/a.ts', 12, 5)).toBe('src/a.ts:12')
  })

  test('trims path and rejects empty', () => {
    expect(formatFileMentionChip('  src/a.ts  ', 3)).toBe('src/a.ts:3')
    expect(formatFileMentionChip('')).toBe('')
    expect(formatFileMentionChip('   ')).toBe('')
  })
})

describe('parseFileMentionChip', () => {
  test('parses plain, line, and span chips', () => {
    expect(parseFileMentionChip('src/a.ts')).toEqual({ path: 'src/a.ts' })
    expect(parseFileMentionChip('src/a.ts:12')).toEqual({ path: 'src/a.ts', startLine: 12 })
    expect(parseFileMentionChip('src/a.ts:12-40')).toEqual({
      path: 'src/a.ts',
      startLine: 12,
      endLine: 40,
    })
  })

  test('parses same-line and cross-line column spans', () => {
    expect(parseFileMentionChip('src/a.ts:12:5-12:18')).toEqual({
      path: 'src/a.ts',
      startLine: 12,
      startCol: 5,
      endCol: 18,
    })
    expect(parseFileMentionChip('src/a.ts:12:5-13:20')).toEqual({
      path: 'src/a.ts',
      startLine: 12,
      endLine: 13,
      startCol: 5,
      endCol: 20,
    })
    expect(parseFileMentionChip('src/a.ts:12:5-12:5')).toEqual({
      path: 'src/a.ts',
      startLine: 12,
      startCol: 5,
      endCol: 5,
    })
  })

  test('12:5-18 is line 12 col 5 through line 18, not same-line columns', () => {
    expect(parseFileMentionChip('src/a.ts:12:5-18')).toEqual({
      path: 'src/a.ts',
      startLine: 12,
      endLine: 18,
    })
  })

  test('invalid :0 suffix falls back to whole-file path', () => {
    expect(parseFileMentionChip('src/a.ts:0')).toEqual({ path: 'src/a.ts' })
    expect(parseFileMentionChip('src/a.ts:0-12')).toEqual({ path: 'src/a.ts' })
    expect(parseFileMentionChip('src/a.ts:0:5-12:18')).toEqual({ path: 'src/a.ts' })
  })

  test('end <= start drops the end line', () => {
    expect(parseFileMentionChip('src/a.ts:12-12')).toEqual({ path: 'src/a.ts', startLine: 12 })
    expect(parseFileMentionChip('src/a.ts:12-5')).toEqual({ path: 'src/a.ts', startLine: 12 })
  })

  test('invalid columns keep the line and drop the column span', () => {
    expect(parseFileMentionChip('src/a.ts:12:0-12:18')).toEqual({ path: 'src/a.ts', startLine: 12 })
    expect(parseFileMentionChip('src/a.ts:12:8-12:3')).toEqual({ path: 'src/a.ts', startLine: 12 })
  })
})

describe('fileMentionLabel', () => {
  test('uses basename with optional line span', () => {
    expect(fileMentionLabel('src/a.ts')).toBe('a.ts')
    expect(fileMentionLabel('src/a.ts:12')).toBe('a.ts (12)')
    expect(fileMentionLabel('src/a.ts:12-40')).toBe('a.ts (12-40)')
    expect(fileMentionLabel('src/a.ts:12:5-12:18')).toBe('a.ts (12:5-12:18)')
    expect(fileMentionLabel('src/a.ts:12:5-13:20')).toBe('a.ts (12:5-13:20)')
    expect(fileMentionLabel('PROGRESS.md')).toBe('PROGRESS.md')
  })
})

describe('fileMentionToken / removeFileMentionToken', () => {
  test('fileMentionToken prefixes @', () => {
    expect(fileMentionToken('src/a.ts:12')).toBe('@src/a.ts:12')
  })

  test('removeFileMentionToken eats one adjacent space', () => {
    expect(removeFileMentionToken('how does @src/a.ts compare', 'src/a.ts')).toBe(
      'how does compare',
    )
    expect(removeFileMentionToken('see @src/a.ts', 'src/a.ts')).toBe('see')
    expect(removeFileMentionToken('@src/a.ts please', 'src/a.ts')).toBe('please')
  })

  test('removeFileMentionToken handles line-span chips', () => {
    expect(removeFileMentionToken('look at @src/a.ts:12-40 please', 'src/a.ts:12-40')).toBe(
      'look at please',
    )
  })
})

describe('mentionQueryAt / skillQueryAt', () => {
  test('mentionQueryAt matches @query after whitespace', () => {
    expect(mentionQueryAt('see @quer', 9)).toEqual({ start: 4, end: 9, query: 'quer' })
    expect(mentionQueryAt('@src', 4)).toEqual({ start: 0, end: 4, query: 'src' })
  })

  test('mentionQueryAt rejects mid-token @ (email)', () => {
    expect(mentionQueryAt('john@acme', 9)).toBeNull()
  })

  test('skillQueryAt matches /name after whitespace', () => {
    expect(skillQueryAt('run /foo', 8)).toEqual({ start: 4, end: 8, query: 'foo' })
    expect(skillQueryAt('/bar', 4)).toEqual({ start: 0, end: 4, query: 'bar' })
  })

  test('skillQueryAt rejects absolute paths', () => {
    expect(skillQueryAt('open /Users/x', 12)).toBeNull()
  })
})

describe('stripFileMentions', () => {
  test('strips plain and line-span chips', () => {
    expect(stripFileMentions('see @src/a.ts please').trim()).toBe('see  please')
    expect(stripFileMentions('see @src/a.ts:12-40 please').trim()).toBe('see  please')
  })
})

describe('repairFileMentionBoundaries', () => {
  test('inserts a space before slashy @paths that lost their boundary', () => {
    expect(repairFileMentionBoundaries('what does it do?@frontend/src/a.ts:1-2')).toBe(
      'what does it do? @frontend/src/a.ts:1-2',
    )
    expect(repairFileMentionBoundaries('look@src/a.ts:12:5-12:18')).toBe(
      'look @src/a.ts:12:5-12:18',
    )
    expect(repairFileMentionBoundaries('@src/a.ts:1@src/b.ts:2')).toBe(
      '@src/a.ts:1 @src/b.ts:2',
    )
    expect(repairFileMentionBoundaries('look@.github/')).toBe('look @.github/')
    expect(repairFileMentionBoundaries('see@foo.bar/')).toBe('see @foo.bar/')
  })

  test('leaves emails and already-bounded mentions alone', () => {
    expect(repairFileMentionBoundaries('email john@acme.io please')).toBe(
      'email john@acme.io please',
    )
    expect(repairFileMentionBoundaries('see @src/a.ts please')).toBe('see @src/a.ts please')
  })
})

describe('prepareFileMentionInsert', () => {
  test('prepends a space when the caret follows non-whitespace', () => {
    const { before, after } = prepareFileMentionInsert('explain this', 12)
    expect(before).toBe('explain this ')
    expect(after).toBe('')
  })

  test('does not double-space after existing whitespace', () => {
    const { before, after } = prepareFileMentionInsert('explain this ', 13)
    expect(before).toBe('explain this ')
    expect(after).toBe('')
  })

  test('empty composer needs no leading space', () => {
    expect(prepareFileMentionInsert('', 0)).toEqual({ before: '', after: '' })
  })

  test('strips an in-progress @query before inserting', () => {
    expect(prepareFileMentionInsert('see @quer', 9)).toEqual({ before: 'see ', after: '' })
  })

  test('strips an in-progress /skill query before inserting', () => {
    expect(prepareFileMentionInsert('run /foo', 8)).toEqual({ before: 'run ', after: '' })
  })

  test('insert mid-text keeps the tail and adds a boundary', () => {
    expect(prepareFileMentionInsert('hello world', 5)).toEqual({
      before: 'hello ',
      after: ' world',
    })
  })

  test('stripQuery:false keeps a committed chip wire form at the caret', () => {
    const text = 'see @src/a.ts'
    const caret = text.length
    expect(prepareFileMentionInsert(text, caret)).toEqual({ before: 'see ', after: '' })
    expect(prepareFileMentionInsert(text, caret, { stripQuery: false })).toEqual({
      before: 'see @src/a.ts ',
      after: '',
    })
  })
})

describe('extractFileMentions / FILE_AT_RE positions', () => {
  const variants = [
    { chip: 'src/a.ts', label: 'plain' },
    { chip: 'src/a.ts:12', label: 'line' },
    { chip: 'src/a.ts:12-40', label: 'span' },
    { chip: 'src/a.ts:12:5-12:18', label: 'cols' },
    { chip: 'src/a.ts:12:5-13:20', label: 'cross-cols' },
    { chip: 'PROGRESS.md', label: 'name.ext' },
    { chip: '.gitignore', label: 'dotfile' },
  ]

  for (const { chip, label } of variants) {
    test(`${label} chip as sole prompt`, () => {
      expect(extractFileMentions(`@${chip}`)).toEqual([chip])
      expect(formatFileMentionRef(parseFileMentionChip(chip))).toBe(chip)
    })

    test(`${label} chip at start of prompt`, () => {
      expect(extractFileMentions(`@${chip} please review`)).toEqual([chip])
    })

    test(`${label} chip at end of prompt`, () => {
      expect(extractFileMentions(`please review @${chip}`)).toEqual([chip])
    })
  }

  test('missing leading boundary is not a mention', () => {
    expect(extractFileMentions('explain this@src/a.ts')).toEqual([])
    expect(extractFileMentions('this@src/a.ts:12-40')).toEqual([])
    const { before, after } = prepareFileMentionInsert('explain this', 12)
    const wire = `${before}@src/a.ts${after.startsWith(' ') || after.startsWith('\n') ? '' : ' '}${after}`
    expect(extractFileMentions(wire)).toEqual(['src/a.ts'])
    expect(new RegExp(FILE_AT_RE.source, 'g').test(wire)).toBe(true)
  })

  test('trailing-slash directories including reserved names', () => {
    expect(extractFileMentions('@src')).toEqual([])
    expect(extractFileMentions('@src/')).toEqual(['src/'])
    expect(extractFileMentions('look at @src/ please')).toEqual(['src/'])
    expect(extractFileMentions('@src/lib/')).toEqual(['src/lib/'])
    expect(extractFileMentions('@terminal/')).toEqual(['terminal/'])
    expect(extractFileMentions('@.github/')).toEqual(['.github/'])
    expect(extractFileMentions('@foo.bar/')).toEqual(['foo.bar/'])
    expect(extractFileMentions('look at @.github/ please')).toEqual(['.github/'])
    expect(extractTerminalMentions('@terminal/')).toEqual([])
    expect(dirFileMentionPath('src')).toBe('src/')
    expect(dirFileMentionPath('src/lib/')).toBe('src/lib/')
    expect(isDirFileMention('src/')).toBe(true)
    expect(isDirFileMention('src/lib')).toBe(false)
  })
})

describe('@terminal mentions', () => {
  test('extracts bare and qualified chips', () => {
    expect(extractTerminalMentions('see @terminal and @terminal:abc-def')).toEqual([
      'terminal',
      'terminal:abc-def',
    ])
    expect(terminalMentionToken('abc-def')).toBe('@terminal:abc-def')
    expect(terminalMentionToken('')).toBe('@terminal')
    expect(terminalMentionLabel('terminal:abc', 'dev')).toBe('dev')
    expect(terminalMentionLabel('terminal')).toBe('terminal')
  })

  test('does not treat emails as terminals', () => {
    expect(extractTerminalMentions('write foo@terminal.com')).toEqual([])
  })

  test('does not match @terminals as a prefix', () => {
    expect(extractTerminalMentions('see @terminals please')).toEqual([])
  })

  test('stripTerminalMentions leaves the prose', () => {
    expect(stripComposerMarkers('errors in @terminal:abc please')).toBe('errors in please')
  })
})

describe('@artifact mentions', () => {
  test('extracts bare and qualified chips', () => {
    expect(extractArtifactMentions('see @artifact and @artifact:abc-def')).toEqual([
      'artifact',
      'artifact:abc-def',
    ])
    expect(artifactMentionToken('abc-def')).toBe('@artifact:abc-def')
    expect(artifactMentionToken('')).toBe('@artifact')
    expect(artifactMentionLabel('artifact:abc', 'Notes')).toBe('Notes')
    expect(artifactMentionLabel('artifact')).toBe('artifact')
  })

  test('does not treat emails as artifacts', () => {
    expect(extractArtifactMentions('write foo@artifact.com')).toEqual([])
  })

  test('does not match @artifacts as a prefix', () => {
    expect(extractArtifactMentions('see @artifacts please')).toEqual([])
  })

  test('stripArtifactMentions leaves the prose', () => {
    expect(stripComposerMarkers('review @artifact:abc please')).toBe('review please')
  })

  test('uniqueArtifactIdForTitle matches the panel name', () => {
    const arts = [
      { id: 'art-1', title: 'Notes' },
      { id: 'art-2', title: 'Chart' },
    ]
    expect(uniqueArtifactIdForTitle('Notes', arts)).toBe('art-1')
    expect(uniqueArtifactIdForTitle('notes', arts)).toBe('art-1')
    expect(uniqueArtifactIdForTitle('missing', arts)).toBe('')
    expect(reservedAtName('artifact')).toBe(true)
    expect(uniqueArtifactIdForTitle('app.ts', [{ id: 'x', title: 'app.ts' }])).toBe('')
    expect(atNameIsFileMention('app.ts')).toBe(true)
    expect(atNameIsFileMention('notes.md:v2')).toBe(false)
    expect(uniqueArtifactIdForTitle('notes.md:v2', [{ id: 'x', title: 'notes.md:v2' }])).toBe('x')
  })
})

describe('agent error replies', () => {
  const spend = "You've hit your individual spend limit · run /usage-credits to ask your admin for a higher limit"
  const prefixed = `${AGENT_ERROR_PREFIX} ${spend}`

  test('classifies usage and rate-limit titles', () => {
    expect(agentErrorTitle(prefixed)).toBe('Usage limit')
    expect(agentErrorTitle('Agent error: rate limit exceeded')).toBe('Rate limit')
    expect(agentErrorTitle('Agent error: cursor run: boom')).toBe('Agent error')
    expect(agentErrorDetail(prefixed)).toBe(spend)
  })

  test('maps Codex refresh-token dumps to a login sentence', () => {
    const ansi =
      '\x1b[2m2026-09-10T15:43:08.630243Z\x1b[0m \x1b[31mERROR\x1b[0m \x1b[2mcodex_login::auth::manager\x1b[0m \x1b[2m-\x1b[0m Failed to refresh token: 401 Unauthorized {'
    const body = `${AGENT_ERROR_PREFIX} ${ansi}`
    expect(agentErrorDetail(body)).toBe(
      'Codex login expired: sign in with ChatGPT in Settings → Providers → Codex, then retry.',
    )
    expect(agentErrorTitle(body)).toBe('Codex login')
  })

  test('tidies tracing dumps so the message is not cut off at a brace', () => {
    expect(agentErrorDetail('Agent error: ERROR workspace::tools - workspace tool failed to start {')).toBe(
      'workspace tool failed to start',
    )
  })

  test('keeps JSON in ordinary provider errors', () => {
    const msg = 'API Error: 400 {"error":{"message":"model does not support images"}}'
    expect(agentErrorDetail(`Agent error: ${msg}`)).toBe(msg)
  })

  test('maps Codex 401 dumps without refresh-token prose', () => {
    const body = `${AGENT_ERROR_PREFIX} ERROR codex_login::auth::manager - 401 Unauthorized`
    expect(agentErrorDetail(body)).toBe(
      'Codex login expired: sign in with ChatGPT in Settings → Providers → Codex, then retry.',
    )
  })

  test('isAgentErrorReply covers prefix and generic fallbacks', () => {
    expect(isAgentErrorReply(prefixed)).toBe(true)
    expect(isAgentErrorReply('The agent hit an error — try again.')).toBe(true)
    expect(isAgentErrorReply('I hit an error trying to respond just now. Try again.')).toBe(true)
    expect(isAgentErrorReply('Done.')).toBe(false)
  })

  test('finishTurn marks the note as an error card and stamps duration', () => {
    const out = finishTurn([{ me: true, body: 'do the thing' }], prefixed, false, 62000)
    expect(out.some((m) => m.error && m.body === prefixed)).toBe(true)
    expect(out.some((m) => m.turnEnd && m.durationMs === 62000)).toBe(true)
  })

  test('finishTurn collapses provider-echoed error text into one card', () => {
    const out = finishTurn(
      [
        { me: true, body: 'do the thing' },
        { body: spend },
        { turnEnd: true, durationMs: 62000 },
      ],
      prefixed,
      true,
      0,
    )
    const errors = out.filter((m) => m.error)
    expect(errors).toHaveLength(1)
    expect(errors[0].body).toBe(prefixed)
    expect(out.filter((m) => m.body === spend && !m.error)).toHaveLength(0)
    expect(out.some((m) => m.turnEnd)).toBe(true)
  })

  test('endTurn settles in-flight rows before appending the footer', () => {
    const out = endTurn(
      [
        { me: true, body: 'go' },
        { body: 'partial', streaming: true },
        { tool: 'Read', resultPending: true },
      ],
      103000,
    )
    expect(out.some((m) => m.streaming || m.resultPending)).toBe(false)
    expect(out[out.length - 1]).toMatchObject({ turnEnd: true, durationMs: 103000 })
    expect(out[out.length - 1].ts).toBeGreaterThan(0)
    expect(endTurn(out, 103000)).toBe(out)
  })

  test('currentTurnHasEnded is true only while the last transcript row is the footer', () => {
    expect(currentTurnHasEnded([])).toBe(false)
    expect(currentTurnHasEnded([{ me: true, body: 'go' }])).toBe(false)
    expect(currentTurnHasEnded([{ me: true, body: 'go' }, { body: 'ok' }])).toBe(false)
    const footerAt = 1_700_000_000_000
    const done: AgentMsg[] = [
      { me: true, body: 'go' },
      { body: 'ok' },
      { turnEnd: true, durationMs: 103000, ts: footerAt },
    ]
    expect(currentTurnHasEnded(done)).toBe(true)
    // Same turn still running after the footer (Send / agentsRunning lag).
    expect(currentTurnHasEnded(done, footerAt - 50_000)).toBe(true)
    expect(currentTurnHasEnded([
      ...done,
      { log: { kind: 'session', code: 'CO', name: 'Coder', messages: [], live: true } },
    ])).toBe(true)
    expect(currentTurnHasEnded([
      ...done,
      { me: true, body: 'queued next', queued: true },
    ])).toBe(true)
    // Retry / autonomous MA stamp turnStartedAt after the footer, no `me` row.
    expect(currentTurnHasEnded(done, footerAt)).toBe(false)
    expect(currentTurnHasEnded(done, footerAt + 10)).toBe(false)
    expect(currentTurnHasEnded([...done, { body: 'retried reply', streaming: true }])).toBe(false)
    expect(currentTurnHasEnded([...done, { tool: 'Read', resultPending: true }])).toBe(false)
    expect(currentTurnHasEnded([...done, { note: true, body: 'Waiting for previous run run-1 to finish.' }])).toBe(false)
    expect(currentTurnHasEnded([
      ...done,
      { me: true, body: 'next turn', optimistic: true },
    ])).toBe(false)
    // Persisted footer with no ts: a positive stamp is still a new turn.
    const noTs: AgentMsg[] = [{ me: true, body: 'go' }, { turnEnd: true, durationMs: 1 }]
    expect(currentTurnHasEnded(noTs)).toBe(true)
    expect(currentTurnHasEnded(noTs, 10)).toBe(false)
  })

  test('dropLiveAfterFooter keeps leftover out but lets a new busy turn through', () => {
    const footerAt = 1_700_000_000_000
    const done: AgentMsg[] = [
      { me: true, body: 'go' },
      { body: 'ok' },
      { turnEnd: true, durationMs: 103000, ts: footerAt },
    ]
    const apply = (list: AgentMsg[], started: number, busy: boolean) =>
      dropLiveAfterFooter(list, started, busy) ? list : applyStream(list, 'text', 'late leftover chunk')
    // Same turn, stamp older than the footer (Send / agentsRunning lag).
    expect(apply(done, footerAt - 50_000, true)).toBe(done)
    // Late leftover after idle — engine is dark.
    expect(apply(done, 0, false)).toBe(done)
    // Autonomous / reload: stamp cleared, engine busy again.
    const next = apply(done, 0, true)
    expect(next).not.toBe(done)
    expect(next[next.length - 1]).toMatchObject({ body: 'late leftover chunk', streaming: true })
    // Retry stamped after the footer.
    const retried = apply(done, footerAt + 10, true)
    expect(retried[retried.length - 1]).toMatchObject({ body: 'late leftover chunk', streaming: true })
    // ts=0 footer + stamp (reload / mapped turn item) must not drop the next turn.
    const noTs: AgentMsg[] = [{ me: true, body: 'go' }, { turnEnd: true, durationMs: 1 }]
    const reopened = apply(noTs, 10, false)
    expect(reopened).not.toBe(noTs)
  })

  test('shouldRefreshTurnStamp replaces a stamp that still belongs to a closed footer', () => {
    const footerAt = 1_700_000_000_000
    const done: AgentMsg[] = [
      { me: true, body: 'go' },
      { body: 'ok' },
      { turnEnd: true, durationMs: 103000, ts: footerAt },
    ]
    expect(shouldRefreshTurnStamp(done, 0)).toBe(true)
    expect(shouldRefreshTurnStamp(done, footerAt - 50_000)).toBe(true)
    expect(shouldRefreshTurnStamp(done, footerAt + 10)).toBe(false)
    expect(shouldRefreshTurnStamp([{ me: true, body: 'go' }], 1)).toBe(false)
  })

  test('shouldIgnoreStaleIdle is true only when a newer stamp already sits on the footer', () => {
    const footerAt = 1_700_000_000_000
    const done: AgentMsg[] = [
      { me: true, body: 'go' },
      { turnEnd: true, durationMs: 1, ts: footerAt },
    ]
    expect(shouldIgnoreStaleIdle(done, 0)).toBe(false)
    expect(shouldIgnoreStaleIdle(done, footerAt - 1)).toBe(false)
    expect(shouldIgnoreStaleIdle(done, footerAt + 10)).toBe(true)
    expect(shouldIgnoreStaleIdle(done, footerAt)).toBe(true)
    expect(shouldIgnoreStaleIdle([{ me: true, body: 'go' }], footerAt + 10)).toBe(false)
    expect(shouldIgnoreStaleIdle(
      [{ me: true, body: 'go' }, { turnEnd: true, durationMs: 1 }],
      10,
    )).toBe(false)
  })

  test('shouldSkipIdleTranscriptReset keeps the first reordered idle from wiping a live turn', () => {
    const footerAt = 1_700_000_000_000
    const done: AgentMsg[] = [
      { me: true, body: 'go' },
      { turnEnd: true, durationMs: 1, ts: footerAt },
    ]
    expect(shouldSkipIdleTranscriptReset(done, footerAt - 1, true)).toBe(false)
    expect(shouldSkipIdleTranscriptReset(done, footerAt + 10, true)).toBe(true)
    expect(shouldSkipIdleTranscriptReset(done, footerAt + 10, false)).toBe(false)
    const live = [...done, { body: 'second turn', streaming: true }]
    expect(shouldSkipIdleTranscriptReset(live, footerAt + 10, true)).toBe(true)
    expect(shouldSkipIdleTranscriptReset(live, footerAt + 10, false)).toBe(false)
    // finishTurn / RPC reply with no session.turn_end footer must settle.
    expect(shouldSkipIdleTranscriptReset(
      [{ me: true, body: 'go' }, { body: 'reply' }],
      10,
      false,
    )).toBe(false)
    expect(shouldSkipIdleTranscriptReset([], 10, true)).toBe(false)
  })

  test('lastAssistantTextInTurn returns the last assistant body before the footer', () => {
    const msgs = [
      { me: true, body: 'do it' },
      { body: 'first' },
      { tool: 'Read', input: '{}' },
      { body: 'final reply' },
      { turnEnd: true, durationMs: 12 },
    ]
    const got = lastAssistantTextInTurn(msgs, 4)
    expect(got?.body).toBe('final reply')
    expect(lastAssistantTextInTurn(msgs, 0)).toBeNull()
    expect(lastAssistantTextInTurn([{ turnEnd: true }], 0)).toBeNull()
  })

  test('stillOpenFindingsForTurn collects Codex mcp__recur__report_finding rows', () => {
    const filed = {
      id: 'f-1',
      file: 'store.go',
      line: 1276,
      endLine: 1286,
      severity: 'high',
      message: 'activity hook can miss a turn',
      summary: 'Hook misses turn',
      status: 'open',
    }
    const msgs = [
      { me: true, body: 'review this' },
      {
        tool: 'mcp__recur__report_finding',
        input: JSON.stringify({
          file: filed.file,
          line: filed.line,
          end_line: filed.endLine,
          severity: filed.severity,
          message: filed.message,
          summary: filed.summary,
        }),
        result: JSON.stringify(filed),
      },
      { body: 'Filed 1 finding.' },
      { turnEnd: true, durationMs: 12000 },
    ]
    const digest = stillOpenFindingsForTurn(msgs, 3, [filed])
    expect(digest).toEqual([
      {
        id: 'f-1',
        file: 'store.go',
        line: 1276,
        endLine: 1286,
        severity: 'high',
        message: filed.message,
        text: filed.summary,
        notes: [],
      },
    ])
  })

  test('stillOpenFindingsForTurn closes a short-id filing when update_finding uses the UUID', () => {
    const uuid = 'aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee'
    const shortId = 'find_k7mq2xf9'
    const filed = {
      id: uuid,
      shortId,
      file: 'store.go',
      line: 10,
      message: 'nil deref',
      status: 'open',
    }
    const msgs = [
      { me: true, body: 'review this' },
      {
        tool: 'mcp__recur__report_finding',
        result: JSON.stringify({
          id: uuid,
          file: filed.file,
          line: filed.line,
          message: filed.message,
          status: 'open',
        }),
      },
      {
        tool: 'mcp__recur__update_finding',
        input: JSON.stringify({ finding_id: uuid, status: 'resolved' }),
        result: '{}',
      },
      { turnEnd: true },
    ]
    expect(stillOpenFindingsForTurn(msgs, 3, [filed])).toEqual([])
  })

  test('mapChatItems keeps a Codex turn footer so the findings digest can render', () => {
    const mapped = mapChatItems([
      { role: 'user', kind: 'text', text: 'review this' },
      { role: 'assistant', kind: 'tool', tool: 'mcp__recur__report_finding', input: '{}' },
      { role: 'assistant', kind: 'text', text: 'Filed 1 finding.' },
      { role: 'assistant', kind: 'turn', durationMs: 8400 },
    ])
    expect(mapped.find((m) => m.turnEnd)?.durationMs).toBe(8400)
  })

  test('mapChatItems has no turnEnd when durationMs was never stamped', () => {
    const mapped = mapChatItems([
      { role: 'user', kind: 'text', text: 'review this' },
      { role: 'assistant', kind: 'tool', tool: 'mcp__recur__report_finding', input: '{}' },
      { role: 'assistant', kind: 'text', text: 'Filed 1 finding.' },
    ])
    expect(mapped.some((m) => m.turnEnd)).toBe(false)
  })

  test('mapChatItems marks errors, skips retry sentinels, and collapses dupes', () => {
    const mapped = mapChatItems([
      { role: 'user', kind: 'text', text: 'do the thing' },
      { role: 'assistant', kind: 'text', text: spend },
      { role: 'assistant', kind: 'turn', durationMs: 62000, messageId: 'msg-turn-1' },
      { role: 'assistant', kind: 'text', text: prefixed },
      { role: 'user', kind: 'text', text: CHAT_RETRY_DISPLAY },
      { role: 'user', kind: 'text', text: 'Switched to Plan: continuing' },
    ])
    expect(mapped.find((m) => m.turnEnd)?.messageId).toBe('msg-turn-1')
    expect(mapped.filter((m) => m.me)).toHaveLength(1)
    expect(mapped.filter((m) => m.error)).toHaveLength(1)
    expect(mapped.find((m) => m.error)?.body).toBe(prefixed)
  })

  test('mapChatItems keeps ts on repeated errors so transcript keys stay unique', () => {
    const err = 'Agent error: [resource_exhausted] Error. Cursor is rate-limited.'
    const mapped = mapChatItems([
      { role: 'user', kind: 'text', text: 'go', ts: 1 },
      { role: 'assistant', kind: 'text', text: err, ts: 10 },
      { role: 'assistant', kind: 'thinking', text: 'retrying', ts: 20 },
      { role: 'assistant', kind: 'text', text: err, ts: 30 },
    ])
    const errs = mapped.filter((m) => m.error)
    expect(errs).toHaveLength(2)
    expect(new Set(errs.map((m) => transcriptMessageBlockLocal(0, m))).size).toBe(2)
  })

  test('mapChatItems collapses coder fix-turn wire on user rows', () => {
    const wire = 'You are the Coder agent. A reviewer found issues with the current diff.\nFix every finding.'
    const mapped = mapChatItems([{ role: 'user', kind: 'text', text: wire }])
    expect(mapped[0]).toMatchObject({ me: true, body: 'Fix open reviewer findings' })
  })

  test('mapChatItems and applyCompact render compaction rows', () => {
    const mapped = mapChatItems([
      { role: 'assistant', kind: 'compact', text: 'older turns folded', tokens: 167219 },
      { role: 'assistant', kind: 'text', text: 'continuing' },
    ])
    expect(mapped[0]).toMatchObject({ compact: true, body: 'older turns folded', tokens: 167219 })
    const live = applyCompact([], { status: 'started', preTokens: 180000 })
    expect(live[0]).toMatchObject({ compact: true, compacting: true, tokens: 180000 })
    const done = applyCompact(live, { status: 'done', summary: 'folded' })
    expect(done).toHaveLength(1)
    expect(done[0]).toMatchObject({ compact: true, compacting: false, body: 'folded' })
  })

  test('applyStream done-only text does not overwrite a compact row', () => {
    const compact = applyCompact([], { status: 'done', summary: 'older turns folded' })
    const next = applyStream(compact, 'text', 'continuing', { done: true })
    expect(next).toHaveLength(2)
    expect(next[0]).toMatchObject({ compact: true, body: 'older turns folded' })
    expect(next[1]).toMatchObject({ body: 'continuing', streaming: false })
  })

  test('applyNotice is not assistant text and applyStream does not overwrite it', () => {
    const live = applyNotice([], 'Waiting for previous run run-1 to finish.')
    expect(live[0]).toMatchObject({ note: true, body: 'Waiting for previous run run-1 to finish.' })
    const next = applyStream(live, 'text', 'delivered', { done: true })
    expect(next).toHaveLength(2)
    expect(next[0]).toMatchObject({ note: true, body: 'Waiting for previous run run-1 to finish.' })
    expect(next[1]).toMatchObject({ body: 'delivered', streaming: false })
    const replaced = applyNotice(live, 'Previous run did not finish; cancelling and retrying.')
    expect(replaced).toHaveLength(1)
    expect(replaced[0]).toMatchObject({ note: true, body: 'Previous run did not finish; cancelling and retrying.' })
  })

  test('mapChatItems and applyFallback render model-fallback rows', () => {
    const mapped = mapChatItems([
      { role: 'assistant', kind: 'fallback', text: 'Opus 4.8', description: 'Fable 5' },
      { role: 'assistant', kind: 'text', text: 'continuing' },
    ])
    expect(mapped[0]).toMatchObject({ fallback: true, body: 'Opus 4.8', description: 'Fable 5' })
    const live = applyFallback([], { from: 'claude-fable-5', to: 'claude-opus-4-8', fromLabel: 'Fable 5', toLabel: 'Opus 4.8' })
    expect(live[0]).toMatchObject({ fallback: true, body: 'Opus 4.8', description: 'Fable 5' })
    const dup = applyFallback(live, { toLabel: 'Opus 4.8', fromLabel: 'Fable 5' })
    expect(dup).toHaveLength(1)
    const incomplete = applyFallback([], { toLabel: 'Opus 4.8' })
    expect(incomplete[0]).toMatchObject({ fallback: true, body: 'Opus 4.8', description: '' })
    const enriched = applyFallback(incomplete, { toLabel: 'Opus 4.8', fromLabel: 'Fable 5' })
    expect(enriched).toHaveLength(1)
    expect(enriched[0]).toMatchObject({ fallback: true, body: 'Opus 4.8', description: 'Fable 5' })
    const retracted = applyFallback(
      [{ body: 'refused', providerUUID: 'fable-frame' }, { body: 'opus', providerUUID: 'opus-frame' }],
      { toLabel: 'Opus 4.8', fromLabel: 'Fable 5', retracted: ['fable-frame'] },
    )
    expect(retracted).toHaveLength(2)
    expect(retracted[0]).toMatchObject({ fallback: true, body: 'Opus 4.8' })
    expect(retracted[1]).toMatchObject({ body: 'opus', providerUUID: 'opus-frame' })
  })

  test('applyStream splits a new provider frame uuid onto a new row', () => {
    const first = applyStream([], 'text', 'fable', { uuid: 'a' })
    const next = applyStream(first, 'text', 'opus', { uuid: 'b' })
    expect(next).toHaveLength(2)
    expect(next[0]).toMatchObject({ body: 'fable', streaming: false, providerUUID: 'a' })
    expect(next[1]).toMatchObject({ body: 'opus', streaming: true, providerUUID: 'b' })
  })

  test('noteThinkingTokens stamps providerUUID so retraction can drop the row', () => {
    const thought = noteThinkingTokens([], 12, 'fable-frame')
    expect(thought[0]).toMatchObject({ thinking: true, tokens: 12, providerUUID: 'fable-frame' })
    const retracted = applyFallback(thought, {
      toLabel: 'Opus 4.8',
      retracted: ['fable-frame'],
    })
    expect(retracted).toHaveLength(1)
    expect(retracted[0]).toMatchObject({ fallback: true, body: 'Opus 4.8' })
  })

  test('noteThinkingTokens keeps one row across per-event uuids', () => {
    const first = noteThinkingTokens([], 326, 'evt-1')
    const next = noteThinkingTokens(first, 340, 'evt-2')
    expect(next).toHaveLength(1)
    expect(next[0]).toMatchObject({ thinking: true, streaming: true, tokens: 340, providerUUID: 'evt-1' })
  })

  test('thinking deltas and token ticks share one live Thought row', () => {
    let list = noteThinkingTokens([], 326)
    list = applyStream(list, 'thinking', 'Act')
    list = noteThinkingTokens(list, 340)
    list = applyStream(list, 'thinking', 'Actually')
    expect(list).toHaveLength(1)
    expect(list[0]).toMatchObject({ thinking: true, streaming: true, body: 'Actually', tokens: 340 })
  })

  test('applyStream done-only text does not overwrite a fallback row', () => {
    const fallback = applyFallback([], { toLabel: 'Opus 4.8', fromLabel: 'Fable 5' })
    const next = applyStream(fallback, 'text', 'continuing', { done: true })
    expect(next).toHaveLength(2)
    expect(next[0]).toMatchObject({ fallback: true, body: 'Opus 4.8' })
    expect(next[1]).toMatchObject({ body: 'continuing', streaming: false })
  })

  test('applyStream done does not rewrite a previous turn above a user answer', () => {
    const list: AgentMsg[] = [
      { thinking: true, body: 'need a decision' },
      { me: true, body: 'use notes', ts: 5000 },
    ]
    const next = applyStream(list, 'thinking', 'resuming', { done: true })
    expect(next[0]).toMatchObject({ thinking: true, body: 'need a decision' })
    expect(next[1]).toMatchObject({ me: true, body: 'use notes' })
    expect(next[2]).toMatchObject({ thinking: true, body: 'resuming', streaming: false })
  })

  test('applyStream continues a settled prefix after a remount lost streaming', () => {
    const list: AgentMsg[] = [{ body: 'hello', widenStream: true }]
    const next = applyStream(list, 'text', 'hello world')
    expect(next).toHaveLength(1)
    expect(next[0]).toMatchObject({ body: 'hello world', streaming: true, widenStream: false })
  })

  test('applyStream does not merge a second consecutive text block into a remounted row', () => {
    const list: AgentMsg[] = [{ body: 'Let me check.', widenStream: true }]
    const early = applyStream(list, 'text', 'L')
    expect(early).toHaveLength(2)
    expect(early[0]).toMatchObject({ body: 'Let me check.', widenStream: false })
    expect(early[1]).toMatchObject({ body: 'L', streaming: true })
    const grown = applyStream(early, 'text', 'Later I will edit.')
    expect(grown).toHaveLength(2)
    expect(grown[1]).toMatchObject({ body: 'Later I will edit.', streaming: true })
  })

  test('applyStream without widenStream opens a new row after a settled text block', () => {
    const list: AgentMsg[] = [{ body: 'Let me check.' }]
    const next = applyStream(list, 'text', 'Let me check. Then I will edit.')
    expect(next).toHaveLength(2)
    expect(next[1]).toMatchObject({ body: 'Let me check. Then I will edit.', streaming: true })
  })

  test('applyStream thinking done does not rewrite the first thought after tools', () => {
    const list: AgentMsg[] = [
      { thinking: true, body: 'first block' },
      { tool: 'Read', toolId: 't1', result: 'ok' },
    ]
    const next = applyStream(list, 'thinking', 'second block', { done: true })
    expect(next[0]).toMatchObject({ thinking: true, body: 'first block' })
    expect(next[1]).toMatchObject({ tool: 'Read' })
    expect(next[2]).toMatchObject({ thinking: true, body: 'second block', streaming: false })
  })

  test('applyCompact started after a finished compact appends a new row', () => {
    const first = applyCompact([], { status: 'done', summary: 'first fold' })
    const second = applyCompact(first, { status: 'started', preTokens: 90000 })
    expect(second).toHaveLength(2)
    expect(second[0]).toMatchObject({ compact: true, compacting: false, body: 'first fold' })
    expect(second[1]).toMatchObject({ compact: true, compacting: true, tokens: 90000, body: '' })
  })

  test('dropOpenCompact removes an interrupted compacting row', () => {
    const live = applyCompact([{ body: 'hello' }], { status: 'started', preTokens: 180000 })
    expect(dropOpenCompact(live)).toEqual([{ body: 'hello' }])
  })
})

describe('addUser', () => {
  test('appends a Maestro steer as its own user row', () => {
    const list = addUser([{ body: 'assistant' }], '  please stop  ')
    expect(list).toHaveLength(2)
    expect(list[1]).toMatchObject({ me: true, body: 'please stop' })
  })

  test('appends a second user after other rows', () => {
    const list = addUser([{ body: 'assistant' }], 'Please review the changes')
    expect(list).toHaveLength(2)
  })

  test('ignores blank text', () => {
    const list = [{ body: 'assistant' }]
    expect(addUser(list, '   ')).toBe(list)
  })

  test('does not duplicate an optimistic bubble of the same text', () => {
    const list = addUser([{ me: true, body: 'please stop', optimistic: true }], 'please stop')
    expect(list).toHaveLength(1)
    expect(list[0].optimistic).toBe(false)
  })

  test('settles optimistic caption when session.user echoes the attachment marker', () => {
    const atts = [{ id: 'img_a', name: 'shot.png', mime: 'image/png', dataUrl: 'data:image/png;base64,xx' }]
    const list = addUser(
      [{ me: true, body: 'fix the layout', optimistic: true, attachments: atts }],
      `${attachmentMarker('img_a')}fix the layout`,
    )
    expect(list).toHaveLength(1)
    expect(list[0]).toMatchObject({ me: true, optimistic: false, attachments: atts })
  })

  test('appends a repeated steer when the prior row is already real', () => {
    const list = addUser([{ me: true, body: 'please stop' }], 'please stop')
    expect(list).toHaveLength(2)
    expect(list[1]).toMatchObject({ me: true, body: 'please stop' })
  })

  test('settles the oldest unmatched optimistic of the same text', () => {
    const list = addUser(
      [
        { me: true, body: 'please stop', optimistic: true },
        { me: true, body: 'please stop', optimistic: true },
      ],
      'please stop',
    )
    expect(list).toHaveLength(2)
    expect(list[0].optimistic).toBe(false)
    expect(list[1].optimistic).toBe(true)
  })

  test('closes a live assistant row so a later delta cannot open a second live-text', () => {
    const list = addUser([{ body: 'working', streaming: true }], 'steer')
    expect(list[0].streaming).toBe(false)
    expect(list[1]).toMatchObject({ me: true, body: 'steer' })
    const next = applyStream(list, 'text', 'ok after steer')
    expect(next.filter((m) => m.streaming && !m.thinking)).toHaveLength(1)
    expect(next[0].streaming).toBe(false)
    expect(next[2]).toMatchObject({ body: 'ok after steer', streaming: true })
  })
})

describe('applyStream after a mid-turn user', () => {
  test('closes the prior live-text when the optimistic user is already in the list', () => {
    const next = applyStream(
      [{ body: 'working', streaming: true }, { me: true, body: 'steer', optimistic: true }],
      'text',
      'ok after steer',
    )
    expect(next.filter((m) => m.streaming && !m.thinking)).toHaveLength(1)
    expect(next[0].streaming).toBe(false)
    expect(next[2]).toMatchObject({ body: 'ok after steer', streaming: true })
  })
})

describe('addTool / attachToolResult streaming frames', () => {
  // Live grok-4.6 provider_tool_use_id values from Recur's session store
  // (this workspace, 2026-08-12). Grok joins call-… and fc_… with a newline.
  const grokGlobTODO =
    'call-9812a46f-b258-4ed8-8d09-55441deb1733-5\nfc_acab39d7-9d73-9fd7-9c87-51495bec8eb8_0'
  const grokGlobREADME =
    'call-9812a46f-b258-4ed8-8d09-55441deb1733-6\nfc_acab39d7-9d73-9fd7-9c87-51495bec8eb8_1'
  const grokGlobMakefile =
    'call-9812a46f-b258-4ed8-8d09-55441deb1733-7\nfc_acab39d7-9d73-9fd7-9c87-51495bec8eb8_2'

  const step = (over: Partial<WorkspaceStep> = {}): WorkspaceStep => ({
    id: grokGlobTODO,
    tool: 'Glob',
    toolInput: 'TODO.md',
    description: '',
    input: '{"globPattern":"TODO.md"}',
    ...over,
  })

  test('repeated session.step with a live grok-4.6 id stays one row', () => {
    let list = addTool([], step())
    list = addTool(list, step())
    list = addTool(list, step())
    list = addTool(list, step())
    list = addTool(list, step())
    expect(list).toHaveLength(1)
    expect(list[0]?.toolId).toBe(grokGlobTODO)
    expect(list[0]?.resultPending).toBe(true)
    expect(list[0]?.input).toBe('{"globPattern":"TODO.md"}')
  })

  test('session.step adopts a store snapshot row that omitted the provider id', () => {
    const list = addTool(
      [{ tool: 'Glob', toolInput: 'TODO.md', resultPending: true }],
      step(),
    )
    expect(list).toHaveLength(1)
    expect(list[0]).toMatchObject({
      tool: 'Glob',
      toolInput: 'TODO.md',
      toolId: grokGlobTODO,
      resultPending: true,
    })
  })

  test('parallel grok-4.6 globs from the same batch stay distinct', () => {
    let list = addTool([], step({ id: grokGlobTODO, input: '{"globPattern":"TODO.md"}' }))
    list = addTool(list, step({
      id: grokGlobREADME, toolInput: 'README*', input: '{"globPattern":"README*"}',
    }))
    list = addTool(list, step({
      id: grokGlobMakefile, toolInput: 'Makefile', input: '{"globPattern":"Makefile"}',
    }))
    expect(list).toHaveLength(3)
    expect(list.map((m) => m.toolId)).toEqual([grokGlobTODO, grokGlobREADME, grokGlobMakefile])
    expect(list.map((m) => m.input)).toEqual([
      '{"globPattern":"TODO.md"}',
      '{"globPattern":"README*"}',
      '{"globPattern":"Makefile"}',
    ])
  })

  test('blank-id repeats of the same tool reuse the pending row', () => {
    let list = addTool([], step({ id: '', tool: 'Glob', toolInput: '**/.golangci.*' }))
    list = addTool(list, step({ id: '', tool: 'Glob', toolInput: '**/.golangci.*' }))
    list = addTool(list, step({ id: '', tool: 'Glob', input: '{"pattern":"**/.golangci.*"}' }))
    expect(list).toHaveLength(1)
    expect(list[0]?.tool).toBe('Glob')
    expect(list[0]?.input).toBe('{"pattern":"**/.golangci.*"}')
  })

  test('attachToolResult drops leftover in-flight clones of a grok-4.6 id', () => {
    const clones = [
      { tool: 'Glob', toolId: grokGlobTODO, resultPending: true, input: '{"globPattern":"TODO.md"}' },
      { tool: 'Glob', toolId: grokGlobTODO, resultPending: true, input: '{"globPattern":"TODO.md"}' },
      { thinking: true, body: 'next' },
    ]
    const out = attachToolResult(clones, grokGlobTODO, './TODO.md', false)
    expect(out.filter((m) => m.tool === 'Glob')).toHaveLength(1)
    expect(out.find((m) => m.tool === 'Glob')).toMatchObject({
      result: './TODO.md',
      resultPending: false,
    })
    expect(out.some((m) => m.thinking)).toBe(true)
  })

  test('attachToolResult with a blank id does not drop other in-flight blank-id tools', () => {
    const list = [
      { tool: 'Glob', toolId: '', resultPending: true, input: '{"globPattern":"**"}' },
      { tool: 'Read', toolId: '', resultPending: true, input: '{"path":"README.md"}' },
    ]
    const out = attachToolResult(list, '', 'file contents', false)
    expect(out).toHaveLength(2)
    expect(out.find((m) => m.tool === 'Read')).toMatchObject({
      result: 'file contents',
      resultPending: false,
    })
    expect(out.find((m) => m.tool === 'Glob')).toMatchObject({
      resultPending: true,
      input: '{"globPattern":"**"}',
    })
  })

  test('addTool with a known id does not resurrect a settled row', () => {
    let list = addTool([], step())
    list = attachToolResult(list, grokGlobTODO, './TODO.md', false)
    expect(list[0]?.resultPending).toBe(false)
    list = addTool(list, step())
    expect(list).toHaveLength(2)
    expect(list[0]).toMatchObject({
      toolId: grokGlobTODO,
      result: './TODO.md',
      resultPending: false,
    })
    expect(list[1]).toMatchObject({
      toolId: grokGlobTODO,
      resultPending: true,
    })
  })

  test('blank-id upsert stops at user and turnEnd rows', () => {
    const acrossMe = addTool(
      [
        { tool: 'Glob', toolId: '', resultPending: true, toolInput: 'old' },
        { me: true, body: 'next' },
      ],
      step({ id: '', tool: 'Glob', toolInput: 'new' }),
    )
    expect(acrossMe).toHaveLength(3)
    expect(acrossMe[0]?.toolInput).toBe('old')
    expect(acrossMe[2]?.toolInput).toBe('new')

    const acrossEnd = addTool(
      [
        { tool: 'Read', toolId: '', resultPending: true, toolInput: 'old.ts' },
        { turnEnd: true, durationMs: 12 },
      ],
      step({ id: '', tool: 'Read', toolInput: 'new.ts' }),
    )
    expect(acrossEnd).toHaveLength(3)
    expect(acrossEnd[0]?.toolInput).toBe('old.ts')
    expect(acrossEnd[2]?.toolInput).toBe('new.ts')
  })

  test('blank-id upsert skips queued follow-ups at the tail', () => {
    let list = addTool([], step({ id: '', tool: 'Glob', toolInput: '**/.golangci.*' }))
    list = [...list, { me: true, body: 'follow-up', queued: true, pendingId: 'p1' }]
    list = addTool(list, step({ id: '', tool: 'Glob', input: '{"pattern":"**/.golangci.*"}' }))
    expect(list.filter((m) => m.tool === 'Glob')).toHaveLength(1)
    expect(list[0]?.input).toBe('{"pattern":"**/.golangci.*"}')
    expect(list[1]?.queued).toBe(true)
  })

  test('stepRowKey is unique when two rows share a toolId', () => {
    const a = stepRowKey(grokGlobTODO, 0)
    const b = stepRowKey(grokGlobTODO, 1)
    expect(a).not.toBe(b)
    expect(stepRowKey(undefined, 3)).toBe('i3')
  })

  test('nested subagent Grep does not leak into the main list', () => {
    let list = addTool([], step({
      id: 'toolu_task_1', tool: 'Task', description: 'Map UI', input: '{"prompt":"search"}',
    }))
    list = addTool(list, step({
      id: 't_nested', tool: 'Grep', toolInput: 'PRDetail',
      input: '{"pattern":"PRDetail"}', parentId: 'toolu_task_1',
    }))
    expect(list).toHaveLength(1)
    expect(list[0]?.tool).toBe('Task')
    expect(list.map((m) => m.tool)).not.toContain('Grep')
    expect(list[0]?.children).toHaveLength(1)
    expect(list[0]?.children?.[0]).toMatchObject({
      tool: 'Grep', toolId: 't_nested', resultPending: true,
    })
  })

  test('nested stream text stays inside the parent card', () => {
    let list = addTool([], step({ id: 'toolu_task_1', tool: 'Task' }))
    list = applyStream(list, 'text', 'Found foo.go', { done: true, parentId: 'toolu_task_1' })
    expect(list).toHaveLength(1)
    expect(list[0]?.body).toBeUndefined()
    expect(list[0]?.children?.[0]).toMatchObject({ body: 'Found foo.go', streaming: false })
  })

  test('attachToolResult settles a nested subagent tool without a sibling row', () => {
    let list = addTool([], step({ id: 'toolu_task_1', tool: 'Task' }))
    list = addTool(list, step({ id: 't_nested', tool: 'Grep', parentId: 'toolu_task_1' }))
    list = attachToolResult(list, 't_nested', 'src/PR.svelte', false)
    expect(list).toHaveLength(1)
    expect(list[0]?.resultPending).toBe(true)
    expect(list[0]?.children?.[0]).toMatchObject({
      result: 'src/PR.svelte', resultPending: false,
    })
  })

  test('nested events drop when the parent card is missing', () => {
    const list = addTool([], step({
      id: 't_nested', tool: 'Grep', parentId: 'toolu_missing',
    }))
    expect(list).toHaveLength(0)
  })

  test('mapChatItems nests persisted children and keeps toolId', () => {
    const mapped = mapChatItems([
      {
        role: 'assistant', kind: 'tool', tool: 'Task', toolId: 'toolu_task_1',
        description: 'Map UI',
        children: [
          { role: 'assistant', kind: 'tool', tool: 'Grep', toolId: 't_nested', toolInput: 'PRDetail', result: 'src/PR.svelte' },
          { role: 'assistant', kind: 'text', text: 'Found foo.go' },
        ],
      },
    ])
    expect(mapped).toHaveLength(1)
    expect(mapped[0]?.toolId).toBe('toolu_task_1')
    expect(mapped[0]?.children).toHaveLength(2)
    expect(mapped[0]?.children?.[0]).toMatchObject({ tool: 'Grep', toolId: 't_nested', result: 'src/PR.svelte' })
    expect(mapped[0]?.children?.[1]).toMatchObject({ body: 'Found foo.go' })
  })
})

describe('draftsFromImagePaths', () => {
  afterEach(() => {
    vi.unstubAllGlobals()
  })

  test('chatImageURL encodes the absolute path', () => {
    expect(chatImageURL('/Users/me/Desktop/shot.png')).toBe(
      '/chat-image?path=%2FUsers%2Fme%2FDesktop%2Fshot.png',
    )
  })

  test('skips failed fetches', async () => {
    vi.stubGlobal('fetch', async () => new Response('', { status: 404 }))
    expect(await draftsFromImagePaths(['/Users/me/Desktop/shot.png'])).toEqual([])
  })
})

describe('dropOptimisticUser', () => {
  test('removes a matching optimistic row after a turn footer', () => {
    const list: AgentMsg[] = [
      { me: true, body: 'steer', optimistic: true, pendingId: 'p1' },
      { turnEnd: true, durationMs: 1200 },
    ]
    expect(dropOptimisticUser(list, 'steer', 'p1')).toEqual([{ turnEnd: true, durationMs: 1200 }])
  })

  test('falls back to display text when pending id is missing', () => {
    const list: AgentMsg[] = [
      { me: true, body: 'steer', optimistic: true },
      { body: 'working' },
    ]
    expect(dropOptimisticUser(list, 'steer')).toEqual([{ body: 'working' }])
  })
})

describe('mergeRejectedSteerDraft', () => {
  test('restores failed steers ahead of later typed text, in send order', () => {
    expect(mergeRejectedSteerDraft(['A'], 'C')).toBe('A\n\nC')
    expect(mergeRejectedSteerDraft(['A', 'B'], 'C')).toBe('A\n\nB\n\nC')
    expect(mergeRejectedSteerDraft(['A', 'B'], 'B\n\nC')).toBe('A\n\nB\n\nC')
  })

  test('overlapping rejections do not duplicate a multi-paragraph restored steer', () => {
    expect(mergeRejectedSteerDraft(['A1\n\nA2', 'B'], 'A1\n\nA2')).toBe('A1\n\nA2\n\nB')
    expect(mergeRejectedSteerDraft(['A1\n\nA2', 'B'], 'A1\n\nA2\n\nC')).toBe('A1\n\nA2\n\nB\n\nC')
  })
})

describe('mergeRejectedSteerAttachments', () => {
  test('keeps failed drafts then later picks, without duplicates', () => {
    expect(mergeRejectedSteerAttachments(
      [[{ id: 'a' }, { id: 'b' }]],
      [{ id: 'b' }, { id: 'c' }],
    )).toEqual([{ id: 'a' }, { id: 'b' }, { id: 'c' }])
  })
})

describe('Task / subagent helpers', () => {
  test('isTaskTool recognizes Task, task, and Agent', () => {
    expect(isTaskTool('Task')).toBe(true)
    expect(isTaskTool('task')).toBe(true)
    expect(isTaskTool('Agent')).toBe(true)
    expect(isTaskTool('Bash')).toBe(false)
    expect(isTaskTool(undefined)).toBe(false)
  })

  test('isSubagentLaunchMeta catches Claude Code background-Task acks', () => {
    const ack = `Async agent launched successfully.
(This tool result is internal metadata – never quote or paste any part of it, including the agentId below, into a user-facing reply.)

agentId: a2a1c3c855409bb10
Use SendMessage with to: "a2a1c3c855409bb10"`
    expect(isSubagentLaunchMeta(ack)).toBe(true)
    expect(isSubagentLaunchMeta('Explored the PR detail Svelte view; tabs live in PRDetail.svelte.')).toBe(false)
    expect(isSubagentLaunchMeta('')).toBe(false)
    expect(isSubagentLaunchMeta(undefined)).toBe(false)
    expect(isSubagentLaunchMeta(
      'see Async agent launched successfully plus agentId and internal metadata in comments',
    )).toBe(false)
  })
})
