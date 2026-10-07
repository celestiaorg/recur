/** @vitest-environment happy-dom */
import { afterEach, describe, expect, test, vi } from 'vitest'
import { ignoreLaunchChromeFocus } from './chrome-focus'

function mountChrome(cls: 'rail' | 'titlebar', html: string) {
  const root = document.createElement('div')
  root.className = cls
  root.innerHTML = html
  document.body.appendChild(root)
  return root
}

function mountRail(html = '<button type="button" aria-label="Add project">+</button>') {
  return mountChrome('rail', html)
}

let stop: (() => void) | undefined

afterEach(() => {
  stop?.()
  stop = undefined
  document.body.replaceChildren()
  vi.useRealTimers()
})

describe('ignoreLaunchChromeFocus', () => {
  test('blurs a sidebar button that the webview focused at launch', () => {
    const root = mountRail()
    const btn = root.querySelector('button')!
    stop = ignoreLaunchChromeFocus({ armMs: 60_000 })
    btn.focus()
    expect(document.activeElement).not.toBe(btn)
  })

  test('blurs a titlebar button the same way', () => {
    const root = mountChrome('titlebar', '<button type="button" class="tb-rail">Toggle</button>')
    const btn = root.querySelector('button')!
    stop = ignoreLaunchChromeFocus({ armMs: 60_000 })
    btn.focus()
    expect(document.activeElement).not.toBe(btn)
  })

  test('blurs chrome that was already focused before the guard installed', async () => {
    const root = mountRail()
    const btn = root.querySelector('button')!
    btn.focus()
    expect(document.activeElement).toBe(btn)
    stop = ignoreLaunchChromeFocus({ armMs: 60_000 })
    await Promise.resolve()
    expect(document.activeElement).not.toBe(btn)
  })

  test('keeps chrome focus after Tab so keyboard users still see the ring', () => {
    const root = mountRail()
    const btn = root.querySelector('button')!
    stop = ignoreLaunchChromeFocus({ armMs: 60_000 })
    window.dispatchEvent(new KeyboardEvent('keydown', { key: 'Tab', bubbles: true }))
    btn.focus()
    expect(document.activeElement).toBe(btn)
  })

  test('disarms on any keydown, not only Tab', () => {
    const root = mountRail()
    const btn = root.querySelector('button')!
    stop = ignoreLaunchChromeFocus({ armMs: 60_000 })
    window.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowDown', bubbles: true }))
    btn.focus()
    expect(document.activeElement).toBe(btn)
  })

  test('disarms on pointerdown so a sidebar click keeps focus', () => {
    const root = mountRail()
    const btn = root.querySelector('button')!
    stop = ignoreLaunchChromeFocus({ armMs: 60_000 })
    window.dispatchEvent(new Event('pointerdown', { bubbles: true }))
    btn.focus()
    expect(document.activeElement).toBe(btn)
  })

  test('disarms after the activation window even with no input', () => {
    vi.useFakeTimers()
    const root = mountRail()
    const btn = root.querySelector('button')!
    stop = ignoreLaunchChromeFocus({ armMs: 500 })
    vi.advanceTimersByTime(500)
    btn.focus()
    expect(document.activeElement).toBe(btn)
  })

  test('window focus re-arms after the boot timer so a late first keying is covered', () => {
    vi.useFakeTimers()
    const root = mountRail()
    const btn = root.querySelector('button')!
    stop = ignoreLaunchChromeFocus({ armMs: 500 })
    vi.advanceTimersByTime(500)
    btn.focus()
    expect(document.activeElement).toBe(btn)
    btn.blur()
    window.dispatchEvent(new Event('focus'))
    btn.focus()
    expect(document.activeElement).not.toBe(btn)
  })

  test('re-arms when the window is backgrounded so Cmd-Tab does not restore the ring', () => {
    const root = mountRail()
    const btn = root.querySelector('button')!
    stop = ignoreLaunchChromeFocus({ armMs: 60_000 })
    window.dispatchEvent(new KeyboardEvent('keydown', { key: 'Tab', bubbles: true }))
    btn.focus()
    expect(document.activeElement).toBe(btn)
    btn.blur()
    window.dispatchEvent(new Event('blur'))
    btn.focus()
    expect(document.activeElement).not.toBe(btn)
  })

  test('keeps the chrome control that was focused when the window was backgrounded', () => {
    const root = mountRail()
    const btn = root.querySelector('button')!
    stop = ignoreLaunchChromeFocus({ armMs: 60_000 })
    window.dispatchEvent(new KeyboardEvent('keydown', { key: 'Tab', bubbles: true }))
    btn.focus()
    expect(document.activeElement).toBe(btn)
    window.dispatchEvent(new Event('blur'))
    window.dispatchEvent(new Event('focus'))
    btn.focus()
    expect(document.activeElement).toBe(btn)
  })

  test('still blurs a different chrome control after the window is keyed', () => {
    const root = mountRail(
      '<button type="button" aria-label="Settings">S</button>'
      + '<button type="button" aria-label="Add project">+</button>',
    )
    const settings = root.querySelector('[aria-label="Settings"]') as HTMLButtonElement
    const add = root.querySelector('[aria-label="Add project"]') as HTMLButtonElement
    stop = ignoreLaunchChromeFocus({ armMs: 60_000 })
    window.dispatchEvent(new KeyboardEvent('keydown', { key: 'Tab', bubbles: true }))
    settings.focus()
    expect(document.activeElement).toBe(settings)
    window.dispatchEvent(new Event('blur'))
    window.dispatchEvent(new Event('focus'))
    add.focus()
    expect(document.activeElement).not.toBe(add)
  })

  test('does not steal focus from a rail text field', () => {
    const root = mountRail('<input class="gname-input" aria-label="Group name" />')
    const input = root.querySelector('input')!
    stop = ignoreLaunchChromeFocus({ armMs: 60_000 })
    input.focus()
    expect(document.activeElement).toBe(input)
  })

  test('leaves main-content focus alone', () => {
    const main = document.createElement('main')
    main.innerHTML = '<button type="button">Create project</button>'
    document.body.appendChild(main)
    const btn = main.querySelector('button')!
    stop = ignoreLaunchChromeFocus({ armMs: 60_000 })
    btn.focus()
    expect(document.activeElement).toBe(btn)
  })

  test('disposer stops the guard and cancels a pending rAF sweep', async () => {
    const root = mountRail()
    const btn = root.querySelector('button')!
    stop = ignoreLaunchChromeFocus({ armMs: 60_000 })
    stop()
    stop = undefined
    btn.focus()
    expect(document.activeElement).toBe(btn)
    await new Promise<void>((resolve) => requestAnimationFrame(() => resolve()))
    expect(document.activeElement).toBe(btn)
  })
})
