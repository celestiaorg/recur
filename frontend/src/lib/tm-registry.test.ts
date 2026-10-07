import { beforeEach, describe, expect, it } from 'vitest'
import {
  _resetTmRegistryForTests,
  applyColorMap,
  getColorMap,
  getColorMapEpoch,
  onColorMapChange,
} from './tm-registry'

beforeEach(() => _resetTmRegistryForTests())

describe('applyColorMap', () => {
  it('re-applying an identical map keeps the epoch and subscribers quiet', () => {
    applyColorMap(['', '#ccc', '#111'])
    const epoch = getColorMapEpoch()
    let notified = 0
    const off = onColorMapChange(() => notified++)
    // Same theme re-applied on a tab switch must not repaint open editors.
    applyColorMap(['', '#ccc', '#111'])
    expect(getColorMapEpoch()).toBe(epoch)
    expect(notified).toBe(0)
    off()
  })

  it('a changed map bumps the epoch and notifies subscribers', () => {
    applyColorMap(['', '#ccc'])
    const epoch = getColorMapEpoch()
    let notified = 0
    const off = onColorMapChange(() => notified++)
    applyColorMap(['', '#ddd'])
    expect(getColorMapEpoch()).toBe(epoch + 1)
    expect(notified).toBe(1)
    expect(getColorMap()).toEqual(['', '#ddd'])
    off()
  })
})
