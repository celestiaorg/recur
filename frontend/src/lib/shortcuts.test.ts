import { describe, expect, test } from 'vitest'
import { SHORTCUTS } from './shortcuts'

describe('SHORTCUTS', () => {
  test('lists ⌘L add file / selection to chat under Navigation', () => {
    const entry = SHORTCUTS.find((s) => s.keys === '⌘L')
    expect(entry).toEqual({
      group: 'Navigation',
      label: 'Add file / selection to chat',
      keys: '⌘L',
      terms: ['@ file', 'mention', 'reference'],
    })
  })

  test('lists ⌘R run picker under Workbench', () => {
    const entry = SHORTCUTS.find((s) => s.keys === '⌘R')
    expect(entry).toEqual({
      group: 'Workbench',
      label: 'Run…',
      keys: '⌘R',
      terms: ['dev server', 'run command', 'run menu'],
    })
  })

  test('lists ⌘⇧R stop run under Workbench', () => {
    const entry = SHORTCUTS.find((s) => s.keys === '⌘⇧R')
    expect(entry).toEqual({
      group: 'Workbench',
      label: 'Stop run',
      keys: '⌘⇧R',
      terms: ['dev server', 'interrupt'],
    })
  })

  test('lists ⇧⌘N new blank workspace under General', () => {
    const entry = SHORTCUTS.find((s) => s.keys === '⇧⌘N' && s.group === 'General')
    expect(entry).toEqual({
      group: 'General',
      label: 'New blank workspace',
      keys: '⇧⌘N',
      terms: ['quick create', 'empty'],
    })
  })

  test('lists ⌥⌘O open file or folder under General', () => {
    const entry = SHORTCUTS.find((s) => s.keys === '⌥⌘O' && s.group === 'General')
    expect(entry).toEqual({
      group: 'General',
      label: 'Open file or folder',
      keys: '⌥⌘O',
      terms: ['open', 'folder', 'file'],
    })
  })

  test('lists ⌘O open in under General', () => {
    const entry = SHORTCUTS.find((s) => s.keys === '⌘O')
    expect(entry).toEqual({
      group: 'General',
      label: 'Open in…',
      keys: '⌘O',
      terms: ['external editor', 'finder', 'terminal'],
    })
  })

  test('lists ⌃⇧↑ / ⌃⇧↓ add-cursor under Editor', () => {
    expect(SHORTCUTS.find((s) => s.keys === '⌃⇧↑ ⌃⇧↓')).toEqual({
      group: 'Editor',
      label: 'Add cursor above / below',
      keys: '⌃⇧↑ ⌃⇧↓',
      terms: ['multi cursor'],
    })
  })

  test('lists ⌥⌘↑ / ⌥⌘↓ workspace cycling under Navigation', () => {
    expect(SHORTCUTS.find((s) => s.keys === '⌥⌘↑')).toEqual({
      group: 'Navigation',
      label: 'Previous workspace',
      keys: '⌥⌘↑',
      terms: ['cycle'],
    })
    expect(SHORTCUTS.find((s) => s.keys === '⌥⌘↓')).toEqual({
      group: 'Navigation',
      label: 'Next workspace',
      keys: '⌥⌘↓',
      terms: ['cycle'],
    })
  })

  test('lists ⌥⌘Z revert diff hunk under Editor', () => {
    const entry = SHORTCUTS.find((s) => s.keys === '⌥⌘Z' && s.label === 'Revert diff hunk')
    expect(entry).toEqual({
      group: 'Editor',
      label: 'Revert diff hunk',
      keys: '⌥⌘Z',
      terms: ['diff', 'reject', 'undo hunk'],
    })
  })

  test('lists ⌘⇧V cycle editor view under Editor', () => {
    const entry = SHORTCUTS.find((s) => s.keys === '⌘⇧V')
    expect(entry).toEqual({
      group: 'Editor',
      label: 'Cycle view',
      keys: '⌘⇧V',
      terms: ['code', 'diff', 'preview', 'toggle'],
    })
  })

  test('lists ⌥⌘[ / ⌥⌘] editor theme cycling under Editor', () => {
    expect(SHORTCUTS.find((s) => s.keys === '⌥⌘[ ⌥⌘]')).toEqual({
      group: 'Editor',
      label: 'Previous / next editor theme',
      keys: '⌥⌘[ ⌥⌘]',
      terms: ['cycle', 'color scheme'],
    })
  })

  test('lists J / K review file stepping under Workbench', () => {
    expect(SHORTCUTS.find((s) => s.keys === 'J / K')).toEqual({
      group: 'Workbench',
      label: 'Review: next / previous file',
      keys: 'J / K',
      terms: ['diff', 'review'],
    })
  })

  test('lists ⌘Click open diff line under Workbench', () => {
    expect(SHORTCUTS.find((s) => s.keys === '⌘Click')).toEqual({
      group: 'Workbench',
      label: 'Open diff line in editor',
      keys: '⌘Click',
      terms: ['review', 'diff', 'jump'],
    })
  })
})
