/** Rows/cols that fit in a terminal host's content box.
 *
 *  @xterm/addon-fit reads the host's CSS height (border box) and only subtracts
 *  padding on the xterm element itself. Recur's inset lives on `.term-pane`, so
 *  fit() sizes one overflow of padding too tall; the dock's overflow:hidden then
 *  clips the last row. Always measure the content box. */

export const TERM_FIT_MIN_COLS = 2
export const TERM_FIT_MIN_ROWS = 1
/** Matches @xterm/addon-fit / ViewportConstants.DEFAULT_SCROLL_BAR_WIDTH. */
export const TERM_FIT_SCROLLBAR_WIDTH = 14

export function termHostInnerSize(host: HTMLElement): { width: number; height: number } {
  const cs = getComputedStyle(host)
  return {
    width: host.clientWidth - (parseFloat(cs.paddingLeft) || 0) - (parseFloat(cs.paddingRight) || 0),
    height: host.clientHeight - (parseFloat(cs.paddingTop) || 0) - (parseFloat(cs.paddingBottom) || 0),
  }
}

export function termFitSize(
  innerWidth: number,
  innerHeight: number,
  cellWidth: number,
  cellHeight: number,
  scrollbarWidth = TERM_FIT_SCROLLBAR_WIDTH,
): { cols: number; rows: number } {
  if (!(cellWidth > 0) || !(cellHeight > 0)) {
    return { cols: TERM_FIT_MIN_COLS, rows: TERM_FIT_MIN_ROWS }
  }
  return {
    cols: Math.max(TERM_FIT_MIN_COLS, Math.floor((innerWidth - scrollbarWidth) / cellWidth)),
    rows: Math.max(TERM_FIT_MIN_ROWS, Math.floor(innerHeight / cellHeight)),
  }
}
