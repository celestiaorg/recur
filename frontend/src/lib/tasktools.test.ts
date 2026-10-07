import { describe, expect, test } from 'vitest'
import {
  isClaudeTaskTool,
  formatTaskId,
  prettyTaskStatus,
  taskStatusChipClass,
  parseClaudeTaskArgs,
  parseTaskCreateId,
  parseTaskList,
  parseTaskGet,
  claudeTaskCounts,
  claudeTaskLabel,
  claudeTaskMeta,
  claudeTaskChipStatus,
  claudeTaskHasBody,
  claudeTaskResultIsAck,
} from './tasktools'

const createArgs = parseClaudeTaskArgs({
  subject: 'Guard theme re-apply (registry + host)',
  description: 'setActiveTheme memoize; applyColorMap no-op on identical maps.',
  activeForm: 'Guarding theme re-apply',
})

describe('isClaudeTaskTool', () => {
  test('matches orchestration tools, not the Task subagent', () => {
    expect(isClaudeTaskTool('TaskCreate')).toBe(true)
    expect(isClaudeTaskTool('TaskUpdate')).toBe(true)
    expect(isClaudeTaskTool('TaskGet')).toBe(true)
    expect(isClaudeTaskTool('TaskList')).toBe(true)
    expect(isClaudeTaskTool('Task')).toBe(false)
    expect(isClaudeTaskTool('TodoWrite')).toBe(false)
  })
})

describe('claudeTaskLabel / meta', () => {
  test('TaskCreate never uses the long description as the title', () => {
    expect(claudeTaskLabel('TaskCreate', false, createArgs)).toBe('Created task')
    expect(claudeTaskLabel('TaskCreate', true, createArgs)).toBe('Creating task')
    expect(claudeTaskMeta('TaskCreate', createArgs)).toBe('Guard theme re-apply (registry + host)')
    expect(claudeTaskMeta('TaskCreate', createArgs)).not.toContain('{')
    expect(claudeTaskMeta('TaskCreate', createArgs)).not.toContain('setActiveTheme')
  })

  test('TaskUpdate status-only is a verb + #id, never JSON', () => {
    const args = parseClaudeTaskArgs({ taskId: '1', status: 'in_progress' })
    expect(claudeTaskLabel('TaskUpdate', false, args)).toBe('Started task')
    expect(claudeTaskLabel('TaskUpdate', true, args)).toBe('Starting task')
    expect(claudeTaskMeta('TaskUpdate', args)).toBe('')
    expect(claudeTaskMeta('TaskUpdate', args)).not.toContain('taskId')
    expect(claudeTaskHasBody('TaskUpdate', args)).toBe(false)
    expect(claudeTaskChipStatus('TaskUpdate', args)).toBe('in_progress')
  })

  test('TaskUpdate completed / deleted verbs', () => {
    expect(claudeTaskLabel('TaskUpdate', false, parseClaudeTaskArgs({ taskId: '2', status: 'completed' }))).toBe('Completed task')
    expect(claudeTaskLabel('TaskUpdate', false, parseClaudeTaskArgs({ taskId: '2', status: 'deleted' }))).toBe('Deleted task')
    expect(claudeTaskLabel('TaskUpdate', false, parseClaudeTaskArgs({ taskId: '2' }))).toBe('Updated task')
  })

  test('TaskList progress lives in the label', () => {
    expect(claudeTaskLabel('TaskList', true, parseClaudeTaskArgs({}))).toBe('Listing tasks')
    expect(claudeTaskLabel('TaskList', false, parseClaudeTaskArgs({}), { done: 2, total: 6 })).toBe('Listed tasks · 2/6')
    expect(claudeTaskMeta('TaskList', parseClaudeTaskArgs({}),
      '#1 [in_progress] Guard theme re-apply (registry + host)\n#2 [pending] Next',
    )).toBe('Guard theme re-apply (registry + host)')
  })
})

describe('parseTaskCreateId', () => {
  test('reads the CLI success ack', () => {
    expect(parseTaskCreateId('Task #1 created successfully: Guard theme re-apply (registry + host)')).toBe('1')
  })
})

describe('parseTaskList', () => {
  test('parses the CLI line format, keeping parens in the subject', () => {
    const rows = parseTaskList(
      '#1 [in_progress] Guard theme re-apply (registry + host)\n' +
      '#2 [pending] Stop plain-frame dispatch (coder) [blocked by #1]\n' +
      '#3 [completed] Add tm-registry applyColorMap guard tests',
    )
    expect(rows).toHaveLength(3)
    expect(rows![0]).toMatchObject({
      id: '1',
      status: 'in_progress',
      subject: 'Guard theme re-apply (registry + host)',
    })
    expect(rows![1]).toMatchObject({
      id: '2',
      status: 'pending',
      subject: 'Stop plain-frame dispatch',
      owner: 'coder',
      blockedBy: ['1'],
    })
    expect(claudeTaskCounts(rows!)).toEqual({ done: 1, total: 3 })
  })

  test('empty list', () => {
    expect(parseTaskList('No tasks found')).toEqual([])
    expect(parseTaskList('{"tasks":[]}')).toEqual([])
    expect(parseTaskList('[]')).toEqual([])
    expect(parseTaskList('{"data":{"tasks":[]}}')).toEqual([])
  })

  test('JSON {tasks}', () => {
    const rows = parseTaskList(JSON.stringify({
      tasks: [{ id: '4', subject: 'Gate scan', status: 'pending', blockedBy: [] }],
    }))
    expect(rows![0]).toMatchObject({ id: '4', subject: 'Gate scan' })
  })

  test('unrecognized shape is null, not an empty list', () => {
    expect(parseTaskList('• Task 1 — Gate scan (in progress)')).toBeNull()
    expect(parseTaskList('{"ok":true}')).toBeNull()
  })
})

describe('parseTaskGet', () => {
  test('parses the CLI multiline ack', () => {
    const got = parseTaskGet(
      'Task #1: Guard theme re-apply (registry + host)\n' +
      'Status: in_progress\n' +
      'Description: setActiveTheme memoize.\nKeep id equality a no-op.\n' +
      'Blocked by: #2\n' +
      'Blocks: #3, #4',
    )
    expect(got).toMatchObject({
      id: '1',
      subject: 'Guard theme re-apply (registry + host)',
      status: 'in_progress',
      description: 'setActiveTheme memoize.\nKeep id equality a no-op.',
      blockedBy: ['2'],
      blocks: ['3', '4'],
    })
    expect(claudeTaskMeta('TaskGet', parseClaudeTaskArgs({ taskId: '1' }), 
      'Task #1: Guard theme re-apply (registry + host)\nStatus: in_progress\nDescription: x',
    )).toBe('Guard theme re-apply (registry + host)')
  })
})

describe('acks / ids / status copy', () => {
  test('success acks are noise', () => {
    expect(claudeTaskResultIsAck('TaskCreate', 'Task #1 created successfully: Guard theme')).toBe(true)
    expect(claudeTaskResultIsAck('TaskUpdate', 'Updated task #1 status')).toBe(true)
    expect(claudeTaskResultIsAck('TaskUpdate', 'Task #1 not found')).toBe(false)
  })

  test('formatTaskId and pretty status', () => {
    expect(formatTaskId('1')).toBe('#1')
    expect(formatTaskId('#1')).toBe('#1')
    expect(prettyTaskStatus('in_progress')).toBe('in progress')
    expect(taskStatusChipClass('in_progress')).toBe('doing')
    expect(taskStatusChipClass('completed')).toBe('done')
    expect(taskStatusChipClass('deleted')).toBe('invalid')
    expect(taskStatusChipClass('pending')).toBe('pending')
  })

  test('TaskCreate with a description is expandable', () => {
    expect(claudeTaskHasBody('TaskCreate', createArgs)).toBe(true)
    expect(claudeTaskHasBody('TaskCreate', parseClaudeTaskArgs({ subject: 'Only a title' }))).toBe(false)
    expect(claudeTaskHasBody('TaskList', parseClaudeTaskArgs({}))).toBe(true)
    expect(claudeTaskHasBody('TaskGet', parseClaudeTaskArgs({ taskId: '1' }), { pending: true })).toBe(true)
    expect(claudeTaskHasBody('TaskGet', parseClaudeTaskArgs({ taskId: '1' }), {
      result: 'Task #1: Guard theme\nStatus: pending\nDescription: ',
    })).toBe(false)
    expect(claudeTaskHasBody('TaskGet', parseClaudeTaskArgs({ taskId: '1' }), {
      result: '• Task 1 — Guard theme (in progress)',
    })).toBe(true)
  })
})
