/** @vitest-environment happy-dom */
import { describe, expect, test } from 'vitest'
import { textareaCaretRect } from './textarea-caret'

describe('textareaCaretRect', () => {
  test('returns a finite viewport rect inside the field', () => {
    const el = document.createElement('textarea')
    el.value = 'hello :roc'
    el.style.width = '240px'
    el.style.height = '80px'
    el.style.font = '14px sans-serif'
    document.body.append(el)
    el.setSelectionRange(el.value.length, el.value.length)
    const r = textareaCaretRect(el)
    const box = el.getBoundingClientRect()
    expect(Number.isFinite(r.top)).toBe(true)
    expect(Number.isFinite(r.left)).toBe(true)
    expect(r.height).toBeGreaterThan(0)
    expect(r.left).toBeGreaterThanOrEqual(box.left - 1)
    el.remove()
  })

  test('later caret is further right on the same line', () => {
    const el = document.createElement('textarea')
    el.value = 'aaaaaaaaaa'
    el.style.width = '400px'
    el.style.height = '80px'
    el.style.font = '14px monospace'
    document.body.append(el)
    const at0 = textareaCaretRect(el, 0)
    const atEnd = textareaCaretRect(el, el.value.length)
    expect(atEnd.left).toBeGreaterThanOrEqual(at0.left)
    el.remove()
  })
})
