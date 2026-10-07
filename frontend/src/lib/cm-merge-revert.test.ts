/** @vitest-environment happy-dom */
import { describe, expect, it } from 'vitest'
import { rejectChunk, unifiedMergeView } from '@codemirror/merge'
import { EditorState } from '@codemirror/state'
import { EditorView } from '@codemirror/view'
import { lineOrientedDiff } from './cm-line-diff'
import {
  chunkAtPos, hunkButtonLeft, mergeRevertHover, mergeRevertHoverPlugin,
  revertAimChunk, revertHunkCommand, REVERT_HUNK_KEY_DISPLAY,
} from './cm-merge-revert'

describe('mergeRevertHover', () => {
  function mount() {
    const parent = document.createElement('div')
    document.body.appendChild(parent)
    const view = new EditorView({
      parent,
      state: EditorState.create({
        doc: 'keep\nNEW\ntail\n',
        extensions: [
          unifiedMergeView({
            original: 'keep\ntail\n',
            mergeControls: false,
            gutter: true,
            highlightChanges: true,
            diffConfig: { override: lineOrientedDiff },
          }),
          mergeRevertHover(),
        ],
      }),
    })
    return { view, parent }
  }

  it('keeps Revert hidden when the caret is outside a hunk', () => {
    const { view, parent } = mount()
    const btn = parent.querySelector('.cm-revertHunk') as HTMLButtonElement | null
    view.destroy()
    parent.remove()
    expect(btn).toBeTruthy()
    expect(btn!.hidden).toBe(true)
    expect(btn!.querySelector('.cm-revertHunk-label')?.textContent).toBe('Revert')
    expect(btn!.querySelector('.cm-revertHunk-key')?.textContent).toBe(REVERT_HUNK_KEY_DISPLAY)
    expect(REVERT_HUNK_KEY_DISPLAY).toBe('⌥⌘Z')
  })

  it('shows Revert when the caret is inside a hunk', () => {
    const { view, parent } = mount()
    view.dispatch({ selection: { anchor: view.state.doc.line(2).from } })
    const btn = parent.querySelector('.cm-revertHunk') as HTMLButtonElement
    const shown = !btn.hidden
    view.dispatch({ selection: { anchor: 0 } })
    const hiddenAgain = btn.hidden
    view.destroy()
    parent.remove()
    expect(shown).toBe(true)
    expect(hiddenAgain).toBe(true)
  })

  it('moves the chip to the caret hunk when the pointer is parked on another', () => {
    const parent = document.createElement('div')
    document.body.appendChild(parent)
    const view = new EditorView({
      parent,
      state: EditorState.create({
        doc: 'keep\nA\nmid\nB\ntail\n',
        extensions: [
          unifiedMergeView({
            original: 'keep\nmid\ntail\n',
            mergeControls: false,
            gutter: true,
            highlightChanges: true,
            diffConfig: { override: lineOrientedDiff },
          }),
          mergeRevertHover(),
        ],
      }),
    })
    const plug = view.plugin(mergeRevertHoverPlugin)!
    const a = view.state.doc.line(2).from
    const b = view.state.doc.line(4).from
    plug.hoverFromB = a
    plug.sync(true)
    const hovered = plug.fromB
    view.dispatch({ selection: { anchor: b } })
    const afterKeys = { hover: plug.hoverFromB, fromB: plug.fromB }
    view.destroy()
    parent.remove()
    expect(hovered).toBe(a)
    expect(afterKeys.hover).toBe(-1)
    expect(afterKeys.fromB).toBe(b)
  })

  it('does not install CM accept/reject chips', () => {
    const { view, parent } = mount()
    const chips = parent.querySelector('.cm-chunkButtons')
    view.destroy()
    parent.remove()
    expect(chips).toBeNull()
  })

  it('maps the inserted line to the changed chunk', () => {
    const { view, parent } = mount()
    const at = view.state.doc.line(2).from
    const ch = chunkAtPos(view.state, at)
    view.destroy()
    parent.remove()
    expect(ch).toBeTruthy()
    expect(ch!.fromB).toBe(at)
  })

  it('rejectChunk restores the hunk to the original document', () => {
    const { view, parent } = mount()
    const ok = rejectChunk(view, view.state.doc.line(2).from)
    const next = view.state.doc.toString()
    view.destroy()
    parent.remove()
    expect(ok).toBe(true)
    expect(next).toBe('keep\ntail\n')
  })

  it('revertHunkCommand restores the hunk at the cursor', () => {
    const { view, parent } = mount()
    view.dispatch({ selection: { anchor: view.state.doc.line(2).from } })
    const ok = revertHunkCommand(view)
    const next = view.state.doc.toString()
    view.destroy()
    parent.remove()
    expect(ok).toBe(true)
    expect(next).toBe('keep\ntail\n')
  })

  it('revertHunkCommand restores the hunk the chip is showing, not the caret', () => {
    const parent = document.createElement('div')
    document.body.appendChild(parent)
    const view = new EditorView({
      parent,
      state: EditorState.create({
        doc: 'keep\nA\nmid\nB\ntail\n',
        extensions: [
          unifiedMergeView({
            original: 'keep\nmid\ntail\n',
            mergeControls: false,
            gutter: true,
            highlightChanges: true,
            diffConfig: { override: lineOrientedDiff },
          }),
          mergeRevertHover(),
        ],
      }),
    })
    view.dispatch({ selection: { anchor: view.state.doc.line(4).from } })
    const plug = view.plugin(mergeRevertHoverPlugin)!
    const a = view.state.doc.line(2).from
    plug.hoverFromB = a
    plug.sync(true)
    const ok = revertHunkCommand(view)
    const next = view.state.doc.toString()
    view.destroy()
    parent.remove()
    expect(ok).toBe(true)
    expect(next).toBe('keep\nmid\nB\ntail\n')
  })

  it('stays hidden on a read-only mount even when the caret is in a hunk', () => {
    const parent = document.createElement('div')
    document.body.appendChild(parent)
    const view = new EditorView({
      parent,
      state: EditorState.create({
        doc: 'keep\nNEW\ntail\n',
        selection: { anchor: 5 },
        extensions: [
          EditorState.readOnly.of(true),
          unifiedMergeView({
            original: 'keep\ntail\n',
            mergeControls: false,
            gutter: true,
            highlightChanges: true,
            diffConfig: { override: lineOrientedDiff },
          }),
          mergeRevertHover(),
        ],
      }),
    })
    const btn = parent.querySelector('.cm-revertHunk') as HTMLButtonElement
    const plug = view.plugin(mergeRevertHoverPlugin)!
    const hidden = btn.hidden
    const fromB = plug.fromB
    view.destroy()
    parent.remove()
    expect(hidden).toBe(true)
    expect(fromB).toBe(-1)
  })

  it('does not throw when posAtCoords reports a missing tile', () => {
    const { view, parent } = mount()
    view.posAtCoords = () => { throw new Error('No tile at position 1251') }
    expect(() => {
      view.scrollDOM.dispatchEvent(new PointerEvent('pointermove', {
        clientX: 12, clientY: 24, bubbles: true,
      }))
    }).not.toThrow()
    view.destroy()
    parent.remove()
  })

  it('aims at a hunk from a painted line when posAtCoords cannot see tiles', () => {
    const { view, parent } = mount()
    view.posAtCoords = () => { throw new Error('No tile at position 1251') }
    const hunkLine = [...view.contentDOM.querySelectorAll('.cm-line')]
      .find(el => el.textContent === 'NEW')
    expect(hunkLine).toBeTruthy()
    const want = view.state.doc.line(2).from
    hunkLine!.dispatchEvent(new PointerEvent('pointermove', {
      clientX: 12, clientY: 24, bubbles: true,
    }))
    const aimed = view.plugin(mergeRevertHoverPlugin)!.hoverFromB
    view.destroy()
    parent.remove()
    expect(aimed).toBe(want)
  })

  it('activates on click, not mousedown', () => {
    const { view, parent } = mount()
    view.dispatch({ selection: { anchor: view.state.doc.line(2).from } })
    const btn = parent.querySelector('.cm-revertHunk') as HTMLButtonElement
    btn.hidden = false
    btn.dispatchEvent(new MouseEvent('mousedown', { bubbles: true }))
    expect(view.state.doc.toString()).toBe('keep\nNEW\ntail\n')
    btn.dispatchEvent(new MouseEvent('click', { bubbles: true }))
    const next = view.state.doc.toString()
    view.destroy()
    parent.remove()
    expect(next).toBe('keep\ntail\n')
  })
})

describe('revertAimChunk', () => {
  function twoHunks() {
    return EditorState.create({
      doc: 'keep\nA\nmid\nB\ntail\n',
      extensions: [
        unifiedMergeView({
          original: 'keep\nmid\ntail\n',
          mergeControls: false,
          gutter: true,
          highlightChanges: true,
          diffConfig: { override: lineOrientedDiff },
        }),
      ],
    })
  }

  it('prefers a live hover over a caret in a different hunk', () => {
    const raw = twoHunks()
    const state = raw.update({ selection: { anchor: raw.doc.line(4).from } }).state
    const hoverAt = state.doc.line(2).from
    expect(revertAimChunk(state, hoverAt)?.fromB).toBe(hoverAt)
  })

  it('falls back to the caret hunk once hover is cleared', () => {
    const raw = twoHunks()
    const state = raw.update({ selection: { anchor: raw.doc.line(4).from } }).state
    expect(revertAimChunk(state, -1)?.fromB).toBe(state.doc.line(4).from)
  })
})

describe('hunkButtonLeft', () => {
  it('pins to the visible scroller when content is wider than the pane', () => {
    // contentRight 2000, scrolled to 0, pane 400, chip 56, inset 8 → 400-56-8
    expect(hunkButtonLeft(2000, 0, 400, 56, 8)).toBe(336)
  })

  it('follows horizontal scroll', () => {
    expect(hunkButtonLeft(2000, 300, 400, 56, 8)).toBe(636)
  })

  it('stays on the content edge when the file is narrower than the pane', () => {
    expect(hunkButtonLeft(280, 0, 400, 56, 8)).toBe(216)
  })
})
