import { describe, expect, test } from 'vitest'
import {
  armedClick,
  armedPointerUp,
  createArmedConfirm,
  markClickEcho,
  resetArmedConfirm,
  takeClickEcho,
} from './armed-confirm'

function nextTask(): Promise<void> {
  return new Promise((r) => setTimeout(r, 0))
}

describe('armedConfirm', () => {
  test('first click arms, second confirms', () => {
    const g = createArmedConfirm()
    expect(armedClick(g)).toBe('arm')
    expect(g.armed).toBe(true)
    expect(armedClick(g)).toBe('confirm')
    expect(g.armed).toBe(false)
  })

  test('pointer-up while armed confirms and swallows the trailing click', () => {
    const g = createArmedConfirm()
    expect(armedClick(g)).toBe('arm')
    expect(armedPointerUp(g)).toBe(true)
    expect(g.armed).toBe(false)
    expect(armedClick(g)).toBe(null)
    expect(g.armed).toBe(false)
  })

  test('pointer-up does not arm', () => {
    const g = createArmedConfirm()
    expect(armedPointerUp(g)).toBe(false)
    expect(armedClick(g)).toBe('arm')
  })

  test('press-drag-out-release does not confirm', () => {
    const g = createArmedConfirm()
    armedClick(g)
    // Release off-control delivers neither pointerup nor click on the button.
    expect(g.armed).toBe(true)
    expect(g.eatClick).toBe(false)
  })

  test('non-primary button does not confirm', () => {
    const g = createArmedConfirm()
    armedClick(g)
    expect(armedPointerUp(g, 2)).toBe(false)
    expect(g.armed).toBe(true)
  })

  test('disabled ignores press and click', () => {
    const g = createArmedConfirm()
    armedClick(g)
    expect(armedPointerUp(g, 0, true)).toBe(false)
    expect(g.armed).toBe(true)
    expect(armedClick(g, true)).toBe(null)
    expect(g.armed).toBe(true)
  })

  test('does not confirm until inspect is ready', () => {
    const g = createArmedConfirm()
    expect(armedClick(g)).toBe('arm')
    expect(armedPointerUp(g, 0, false, false)).toBe(false)
    expect(g.armed).toBe(true)
    expect(armedClick(g, false, false)).toBe(null)
    expect(g.armed).toBe(true)
    expect(armedClick(g, false, true)).toBe('confirm')
  })

  test('echo does not block the next control after the button is gone', async () => {
    const g = createArmedConfirm()
    armedClick(g)
    expect(armedPointerUp(g)).toBe(true)
    await nextTask()
    expect(armedClick(g)).toBe('arm')
  })

  test('reset clears a pending confirm click', () => {
    const g = createArmedConfirm()
    armedClick(g)
    armedPointerUp(g)
    resetArmedConfirm(g)
    expect(armedClick(g)).toBe('arm')
  })

  test('takeClickEcho is scoped to one gesture', async () => {
    const g = createArmedConfirm()
    markClickEcho(g)
    expect(takeClickEcho(g)).toBe(true)
    expect(takeClickEcho(g)).toBe(false)
    markClickEcho(g)
    await nextTask()
    expect(takeClickEcho(g)).toBe(false)
  })
})
