/** @vitest-environment happy-dom */
import { describe, expect, it } from 'vitest'
import { pickDiffJumpEl } from './review-scroll'

describe('pickDiffJumpEl', () => {
  it('prefers the inline thread over the diff row', () => {
    const root = document.createElement('div')
    root.innerHTML = `
      <div class="hline" data-line="12"></div>
      <div class="icmt-thread" data-note-line="12"></div>
    `
    expect(pickDiffJumpEl(root, 12)?.className).toBe('icmt-thread')
  })

  it('falls back to the highlighted row', () => {
    const root = document.createElement('div')
    root.innerHTML = `<div class="hline" data-line="8"></div>`
    expect(pickDiffJumpEl(root, 8)?.getAttribute('data-line')).toBe('8')
  })
})
