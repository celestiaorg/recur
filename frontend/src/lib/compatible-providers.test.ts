import { describe, expect, test } from 'vitest'
import { compatibleSlugError, persistableFlags, persistableHeaders, persistableModels, unusedKey } from './compatible-providers'

describe('persistableModels', () => {
  test('drops empty ids so Add model drafts are not saved', () => {
    expect(persistableModels([
      { id: 'qwen/qwen3-coder-next', label: 'Qwen' },
      { id: '  ', label: 'x' },
      { id: '', label: '' },
    ])).toEqual([{ id: 'qwen/qwen3-coder-next', label: 'Qwen' }])
  })
})

describe('compatibleSlugError', () => {
  test('rejects reserved and malformed slugs', () => {
    expect(compatibleSlugError('claude-code')).toContain('reserved')
    expect(compatibleSlugError('Foo_Bar')).toContain('kebab-case')
    expect(compatibleSlugError('openai', ['openai'])).toContain('already exists')
    expect(compatibleSlugError('openrouter')).toBe('')
  })
})

describe('persistableFlags', () => {
  test('drops empty ids and option values', () => {
    expect(persistableFlags([
      { id: 'effort', label: 'Effort', wire: 'reasoning.effort', options: [{ value: 'high', label: 'High' }, { value: '', label: 'x' }] },
      { id: '', label: 'nope' },
    ])).toEqual([{ id: 'effort', label: 'Effort', wire: 'reasoning.effort', options: [{ value: 'high', label: 'High' }] }])
  })
})

describe('unusedKey', () => {
  test('repeated add option keeps unique values', () => {
    const values: string[] = []
    values.push(unusedKey(values, 'value'))
    values.push(unusedKey(values, 'value'))
    expect(values).toEqual(['value', 'value2'])
  })

  test('remove-then-add dimension does not collide with a live id', () => {
    let ids = ['effort', 'flag2', 'flag3']
    ids = ids.filter((id) => id !== 'flag2')
    ids = [...ids, unusedKey(ids, 'flag')]
    expect(ids).toEqual(['effort', 'flag3', 'flag'])
  })
})

describe('persistableHeaders', () => {
  test('drops empty keys so Add header drafts are not saved', () => {
    expect(persistableHeaders([
      { key: 'X-Title', value: 'recur' },
      { key: '  ', value: 'x' },
      { key: '', value: '' },
    ])).toEqual({ 'X-Title': 'recur' })
  })

  test('empty list clears extra headers', () => {
    expect(persistableHeaders([])).toBeUndefined()
  })
})
