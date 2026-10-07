/** @vitest-environment happy-dom */
import { afterEach, expect, test } from 'vitest'
import { flushSync, mount, unmount } from 'svelte'
import { fromStore, writable } from 'svelte/store'
import AgentChat, { type AgentMsg, type ChatLogSession } from './AgentChat.svelte'
import SessionCard from './SessionCard.svelte'
import Harness from './AgentChat.harness.svelte'

afterEach(() => {
  document.body.replaceChildren()
})

function coder(extra?: Partial<ChatLogSession>): ChatLogSession {
  return {
    kind: 'session',
    id: 's-co#0',
    code: 'CO',
    name: 'Coder',
    live: false,
    messages: [{ body: 'done', ts: 1100 }],
    ...extra,
  }
}

test('plain notices stay escaped; engine wrap facts render HTML', () => {
  const target = document.createElement('div')
  document.body.append(target)
  const messages = fromStore(writable<AgentMsg[]>([
    { note: true, noteLabel: 'Subagent', body: 'fix <img src=x onerror=alert(1)> (child-1)', ts: 1 },
  ]))
  const component = mount(AgentChat, {
    target,
    props: {
      readonly: true,
      get messages() { return messages.current },
    },
  })
  flushSync()
  const notice = target.querySelector('.tc.notice .tc-meta')
  expect(notice?.querySelector('img')).toBeNull()
  expect(notice?.textContent).toContain('<img src=x onerror=alert(1)>')

  messages.current = [
    { note: true, noteLabel: 'Commit', noteHtml: true, body: 'Committed <code>abc</code> to <code>main</code>.', ts: 2 },
  ]
  flushSync()
  expect(target.querySelector('.tc.notice .tc-meta code')?.textContent).toBe('abc')
  unmount(component)
})

test('Subagent notices fold into the following steps group', () => {
  const target = document.createElement('div')
  document.body.append(target)
  const messages = fromStore(writable<AgentMsg[]>([
    { note: true, noteLabel: 'Subagent', body: 'Goal finished (child-1)', ts: 1 },
    { tool: 'Read', toolInput: 'a.ts', result: 'ok', ts: 2 },
    { tool: 'Grep', toolInput: 'TODO', result: 'none', ts: 3 },
  ]))
  const component = mount(AgentChat, {
    target,
    props: {
      readonly: true,
      get messages() { return messages.current },
    },
  })
  flushSync()
  expect(target.querySelectorAll('.steps')).toHaveLength(1)
  expect(target.querySelector('.cblk .tc.notice')).toBeNull()
  expect(target.querySelector('.steps-head')?.textContent).toContain('2 tool calls')

  target.querySelector<HTMLButtonElement>('.steps-head')!.click()
  flushSync()
  const folded = target.querySelector('.steps .tc.notice')
  expect(folded).toBeTruthy()
  expect(folded?.textContent).toContain('Goal finished (child-1)')

  messages.current = [
    { note: true, noteLabel: 'Commit', noteHtml: true, body: 'Committed <code>abc</code> to <code>main</code>.', ts: 1 },
    { tool: 'Read', toolInput: 'a.ts', result: 'ok', ts: 2 },
  ]
  flushSync()
  expect(target.querySelector('.steps .tc.notice')).toBeNull()
  expect(target.querySelector('.tc.notice')?.textContent).toContain('abc')
  expect(target.querySelector('.steps')).toBeTruthy()

  messages.current = [
    { note: true, noteLabel: 'Subagent', body: 'Goal finished (child-1)', ts: 1 },
  ]
  flushSync()
  expect(target.querySelector('.tc.notice')).toBeTruthy()
  expect(target.querySelector('.steps')).toBeNull()
  unmount(component)
})

test('collapsed SessionCard meta escapes ordinary notes', () => {
  const target = document.createElement('div')
  document.body.append(target)
  const component = mount(SessionCard, {
    target,
    props: {
      code: 'CO',
      name: 'Coder',
      live: false,
      messages: [{ note: true, noteLabel: 'Subagent', body: 'fix <b>xss</b> (child-1)' }],
    },
  })
  flushSync()
  const meta = target.querySelector('.sess-meta')
  expect(meta?.querySelector('b')).toBeNull()
  expect(meta?.textContent).toContain('<b>xss</b>')
  unmount(component)
})

test('SessionCard stays expanded when a sibling specialist joins the group', () => {
  const target = document.createElement('div')
  document.body.append(target)
  const messages = fromStore(writable<AgentMsg[]>([{ ts: 1000, log: coder() }]))
  const component = mount(Harness, {
    target,
    props: {
      get messages() { return messages.current },
    },
  })
  flushSync()
  const head = target.querySelector<HTMLButtonElement>('.sess-head')
  expect(head?.getAttribute('aria-expanded')).toBe('false')
  head!.click()
  flushSync()
  expect(head?.getAttribute('aria-expanded')).toBe('true')
  expect(target.querySelector('.fanout-transcript')).toBeTruthy()

  messages.current = [{
    ts: 1000,
    log: {
      kind: 'sessions',
      id: 's-co#0',
      sessions: [
        coder(),
        { kind: 'session', id: 's-rv#0', code: 'RV', name: 'Reviewer', live: true, messages: [] },
      ],
    },
  }]
  flushSync()
  const heads = target.querySelectorAll('.sess-head')
  expect(heads).toHaveLength(2)
  expect(heads[0].getAttribute('aria-expanded')).toBe('true')
  expect(target.querySelector('.fanout-transcript')).toBeTruthy()
  unmount(component)
})

test('final reply stays visible when compaction follows the turn', () => {
  const target = document.createElement('div')
  document.body.append(target)
  const component = mount(AgentChat, {
    target,
    props: {
      readonly: true,
      messages: [
        { me: true, body: 'where is check-in?', ts: 1 },
        { tool: 'Read', toolInput: 'checkin.go', result: 'ok', ts: 2 },
        { thinking: true, body: 'looking at mockup vs product', ts: 3 },
        { body: 'Check-in only renders when Apollo Sync returns one.', ts: 4 },
        { compact: true, body: 'folded prior turns', ts: 5 },
        { turnEnd: true, durationMs: 42000, ts: 6 },
      ],
    },
  })
  flushSync()
  expect(target.querySelector('.steps-head')?.textContent).toContain('compaction')
  expect(target.querySelector('.steps')?.textContent).not.toContain('Apollo Sync')
  expect(target.querySelector('.chat-row.assistant')?.textContent).toContain('Apollo Sync')
  unmount(component)
})
