import { describe, expect, it } from 'vitest'
import { get } from 'svelte/store'
import { setChatOccluder, chatUIOccluded, chatOverlayOpen } from './chat-occlusion'

describe('chat occlusion', () => {
  it('treats a collapsed agent pane as attention-only, not a full-screen overlay', () => {
    setChatOccluder('agent-pane', true)
    expect(get(chatUIOccluded)).toBe(true)
    expect(get(chatOverlayOpen)).toBe(false)

    setChatOccluder('quick-open', true)
    expect(get(chatUIOccluded)).toBe(true)
    expect(get(chatOverlayOpen)).toBe(true)

    setChatOccluder('quick-open', false)
    expect(get(chatOverlayOpen)).toBe(false)
    expect(get(chatUIOccluded)).toBe(true)

    setChatOccluder('agent-pane', false)
    expect(get(chatUIOccluded)).toBe(false)
    expect(get(chatOverlayOpen)).toBe(false)
  })
})
