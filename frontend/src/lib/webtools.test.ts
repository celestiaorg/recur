import { describe, expect, it } from 'vitest'
import { compactUrl, webFetchPrompt, webFetchUrl, webResultIsJson, webSearchQuery } from './webtools'

const coinbase =
  'https://docs.cdp.coinbase.com/api-reference/rest-api/onramp-offramp/create-session-token'

describe('webFetchUrl', () => {
  it('reads url from args and ignores the extraction prompt', () => {
    expect(webFetchUrl({ url: coinbase, prompt: 'List the fields. Is clientIp required?' })).toBe(coinbase)
  })

  it('falls back to a bare URL summary, not truncated JSON', () => {
    expect(webFetchUrl(null, coinbase)).toBe(coinbase)
    expect(webFetchUrl(undefined, `{"url":"${coinbase}"}`)).toBe(coinbase)
    expect(webFetchUrl({}, '')).toBe('')
  })
})

describe('webFetchPrompt', () => {
  it('returns the prompt string or empty', () => {
    expect(webFetchPrompt({ url: coinbase, prompt: '  Is clientIp required?  ' })).toBe('Is clientIp required?')
    expect(webFetchPrompt({ url: coinbase })).toBe('')
    expect(webFetchPrompt(null, '{"prompt":"from summary"}')).toBe('from summary')
    expect(webFetchPrompt(null)).toBe('')
  })
})

describe('webSearchQuery', () => {
  it('prefers query, then search_term, then a non-JSON fallback', () => {
    expect(webSearchQuery({ query: 'onramp session token' })).toBe('onramp session token')
    expect(webSearchQuery({ search_term: 'clientIp required' })).toBe('clientIp required')
    expect(webSearchQuery(null, 'bare query')).toBe('bare query')
    expect(webSearchQuery(null, '{"query":"x"}')).toBe('x')
  })
})

describe('compactUrl', () => {
  it('drops the scheme and collapses a long path to host/…/last', () => {
    expect(compactUrl(coinbase)).toBe('docs.cdp.coinbase.com/…/create-session-token')
  })

  it('keeps short paths and a bare host', () => {
    expect(compactUrl('https://example.com/')).toBe('example.com')
    expect(compactUrl('https://www.example.com/docs/fetch')).toBe('example.com/docs/fetch')
    expect(compactUrl('https://example.com/a?q=1#top')).toBe('example.com/a?q=1#top')
  })

  it('passes through unparseable input', () => {
    expect(compactUrl('not a url')).toBe('not a url')
    expect(compactUrl('')).toBe('')
  })
})

describe('webResultIsJson', () => {
  it('accepts a JSON object or array', () => {
    expect(webResultIsJson('{"error":"not found"}')).toBe(true)
    expect(webResultIsJson('  [{"title":"a"}]  ')).toBe(true)
  })

  it('does not treat badge-prefixed or brace-prefixed markdown as JSON', () => {
    expect(webResultIsJson('[![CI](https://img.shields.io/badge.svg)](https://github.com/org/repo)\n# README')).toBe(false)
    expect(webResultIsJson('{docs}\nA page about JSON Schema.')).toBe(false)
    expect(webResultIsJson('')).toBe(false)
    expect(webResultIsJson('# heading')).toBe(false)
  })
})
