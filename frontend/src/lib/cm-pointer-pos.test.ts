/** @vitest-environment happy-dom */
import { describe, expect, it } from 'vitest'
import { EditorState } from '@codemirror/state'
import { EditorView } from '@codemirror/view'
import {
  caretFromPoint,
  nearestPaintedLine,
  paintedLineAtPoint,
  pickClosestLine,
  pointerMouseSelection,
  pointerMoved,
  selectionForClick,
} from './cm-pointer-pos'

describe('caretFromPoint', () => {
  it('returns null when the document has no caret hit-testing APIs', () => {
    expect(caretFromPoint(0, 0)).toBeNull()
  })
})

describe('pointerMoved', () => {
  const origin = { clientX: 40, clientY: 80 }

  it('treats an unmoved click as still', () => {
    expect(pointerMoved(origin, { clientX: 40, clientY: 80 })).toBe(false)
  })

  it('ignores sub-slop jitter from a viewport jump under the cursor', () => {
    expect(pointerMoved(origin, { clientX: 42, clientY: 83 })).toBe(false)
  })

  it('counts a real drag', () => {
    expect(pointerMoved(origin, { clientX: 40, clientY: 120 })).toBe(true)
  })
})

describe('pickClosestLine', () => {
  const lines = [
    { item: 'a', top: 0, bottom: 20 },
    { item: 'b', top: 20, bottom: 40 },
    { item: 'c', top: 40, bottom: 60 },
  ]

  it('hits the line that contains y', () => {
    expect(pickClosestLine(lines, 25)).toBe('b')
  })

  it('picks the nearest line when y sits in a gap', () => {
    expect(pickClosestLine([
      { item: 'a', top: 0, bottom: 20 },
      { item: 'c', top: 80, bottom: 100 },
    ], 45)).toBe('a')
  })
})

describe('paintedLineAtPoint', () => {
  it('falls back to the nearest .cm-line by box when caret APIs miss', () => {
    const content = document.createElement('div')
    const a = document.createElement('div')
    a.className = 'cm-line'
    a.getBoundingClientRect = () => ({
      top: 10, bottom: 30, left: 0, right: 100, width: 100, height: 20, x: 0, y: 10, toJSON() { return {} },
    })
    const b = document.createElement('div')
    b.className = 'cm-line'
    b.getBoundingClientRect = () => ({
      top: 200, bottom: 220, left: 0, right: 100, width: 100, height: 20, x: 0, y: 200, toJSON() { return {} },
    })
    content.append(a, b)
    expect(nearestPaintedLine(content, 205)).toBe(b)
    expect(paintedLineAtPoint(content, 10, 205)).toBe(b)
  })
})

describe('selectionForClick', () => {
  const state = EditorState.create({ doc: 'hello world\nsecond' })

  it('places a collapsed caret on a single click', () => {
    const sel = selectionForClick(state, 3, 1, 1)
    expect(sel.empty).toBe(true)
    expect(sel.head).toBe(3)
  })

  it('selects the word on a double click', () => {
    const sel = selectionForClick(state, 1, 1, 2)
    expect(sel.from).toBe(0)
    expect(sel.to).toBe(5)
  })

  it('selects the line on a triple click', () => {
    const sel = selectionForClick(state, 1, 1, 3)
    expect(sel.from).toBe(0)
    expect(sel.to).toBe(12)
  })
})

describe('pointerMouseSelection', () => {
  it('installs without throwing on a live EditorView', () => {
    const parent = document.createElement('div')
    document.body.appendChild(parent)
    const view = new EditorView({
      parent,
      state: EditorState.create({
        doc: 'alpha\nbeta\ngamma',
        extensions: [pointerMouseSelection()],
      }),
    })
    expect(view.scrollDOM).toBeTruthy()
    view.destroy()
    parent.remove()
  })

  it('unfreezes scroll after the focus-settling frames even without a mouseup', async () => {
    const parent = document.createElement('div')
    document.body.appendChild(parent)
    const view = new EditorView({
      parent,
      state: EditorState.create({
        doc: 'alpha\nbeta\ngamma',
        extensions: [pointerMouseSelection()],
      }),
    })
    view.contentDOM.blur()
    view.contentDOM.dispatchEvent(new MouseEvent('mousedown', {
      button: 0, bubbles: true, cancelable: true,
    }))
    await new Promise<void>((resolve) => {
      requestAnimationFrame(() => requestAnimationFrame(() => resolve()))
    })
    view.scrollDOM.scrollTop = 40
    await new Promise<void>((resolve) => requestAnimationFrame(() => resolve()))
    expect(view.scrollDOM.scrollTop).toBe(40)
    view.destroy()
    parent.remove()
  })
})
