import { derived, writable } from 'svelte/store'

/**
 * Named UI layers that cover the chat surface (D-attention). Components that
 * mount a full-screen dialog should call setChatOccluder so a finish under them
 * still raises unseen / dock — same as New workspace / OS window blur.
 *
 * `agent-pane` is attention-only: the chat is off screen (collapsed dock / other
 * panel) but it is not a full-screen overlay, so in-page menus and workspace
 * cycling must keep working.
 */
const occluders = writable<ReadonlySet<string>>(new Set())

/** Collapsed / off-screen chat — still occludes attention, not a full-screen overlay. */
const ATTENTION_ONLY_IDS = new Set(['agent-pane'])

/** Register or clear an occluding overlay by stable id (e.g. "quick-open"). */
export function setChatOccluder(id: string, active: boolean) {
  occluders.update((prev) => {
    const has = prev.has(id)
    if (active === has) return prev
    const next = new Set(prev)
    if (active) next.add(id)
    else next.delete(id)
    return next
  })
}

/** True when any registered occluder is active — including a collapsed agent pane. */
export const chatUIOccluded = derived(occluders, (s) => s.size > 0)

/** Full-screen overlays only (lightbox, Quick Open, …). A collapsed chat pane is not one. */
export const chatOverlayOpen = derived(occluders, (s) => {
  for (const id of s) {
    if (!ATTENTION_ONLY_IDS.has(id)) return true
  }
  return false
})
