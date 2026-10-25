/** @vitest-environment happy-dom */
import { afterEach, describe, expect, test, vi } from 'vitest'
import { isTitlebarZoomTarget } from './titlebar'

function mount(html: string): HTMLElement {
  const root = document.createElement('div')
  root.className = 'titlebar'
  root.innerHTML = html
  document.body.appendChild(root)
  return root
}

afterEach(() => {
  document.body.replaceChildren()
  document.head.replaceChildren()
  vi.restoreAllMocks()
})

describe('isTitlebarZoomTarget', () => {
  test('empty chrome and crumbs zoom; traffic-light inset does not', () => {
    const bar = mount(`<div class="tb-traffic-clear"></div><span class="crumb">/ inbox</span>`)
    expect(isTitlebarZoomTarget(bar)).toBe(true)
    expect(isTitlebarZoomTarget(bar.querySelector('.tb-traffic-clear'))).toBe(false)
    expect(isTitlebarZoomTarget(bar.querySelector('.crumb'))).toBe(true)
  })

  test('Linux custom-chrome menu and window controls do not zoom', () => {
    const bar = mount(`
      <div class="tb-menu-wrap"><div class="merge-menu app-menu"><div class="app-menu-sep"></div></div></div>
      <div class="tb-winctl"><span class="tb-winbtn-icon"></span></div>
    `)
    expect(isTitlebarZoomTarget(bar.querySelector('.app-menu-sep'))).toBe(false)
    expect(isTitlebarZoomTarget(bar.querySelector('.tb-winbtn-icon'))).toBe(false)
  })

  test('buttons, no-drag crumbs, and open menus do not zoom', () => {
    const bar = mount(`
      <button type="button" class="tb-rail">Rail</button>
      <span class="crumb-arrow">→</span>
      <div class="crumb-base">
        <div class="sel">
          <button type="button" class="sel-btn">main</button>
          <div class="sel-menu"><div class="sel-search"></div></div>
        </div>
      </div>
      <div class="merge-menu" role="menu"><button type="button" role="menuitem">Merge</button></div>
      <span class="tb-instance"><span class="tb-instance-k">dev</span>mycodecrafting/smoky-shortcake</span>
    `)
    expect(isTitlebarZoomTarget(bar.querySelector('.tb-rail'))).toBe(false)
    expect(isTitlebarZoomTarget(bar.querySelector('.crumb-arrow'))).toBe(false)
    expect(isTitlebarZoomTarget(bar.querySelector('.crumb-base'))).toBe(false)
    expect(isTitlebarZoomTarget(bar.querySelector('.sel-menu'))).toBe(false)
    expect(isTitlebarZoomTarget(bar.querySelector('.sel-search'))).toBe(false)
    expect(isTitlebarZoomTarget(bar.querySelector('[role="menuitem"]'))).toBe(false)
    expect(isTitlebarZoomTarget(bar.querySelector('[role="menu"]'))).toBe(false)
    expect(isTitlebarZoomTarget(bar.querySelector('.tb-instance'))).toBe(false)
  })

  test('clicks below the titlebar band do not zoom', () => {
    const bar = mount(`<span class="crumb">/ inbox</span>`)
    vi.spyOn(bar, 'getBoundingClientRect').mockReturnValue({
      x: 0, y: 0, top: 0, left: 0, right: 800, bottom: 52, width: 800, height: 52, toJSON() { return {} },
    })
    const crumb = bar.querySelector('.crumb')
    expect(isTitlebarZoomTarget(crumb, 20)).toBe(true)
    expect(isTitlebarZoomTarget(crumb, 52)).toBe(false)
    expect(isTitlebarZoomTarget(crumb, 80)).toBe(false)
  })

  test('computed -webkit-app-region:no-drag excludes classes absent from the selector list', () => {
    const sheet = document.createElement('style')
    sheet.textContent = '.tb-custom{-webkit-app-region:no-drag}'
    document.head.appendChild(sheet)
    const bar = mount(`<span class="tb-custom">extra</span><span class="crumb">/ inbox</span>`)
    const custom = bar.querySelector('.tb-custom')
    expect(custom).toBeTruthy()
    expect(getComputedStyle(custom!).getPropertyValue('-webkit-app-region').trim()).toBe('no-drag')
    expect(isTitlebarZoomTarget(custom)).toBe(false)
    expect(isTitlebarZoomTarget(bar.querySelector('.crumb'))).toBe(true)
  })

  test('ignores non-elements', () => {
    expect(isTitlebarZoomTarget(null)).toBe(false)
    expect(isTitlebarZoomTarget(document.createTextNode('x'))).toBe(false)
  })
})
