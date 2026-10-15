import { afterAll } from 'vitest'

// @wailsio/runtime's drag.js starts a 50ms window.setInterval on import under a DOM.
// If a happy-dom file finishes before its first tick, the tick fires after teardown
// and throws "window is not defined". Its first tick clears the interval, so wait it out.
afterAll(async () => {
  if (typeof window !== 'undefined' && '_wails' in window) await new Promise((r) => setTimeout(r, 60))
})
