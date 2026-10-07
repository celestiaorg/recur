import { describe, expect, test } from 'vitest'
import { latestTodosFromMessages, parseTodos, todoCounts } from './todos'

describe('parseTodos', () => {
  test('reads the common TodoWrite shape', () => {
    const todos = parseTodos(JSON.stringify({
      todos: [
        { content: 'Read files', status: 'completed' },
        { content: 'Edit panel', status: 'in_progress' },
        { content: '  ', status: 'pending' },
      ],
    }))
    expect(todos).toEqual([
      { content: 'Read files', status: 'completed' },
      { content: 'Edit panel', status: 'in_progress' },
    ])
  })

  test('returns empty on junk', () => {
    expect(parseTodos(undefined)).toEqual([])
    expect(parseTodos('{')).toEqual([])
    expect(parseTodos(JSON.stringify({ todos: 'nope' }))).toEqual([])
  })
})

describe('latestTodosFromMessages', () => {
  test('uses the last TodoWrite with items', () => {
    const todos = latestTodosFromMessages([
      { tool: 'TodoWrite', input: JSON.stringify({ todos: [{ content: 'old', status: 'pending' }] }) },
      { tool: 'Read', input: '{"file_path":"x"}' },
      { tool: 'TodoWrite', input: JSON.stringify({ todos: [{ content: 'new', status: 'completed' }] }) },
    ])
    expect(todos).toEqual([{ content: 'new', status: 'completed' }])
    expect(todoCounts(todos)).toEqual({ done: 1, total: 1 })
  })
})
