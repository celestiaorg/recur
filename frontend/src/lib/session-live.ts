// Live vs settled specialist cards in the Loop cockpit.
//
// Stream events (delta/step/turn_end) can still land after agent.updated idle
// (a leftover publish after the turn unwound). #patch used to treat every
// stream event as "still running", which re-lit the elapsed timer while the
// sidebar (agentsRunning) stayed idle. Switching workspace re-attaches the
// feed and hid it.
//
// Latch settled only on idle. turn_end is per provider run, and runCoder can
// emit several inside one working window — clearing live without latching lets
// the next nudge's deltas re-light. After idle, touchFromStream stays dark.

export class SessionLiveFlags {
  #live = new Set<string>()
  #settled = new Set<string>()

  snapshot(): Set<string> {
    return new Set(this.#live)
  }

  reset() {
    this.#live.clear()
    this.#settled.clear()
  }

  /** Engine says this specialist is mid-turn (agent.updated / agentsRunning). */
  markRunning(code: string): boolean {
    if (!code) return false
    this.#settled.delete(code)
    if (this.#live.has(code)) return false
    this.#live.add(code)
    return true
  }

  /** agent.updated idle — the one terminal signal. Late stream events stay dark. */
  markSettled(code: string): boolean {
    if (!code) return false
    this.#settled.add(code)
    if (!this.#live.has(code)) return false
    this.#live.delete(code)
    return true
  }

  /** turn_end of one provider run. Does not latch — the next run may still be in the same working window. */
  clearLive(code: string): boolean {
    if (!code || !this.#live.has(code)) return false
    this.#live.delete(code)
    return true
  }

  /** Optimistic live from a stream event before agent.updated working. */
  touchFromStream(code: string): boolean {
    if (!code || this.#settled.has(code) || this.#live.has(code)) return false
    this.#live.add(code)
    return true
  }
}
