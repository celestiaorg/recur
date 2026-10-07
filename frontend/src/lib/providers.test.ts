import { describe, expect, test } from 'vitest'
import {
  PROVIDER_CLAUDE,
  PROVIDER_CODEX,
  PROVIDER_CURSOR,
  canDisableProvider,
  modelProvider,
  providerEnabled,
  providerOffersModels,
  chatSteerFollowUp,
} from './providers'

describe('providerEnabled', () => {
  test('defaults on', () => {
    expect(providerEnabled(undefined, PROVIDER_CLAUDE)).toBe(true)
    expect(providerEnabled([], PROVIDER_CURSOR)).toBe(true)
  })

  test('listed slugs are off', () => {
    expect(providerEnabled([PROVIDER_CURSOR], PROVIDER_CURSOR)).toBe(false)
    expect(providerEnabled([PROVIDER_CURSOR], PROVIDER_CLAUDE)).toBe(true)
  })
})

describe('modelProvider', () => {
  test('reads the slug, defaulting unqualified refs to Claude Code', () => {
    expect(modelProvider('cursor/composer-2.5')).toBe(PROVIDER_CURSOR)
    expect(modelProvider('sonnet')).toBe(PROVIDER_CLAUDE)
    expect(modelProvider('')).toBe(PROVIDER_CLAUDE)
  })
})

describe('chatSteerFollowUp', () => {
  test('injects text follow-ups while a pair-chat turn is running', () => {
    expect(chatSteerFollowUp(true, 'agent', false, 'PA')).toBe(true)
    expect(chatSteerFollowUp(true, 'ask', false, 'PA')).toBe(true)
    expect(chatSteerFollowUp(true, 'plan', false, 'PA2')).toBe(true)
  })

  test('still queues images, idle sends, MA, Loop, and a non-empty Recur queue', () => {
    expect(chatSteerFollowUp(true, 'agent', true, 'PA')).toBe(false)
    expect(chatSteerFollowUp(false, 'agent', false, 'PA')).toBe(false)
    expect(chatSteerFollowUp(true, 'loop', false, 'MA')).toBe(false)
    expect(chatSteerFollowUp(true, 'loop', false, 'PA')).toBe(false)
  })
})

describe('providerOffersModels', () => {
  test('Claude needs no Recur key', () => {
    expect(providerOffersModels(PROVIDER_CLAUDE, [], false, false)).toBe(true)
    expect(providerOffersModels(PROVIDER_CLAUDE, [PROVIDER_CLAUDE], false, false)).toBe(false)
  })

  test('Cursor/Codex need credentials and to be on', () => {
    expect(providerOffersModels(PROVIDER_CURSOR, [], false, false)).toBe(false)
    expect(providerOffersModels(PROVIDER_CURSOR, [], true, false)).toBe(true)
    expect(providerOffersModels(PROVIDER_CURSOR, [PROVIDER_CURSOR], true, false)).toBe(false)
    expect(providerOffersModels(PROVIDER_CODEX, [], false, true)).toBe(true)
  })

  test('compatible slugs need extraReady', () => {
    expect(providerOffersModels('openai', [], false, false)).toBe(false)
    expect(providerOffersModels('openai', [], false, false, ['openai'])).toBe(true)
    expect(providerOffersModels('openai', ['openai'], false, false, ['openai'])).toBe(false)
  })
})

describe('canDisableProvider', () => {
  test('cannot turn off the only catalogue provider', () => {
    expect(canDisableProvider(PROVIDER_CLAUDE, [], false, false)).toBe(false)
    expect(canDisableProvider(PROVIDER_CURSOR, [], false, false)).toBe(true)
  })

  test('Claude can go off once Cursor is ready', () => {
    expect(canDisableProvider(PROVIDER_CLAUDE, [], true, false)).toBe(true)
    expect(canDisableProvider(PROVIDER_CLAUDE, [], false, false)).toBe(false)
    expect(canDisableProvider(PROVIDER_CURSOR, [PROVIDER_CLAUDE], true, false)).toBe(false)
  })

  test('Claude can go off once a compatible provider is ready', () => {
    expect(canDisableProvider(PROVIDER_CLAUDE, [], false, false, ['openai'])).toBe(true)
    expect(canDisableProvider('openai', [], false, false, ['openai'])).toBe(true)
    expect(canDisableProvider('openai', [PROVIDER_CLAUDE], false, false, ['openai'])).toBe(false)
  })
})
