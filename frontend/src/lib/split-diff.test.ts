import { describe, expect, it } from 'vitest'
import { windowAround } from './review-window'
import { slotOfSplitHighlight, slotOfUnifiedHighlight, splitDiffRows, splitRowDataLine, type SplitDiffLine } from './split-diff'

function L(type: string, old = '', neu = '', html = type): SplitDiffLine {
  return { type, old, new: neu, codeHtml: html }
}

describe('splitDiffRows', () => {
  it('keeps hunk headers and context on both sides', () => {
    const rows = splitDiffRows([
      L('hunk', '', '', '@@'),
      L('ctx', '1', '1', 'same'),
    ])
    expect(rows).toEqual([
      { kind: 'hunk', i: 0, line: L('hunk', '', '', '@@') },
      { kind: 'pair', i: 1, left: { i: 1, line: L('ctx', '1', '1', 'same') }, right: { i: 1, line: L('ctx', '1', '1', 'same') } },
    ])
  })

  it('zips a delete with the following add', () => {
    const rows = splitDiffRows([
      L('del', '4', '', 'old'),
      L('add', '', '4', 'new'),
    ])
    expect(rows).toHaveLength(1)
    expect(rows[0]).toMatchObject({
      kind: 'pair',
      left: { i: 0, line: { type: 'del', codeHtml: 'old' } },
      right: { i: 1, line: { type: 'add', codeHtml: 'new' } },
    })
  })

  it('pads the shorter side when dels and adds differ in count', () => {
    const rows = splitDiffRows([
      L('del', '1', '', 'a'),
      L('del', '2', '', 'b'),
      L('add', '', '1', 'c'),
    ])
    expect(rows).toHaveLength(2)
    expect(rows[0]).toMatchObject({ left: { i: 0 }, right: { i: 2 } })
    expect(rows[1]).toMatchObject({ left: { i: 1 }, right: null })
  })
})

describe('slotOfSplitHighlight', () => {
  function replacementSlots(n = 400) {
    const lines: SplitDiffLine[] = [L('hunk', '', '', `@@ -701,${n} +501,${n} @@`)]
    for (let i = 0; i < n; i++) {
      lines.push(L('del', String(701 + i), '', 'old'))
      lines.push(L('add', '', String(501 + i), 'new'))
    }
    return splitDiffRows(lines)
  }

  it('mounts the data-line row when old and new numbers diverge', () => {
    const slots = replacementSlots()
    expect(splitRowDataLine(slots[100])).toBe(600)
    expect(splitRowDataLine(slots[300])).toBe(800)

    const slot = slotOfSplitHighlight(slots, 800)
    expect(slot).toBe(300)
    const win = windowAround(slot, slots.length)
    expect(slot).toBeGreaterThanOrEqual(win.start)
    expect(slot).toBeLessThan(win.end)
    expect(100).toBeLessThan(win.start)
  })

  it('mounts a LEFT thread when old and new numbers diverge', () => {
    const slots = replacementSlots()
    expect(splitRowDataLine(slots[150])).toBe(650)
    expect(slotOfSplitHighlight(slots, 850)).toBe(350)

    const slot = slotOfSplitHighlight(slots, 850, [{ line: 850, side: 'LEFT' }])
    expect(slot).toBe(150)
    const win = windowAround(slot, slots.length)
    expect(slot).toBeGreaterThanOrEqual(win.start)
    expect(slot).toBeLessThan(win.end)
    expect(350).toBeGreaterThanOrEqual(win.end)
  })
})

describe('slotOfUnifiedHighlight', () => {
  function replacementLines(n = 200) {
    const lines: SplitDiffLine[] = [L('hunk', '', '', `@@ -1,${n} +1,${n} @@`)]
    for (let i = 1; i <= n; i++) lines.push(L('del', String(i), '', 'old'))
    for (let i = 1; i <= n; i++) lines.push(L('add', '', String(i), 'new'))
    return lines
  }

  it('mounts a RIGHT thread when old and new numbers overlap', () => {
    const lines = replacementLines()
    const slots = lines.map((_, i) => i)
    expect(slotOfUnifiedHighlight(lines, slots, 150)).toBe(150)

    const slot = slotOfUnifiedHighlight(lines, slots, 150, [{ line: 150, side: 'RIGHT' }])
    expect(slot).toBe(350)
    const win = windowAround(slot, slots.length)
    expect(slot).toBeGreaterThanOrEqual(win.start)
    expect(slot).toBeLessThan(win.end)
    expect(150).toBeLessThan(win.start)
  })

  it('mounts a LEFT thread when old and new numbers overlap', () => {
    const lines = replacementLines()
    const slots = lines.map((_, i) => i)
    const slot = slotOfUnifiedHighlight(lines, slots, 150, [{ line: 150, side: 'LEFT' }])
    expect(slot).toBe(150)
    const win = windowAround(slot, slots.length)
    expect(slot).toBeGreaterThanOrEqual(win.start)
    expect(slot).toBeLessThan(win.end)
    expect(350).toBeGreaterThanOrEqual(win.end)
  })
})
