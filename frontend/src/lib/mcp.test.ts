import { describe, expect, test } from 'vitest'
import {
  LINEAR_MCP_ID,
  customMCPServers,
  linearMCPOn,
  mcpGlobalDisabled,
  slugMCPID,
  updateMCPServer,
  upsertLinearEnabled,
  validMCPID,
} from './mcp'

describe('validMCPID', () => {
  test('rejects reserved and invalid ids', () => {
    expect(validMCPID('recur')).toBe(false)
    expect(validMCPID('playwright')).toBe(false)
    expect(validMCPID('Linear')).toBe(false)
    expect(validMCPID('')).toBe(false)
  })

  test('accepts linear and custom slugs', () => {
    expect(validMCPID('linear')).toBe(true)
    expect(validMCPID('acme-jira')).toBe(true)
  })
})

describe('mcpGlobalDisabled', () => {
  test('Linear is off until enabled', () => {
    expect(mcpGlobalDisabled(undefined)).toEqual([LINEAR_MCP_ID])
    expect(linearMCPOn(undefined)).toBe(false)
  })

  test('enabled Linear is not on the off list', () => {
    expect(mcpGlobalDisabled([{ id: LINEAR_MCP_ID, enabled: true }])).toEqual([])
    expect(linearMCPOn([{ id: LINEAR_MCP_ID, enabled: true }])).toBe(true)
  })
})

describe('upsertLinearEnabled', () => {
  test('stores Linear only while enabled', () => {
    const on = upsertLinearEnabled([{ id: 'acme', enabled: true, url: 'https://x' }], true)
    expect(on[0]).toMatchObject({ id: LINEAR_MCP_ID, enabled: true, preset: LINEAR_MCP_ID })
    expect(customMCPServers(on).map((s) => s.id)).toEqual(['acme'])
    expect(upsertLinearEnabled(on, false).map((s) => s.id)).toEqual(['acme'])
  })
})

describe('slugMCPID', () => {
  test('slugifies a display name', () => {
    expect(slugMCPID('Acme Jira')).toBe('acme-jira')
  })
})

describe('updateMCPServer', () => {
  test('keeps the id while replacing url and auth', () => {
    const got = updateMCPServer(
      [{ id: 'acme', name: 'Acme', enabled: true, url: 'https://old', headers: { Authorization: 'Bearer old' } }],
      'acme',
      { name: 'Acme API', url: 'https://new', headers: { Authorization: 'Bearer new' } },
    )
    expect(got).toEqual([{
      id: 'acme',
      name: 'Acme API',
      enabled: true,
      url: 'https://new',
      headers: { Authorization: 'Bearer new' },
    }])
  })
})
