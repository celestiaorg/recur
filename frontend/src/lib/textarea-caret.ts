/**
 * Viewport rect of the caret in a textarea/input.
 * Mirror-div technique (same idea as textarea-caret-position).
 */

const COPY_STYLES = [
  'direction', 'boxSizing', 'width', 'height',
  'overflowX', 'overflowY',
  'borderTopWidth', 'borderRightWidth', 'borderBottomWidth', 'borderLeftWidth',
  'borderTopStyle', 'borderRightStyle', 'borderBottomStyle', 'borderLeftStyle',
  'paddingTop', 'paddingRight', 'paddingBottom', 'paddingLeft',
  'fontStyle', 'fontVariant', 'fontWeight', 'fontStretch', 'fontSize',
  'fontSizeAdjust', 'lineHeight', 'fontFamily',
  'textAlign', 'textTransform', 'textIndent', 'textDecoration',
  'letterSpacing', 'wordSpacing', 'tabSize', 'MozTabSize',
] as const

export function textareaCaretRect(
  el: HTMLTextAreaElement | HTMLInputElement,
  position = el.selectionStart ?? 0,
): { top: number; bottom: number; left: number; height: number } {
  const style = getComputedStyle(el)
  const mirror = document.createElement('div')
  const isInput = el instanceof HTMLInputElement
  mirror.setAttribute('aria-hidden', 'true')
  mirror.style.position = 'absolute'
  mirror.style.visibility = 'hidden'
  mirror.style.overflow = 'hidden'
  mirror.style.whiteSpace = isInput ? 'pre' : 'pre-wrap'
  mirror.style.wordWrap = isInput ? 'normal' : 'break-word'
  mirror.style.top = '0'
  mirror.style.left = '-9999px'
  for (const prop of COPY_STYLES) {
    const value = style[prop as keyof CSSStyleDeclaration]
    if (typeof value === 'string') {
      ;(mirror.style as unknown as Record<string, string>)[prop] = value
    }
  }

  mirror.textContent = el.value.slice(0, Math.max(0, position))
  if (isInput) mirror.textContent = (mirror.textContent ?? '').replace(/\s/g, '\u00a0')

  const marker = document.createElement('span')
  marker.textContent = '\u200b'
  mirror.append(marker)
  document.body.append(mirror)
  try {
    const elRect = el.getBoundingClientRect()
    const borderTop = parseFloat(style.borderTopWidth) || 0
    const borderLeft = parseFloat(style.borderLeftWidth) || 0
    const lineH = marker.offsetHeight || parseFloat(style.lineHeight) || 16
    const top = elRect.top - el.scrollTop + marker.offsetTop + borderTop
    const left = elRect.left - el.scrollLeft + marker.offsetLeft + borderLeft
    return { top, bottom: top + lineH, left, height: lineH }
  } finally {
    mirror.remove()
  }
}
