import { describe, expect, it } from 'vitest'
import { ComposerUndo, changeEnd } from './composerUndo'

type A = { id: string }
const none: A[] = []

function typeText(u: ComposerUndo<A>, start: string, add: string, t0: number, gap = 50) {
  let text = start
  let t = t0
  for (const ch of add) {
    text += ch
    u.record({ text, atts: none, caret: text.length }, t)
    t += gap
  }
  return t
}

describe('ComposerUndo', () => {
  it('merges continuous typing and splits on a pause', () => {
    const u = new ComposerUndo<A>(700)
    u.reset({ text: '', atts: none, caret: 0 })
    const t = typeText(u, '', 'hello', 0)
    typeText(u, 'hello', ' world', t + 1000)
    expect(u.undo()).toEqual({ text: 'hello', atts: none, caret: 5 })
    expect(u.undo()).toEqual({ text: '', atts: none, caret: 0 })
    expect(u.undo()).toBeNull()
    expect(u.redo()?.text).toBe('hello')
    expect(u.redo()?.text).toBe('hello world')
    expect(u.redo()).toBeNull()
  })

  it('splits when switching between typing and deleting', () => {
    const u = new ComposerUndo<A>(700)
    u.reset({ text: '', atts: none, caret: 0 })
    typeText(u, '', 'abc', 0)
    u.record({ text: 'ab', atts: none, caret: 2 }, 200)
    expect(u.undo()?.text).toBe('abc')
    expect(u.undo()?.text).toBe('')
  })

  it('keeps pastes, chips, and caret jumps as their own steps', () => {
    const u = new ComposerUndo<A>(700)
    u.reset({ text: '', atts: none, caret: 0 })
    typeText(u, '', 'ab', 0)
    u.record({ text: 'ab pasted', atts: none, caret: 9 }, 120)
    u.record({ text: 'ab pastedx', atts: none, caret: 10 }, 140)
    u.record({ text: 'yab pastedx', atts: none, caret: 1 }, 160)
    const img = [{ id: 'img1' }]
    u.record({ text: 'yab pastedx', atts: img, caret: 11 }, 180)
    expect(u.undo()).toEqual({ text: 'yab pastedx', atts: none, caret: 11 })
    expect(u.undo()).toEqual({ text: 'ab pastedx', atts: none, caret: 0 })
    expect(u.undo()?.text).toBe('ab pasted')
    expect(u.undo()?.text).toBe('ab')
  })

  it('splits at a newline and survives a same-content reset', () => {
    const u = new ComposerUndo<A>(700)
    u.reset({ text: '', atts: none, caret: 0 })
    let t = typeText(u, '', 'ab', 0)
    u.reset({ text: 'ab', atts: none, caret: 2 })
    t = typeText(u, 'ab', '\ncd', t)
    expect(u.undo()?.text).toBe('ab\n')
    expect(u.undo()?.text).toBe('ab')
    expect(u.undo()?.text).toBe('')
    u.reset({ text: 'other', atts: none, caret: 5 })
    expect(u.undo()).toBeNull()
  })

  it('forced reset drops history even when content matches', () => {
    const u = new ComposerUndo<A>(700)
    u.reset({ text: '', atts: none, caret: 0 })
    typeText(u, '', 'hi', 0)
    u.record({ text: '', atts: none, caret: 0 }, 5000)
    u.reset({ text: '', atts: none, caret: 0 }, true)
    expect(u.undo()).toBeNull()
    expect(u.redo()).toBeNull()
  })

  it('drops redo after a new edit', () => {
    const u = new ComposerUndo<A>(700)
    u.reset({ text: '', atts: none, caret: 0 })
    typeText(u, '', 'ab', 0)
    u.undo()
    u.record({ text: 'z', atts: none, caret: 1 }, 5000)
    expect(u.redo()).toBeNull()
    expect(u.undo()?.text).toBe('')
  })
})

describe('changeEnd', () => {
  it('puts the caret after the changed region', () => {
    expect(changeEnd('hello world', 'hello')).toBe(5)
    expect(changeEnd('hello', 'hello world')).toBe(11)
    expect(changeEnd('ab', 'aXb')).toBe(2)
    expect(changeEnd('aXb', 'ab')).toBe(1)
  })
})
