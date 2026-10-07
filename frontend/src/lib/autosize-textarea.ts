/** Grow a textarea with its text, then scroll — same idea as the chat composer. */

export type AutosizeOpts = {
  minPx: number
  maxPx: number
}

/** Main PR conversation composer: a bit taller than inline note replies. */
export const PR_COMPOSER_AUTOSIZE: AutosizeOpts = { minPx: 72, maxPx: 160 }

/** Line-note reply: compact, grow through ~6 lines. */
export const PR_REPLY_AUTOSIZE: AutosizeOpts = { minPx: 40, maxPx: 120 }

export function fitTextareaHeight(el: HTMLTextAreaElement, opts: AutosizeOpts) {
  el.style.height = `${opts.minPx}px`
  const content = el.scrollHeight
  const next = Math.min(Math.max(content, opts.minPx), opts.maxPx)
  el.style.height = `${next}px`
  el.style.overflowY = content > opts.maxPx ? 'auto' : 'hidden'
}

/** `{@attach}` factory — pass the bound value so height updates on each edit. */
export function autosizeTextarea(value: string, opts: AutosizeOpts) {
  return (el: HTMLElement) => {
    if (!(el instanceof HTMLTextAreaElement)) return
    void value
    fitTextareaHeight(el, opts)
  }
}
