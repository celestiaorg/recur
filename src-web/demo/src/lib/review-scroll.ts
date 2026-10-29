/** Scroll a review diff pane so `el` sits in the upper third (thread visible below). */
export function paneScrollTo(pane: HTMLElement, el: HTMLElement, bias = 0.28): void {
  const pr = pane.getBoundingClientRect()
  const er = el.getBoundingClientRect()
  const top = pane.scrollTop + (er.top - pr.top) - pane.clientHeight * bias
  pane.scrollTo({ top: Math.max(0, top), behavior: 'instant' })
}

/** Prefer the inline thread, then the highlighted diff row. */
export function pickDiffJumpEl(root: ParentNode, line: number): HTMLElement | null {
  if (line > 0) {
    const thread = root.querySelector<HTMLElement>(`.icmt-thread[data-note-line="${line}"]`)
    if (thread) return thread
    const row = root.querySelector<HTMLElement>(`[data-line="${line}"]`)
    if (row) return row
  }
  return root instanceof HTMLElement ? root : null
}
