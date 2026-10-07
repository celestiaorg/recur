import { describe, expect, test } from 'vitest'
import {
  isSubagentTool,
  isSendMessageTool,
  parseSubagentArgs,
  parseSendMessageArgs,
  isSubagentLaunchAck,
  subagentMeta,
  sendMessageLabel,
  sendMessageMeta,
} from './subagent'

const launchAck = `Async agent launched successfully. (This tool result is internal metadata – never quote or paste any part of it to the user.)
agentId: ad643cfdedf8d9846
Use SendMessage with to: 'ad643cfdedf8d9846', summary: '<5-10 word recap>' to continue this agent.
You know nothing about its results until that notification arrives – do not report, assume, or predict them.
output_file: /private/tmp/claude-501/tasks/ad643cfdedf8d9846.output
Do NOT Read or tail this file via the shell tool – it is the full subagent JSONL transcript and reading it will overflow your context.`

describe('isSubagentTool', () => {
  test('Task and Agent, not TaskCreate', () => {
    expect(isSubagentTool('Task')).toBe(true)
    expect(isSubagentTool('Agent')).toBe(true)
    expect(isSubagentTool('TaskCreate')).toBe(false)
    expect(isSubagentTool('SendMessage')).toBe(false)
  })
})

describe('parseSubagentArgs', () => {
  test('reads type, prompt, background — not a JSON dump', () => {
    const a = parseSubagentArgs({
      description: 'Research VS Code tokenization pipeline',
      subagent_type: 'Explore',
      run_in_background: true,
      prompt: 'Find tokenizeViewport and forceTokenization.',
    })
    expect(a.subagentType).toBe('Explore')
    expect(a.background).toBe(true)
    expect(a.prompt).toContain('tokenizeViewport')
    expect(subagentMeta(a.subagentType, a.model)).toBe('Explore')
    expect(subagentMeta(a.subagentType, a.model)).not.toContain('{')
    expect(subagentMeta(a.subagentType, a.model)).not.toContain('description')
  })
})

describe('isSubagentLaunchAck', () => {
  test('hides the model-only launch metadata', () => {
    expect(isSubagentLaunchAck(launchAck)).toBe(true)
    expect(isSubagentLaunchAck('The tokenizer lives in tokenizationTextModelPart.ts.')).toBe(false)
    expect(isSubagentLaunchAck('')).toBe(false)
    expect(isSubagentLaunchAck(
      'see Async agent launched successfully plus agentId and internal metadata in comments',
    )).toBe(false)
  })
})

describe('SendMessage', () => {
  test('label uses the short description; meta never leaks agent ids', () => {
    expect(isSendMessageTool('SendMessage')).toBe(true)
    const a = parseSendMessageArgs({
      to: 'ad643cfdedf8d9846',
      description: 'Ask about tokenizeViewport',
      prompt: 'Where is forceTokenization called?',
    })
    expect(sendMessageLabel(false, a.description)).toBe('Sent Ask about tokenizeViewport')
    expect(sendMessageMeta(a.description, a.message)).toBe('')
    expect(sendMessageMeta('', a.message)).toBe('Where is forceTokenization called?')
    expect(sendMessageMeta('', a.message)).not.toContain('ad643cf')
  })
})
