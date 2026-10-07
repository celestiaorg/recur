/** @vitest-environment happy-dom */
import { afterEach, beforeEach, describe, expect, test, vi } from 'vitest'
import {
  externalHrefFromAnchor,
  handleExternalLinkClick,
  isExternalHttpUrl,
  isWailsDesktop,
  openExternalUrl,
} from './open-external'

vi.mock('@wailsio/runtime', () => ({
  Browser: { OpenURL: vi.fn(() => Promise.resolve()) },
}))

const origin = 'http://localhost:3000'

describe('isExternalHttpUrl', () => {
  test('accepts any absolute http(s) URL, including this origin', () => {
    expect(isExternalHttpUrl('https://github.com/org/repo')).toBe(true)
    expect(isExternalHttpUrl('http://example.com/a?q=1')).toBe(true)
    expect(isExternalHttpUrl(`${origin}/foo`)).toBe(true)
  })

  test('rejects relative hrefs and non-http schemes', () => {
    expect(isExternalHttpUrl('/workspaces/x/screenshots/a.png')).toBe(false)
    expect(isExternalHttpUrl('javascript:alert(1)')).toBe(false)
    expect(isExternalHttpUrl('data:text/html,hi')).toBe(false)
    expect(isExternalHttpUrl('file:///etc/passwd')).toBe(false)
    expect(isExternalHttpUrl('')).toBe(false)
    expect(isExternalHttpUrl('not a url')).toBe(false)
  })
})

describe('externalHrefFromAnchor', () => {
  test('returns the absolute href for an http(s) <a>', () => {
    const a = document.createElement('a')
    a.setAttribute('href', 'https://example.com/path')
    a.setAttribute('target', '_blank')
    expect(externalHrefFromAnchor(a)).toBe('https://example.com/path')
  })

  test('opens same-origin absolute URLs (agent/terminal must not hijack the webview)', () => {
    const a = document.createElement('a')
    a.setAttribute('href', `${origin}/evil`)
    expect(externalHrefFromAnchor(a)).toBe(`${origin}/evil`)
  })

  test('ignores relative in-app hrefs', () => {
    const a = document.createElement('a')
    a.setAttribute('href', '/workspaces/id/screenshots/x.png')
    a.setAttribute('target', '_blank')
    expect(externalHrefFromAnchor(a)).toBeNull()
  })
})

describe('handleExternalLinkClick', () => {
  let open: ReturnType<typeof vi.spyOn>

  beforeEach(() => {
    open = vi.spyOn(window, 'open').mockReturnValue(null)
  })

  afterEach(() => {
    open.mockRestore()
    document.body.replaceChildren()
  })

  test('prevents default and opens https markdown links', () => {
    document.body.innerHTML = '<p class="chat-canvas"><a href="https://example.com/pr/1" target="_blank">PR</a></p>'
    const a = document.querySelector('a')!
    const e = new MouseEvent('click', { bubbles: true, cancelable: true, button: 0 })
    Object.defineProperty(e, 'target', { value: a })
    expect(handleExternalLinkClick(e)).toBe(true)
    expect(e.defaultPrevented).toBe(true)
    expect(open).toHaveBeenCalledWith('https://example.com/pr/1', '_blank', 'noopener,noreferrer')
  })

  test('opens when the click is on text inside the anchor', () => {
    document.body.innerHTML = '<a href="https://example.com"><span>label</span></a>'
    const span = document.querySelector('span')!
    const e = new MouseEvent('click', { bubbles: true, cancelable: true, button: 0 })
    Object.defineProperty(e, 'target', { value: span })
    expect(handleExternalLinkClick(e)).toBe(true)
    expect(open).toHaveBeenCalledWith('https://example.com/', '_blank', 'noopener,noreferrer')
  })

  test('leaves relative in-app screenshot links alone', () => {
    document.body.innerHTML = '<a href="/workspaces/x/screenshots/a.png" target="_blank">shot</a>'
    const a = document.querySelector('a')!
    const e = new MouseEvent('click', { bubbles: true, cancelable: true, button: 0 })
    Object.defineProperty(e, 'target', { value: a })
    expect(handleExternalLinkClick(e)).toBe(false)
    expect(e.defaultPrevented).toBe(false)
    expect(open).not.toHaveBeenCalled()
  })

  test('opens same-origin absolute http links instead of navigating the webview', () => {
    document.body.innerHTML = `<a href="${origin}/guide">local</a>`
    const a = document.querySelector('a')!
    const e = new MouseEvent('click', { bubbles: true, cancelable: true, button: 0 })
    Object.defineProperty(e, 'target', { value: a })
    expect(handleExternalLinkClick(e)).toBe(true)
    expect(e.defaultPrevented).toBe(true)
    expect(open).toHaveBeenCalledWith(`${origin}/guide`, '_blank', 'noopener,noreferrer')
  })

  test('leaves image clicks inside anchors for the lightbox', () => {
    document.body.innerHTML = '<a href="https://example.com"><img src="/x.png" alt="shot"></a>'
    const img = document.querySelector('img')!
    const e = new MouseEvent('click', { bubbles: true, cancelable: true, button: 0 })
    Object.defineProperty(e, 'target', { value: img })
    expect(handleExternalLinkClick(e)).toBe(false)
    expect(e.defaultPrevented).toBe(false)
    expect(open).not.toHaveBeenCalled()
  })

  test('ignores javascript: hrefs', () => {
    document.body.innerHTML = '<a href="javascript:alert(1)">x</a>'
    const a = document.querySelector('a')!
    const e = new MouseEvent('click', { bubbles: true, cancelable: true, button: 0 })
    Object.defineProperty(e, 'target', { value: a })
    expect(handleExternalLinkClick(e)).toBe(false)
    expect(open).not.toHaveBeenCalled()
  })
})

describe('openExternalUrl / isWailsDesktop', () => {
  test('happy-dom is not a Wails desktop webview', () => {
    expect(isWailsDesktop()).toBe(false)
  })

  test('no-ops on non-http URLs', async () => {
    const open = vi.spyOn(window, 'open').mockReturnValue(null)
    await openExternalUrl('javascript:alert(1)')
    expect(open).not.toHaveBeenCalled()
    open.mockRestore()
  })

  test('opens same-origin absolute http URLs', async () => {
    const open = vi.spyOn(window, 'open').mockReturnValue(null)
    await openExternalUrl(`${origin}/api`)
    expect(open).toHaveBeenCalledWith(`${origin}/api`, '_blank', 'noopener,noreferrer')
    open.mockRestore()
  })
})
