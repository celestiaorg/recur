// The capture picker. Runs only in WebKit's default client world, so the page cannot see
// `__recurCapture` or `webkit.messageHandlers.recurCapture`. Click picks an element, drag picks
// an area, Esc cancels; pointer, mouse and click events are swallowed while picking.
(() => {
  if (window.__recurCapture) return
  const HTML_MAX = 4000
  const TEXT_MAX = 2000
  const post = (msg) => window.webkit.messageHandlers.recurCapture.postMessage(JSON.stringify(msg))

  // Shortest `>` chain of tag / :nth-of-type steps (anchored at the nearest id) that matches
  // only el.
  const cssSelector = (el) => {
    const doc = el.ownerDocument
    const parts = []
    for (let n = el; n && n !== doc.documentElement; n = n.parentElement) {
      if (n.id) {
        parts.unshift(`#${CSS.escape(n.id)}`)
      } else {
        let part = CSS.escape(n.localName)
        const p = n.parentElement
        if (p) {
          const same = Array.from(p.children).filter((c) => c.localName === n.localName)
          if (same.length > 1) part += `:nth-of-type(${same.indexOf(n) + 1})`
        }
        parts.unshift(part)
      }
      const sel = parts.join(' > ')
      try {
        if (doc.querySelectorAll(sel).length === 1) return sel
      } catch {}
    }
    return parts.join(' > ')
  }

  const truncate = (html, max) => {
    if (html.length <= max) return html
    let cut = html.slice(0, max)
    const lt = cut.lastIndexOf('<')
    if (lt > cut.lastIndexOf('>')) cut = cut.slice(0, lt)
    return `${cut}…`
  }

  let host = null
  let box
  let label
  let down = null
  let dragging = false
  const swallow = ['pointerdown', 'pointerup', 'mousedown', 'mouseup', 'mousemove', 'click', 'dblclick', 'auxclick', 'contextmenu']

  const draw = (r, text) => {
    box.style.cssText = `position:fixed;left:${r.left}px;top:${r.top}px;width:${r.width}px;height:${r.height}px;` +
      'border:2px solid #f2b33d;background:rgba(242,179,61,.12);box-sizing:border-box;border-radius:2px'
    label.textContent = text
    label.style.cssText = `position:fixed;left:${r.left}px;top:${Math.max(0, r.top - 20)}px;` +
      'font:11px/18px -apple-system,system-ui,sans-serif;padding:0 5px;background:#f2b33d;color:#1a1a1a;border-radius:3px;white-space:nowrap'
  }
  const at = (x, y) => document.elementFromPoint(x, y)
  const dragRect = (e) => {
    const x = Math.min(down.x, e.clientX)
    const y = Math.min(down.y, e.clientY)
    return { x, y, width: Math.abs(e.clientX - down.x), height: Math.abs(e.clientY - down.y) }
  }

  const onMove = (e) => {
    e.preventDefault()
    e.stopImmediatePropagation()
    if (down && !dragging && Math.hypot(e.clientX - down.x, e.clientY - down.y) > 4) dragging = true
    if (dragging) {
      const r = dragRect(e)
      draw({ left: r.x, top: r.y, width: r.width, height: r.height }, `${Math.round(r.width)}×${Math.round(r.height)}`)
      return
    }
    const el = at(e.clientX, e.clientY)
    if (el) draw(el.getBoundingClientRect(), el.localName)
  }
  const onDown = (e) => {
    e.preventDefault()
    e.stopImmediatePropagation()
    if (e.button === 0) down = { x: e.clientX, y: e.clientY }
  }
  const onUp = (e) => {
    e.preventDefault()
    e.stopImmediatePropagation()
    if (!down || e.button !== 0) return
    let rect = null
    let el
    if (dragging) {
      rect = dragRect(e)
      el = at(rect.x + rect.width / 2, rect.y + rect.height / 2)
    } else {
      el = at(e.clientX, e.clientY)
      const b = el && el.getBoundingClientRect()
      if (b) rect = { x: b.left, y: b.top, width: b.width, height: b.height }
    }
    const mode = dragging ? 'area' : 'element'
    stop()
    if (!el || !rect) return post({ kind: 'recur.capture.cancel' })
    post({
      kind: 'recur.capture',
      mode,
      rect,
      viewport: { width: innerWidth, height: innerHeight },
      tag: el.localName,
      selector: cssSelector(el),
      text: (el.innerText ?? el.textContent ?? '').replace(/\s+/g, ' ').trim().slice(0, TEXT_MAX),
      html: truncate(el.outerHTML, HTML_MAX),
    })
  }
  const onKey = (e) => {
    e.preventDefault()
    e.stopImmediatePropagation()
    if (e.key === 'Escape') {
      stop()
      post({ kind: 'recur.capture.cancel' })
    }
  }
  const block = (e) => {
    e.preventDefault()
    e.stopImmediatePropagation()
  }
  const listen = (on) => {
    const f = on ? addEventListener : removeEventListener
    f('pointermove', onMove, true)
    f('pointerdown', onDown, true)
    f('pointerup', onUp, true)
    f('keydown', onKey, true)
    f('keyup', block, true)
    for (const t of swallow) f(t, block, true)
  }

  function start() {
    if (host) return
    host = document.createElement('div')
    host.style.cssText = 'all:initial;position:fixed;inset:0;z-index:2147483647;pointer-events:none'
    const root = host.attachShadow({ mode: 'closed' })
    box = document.createElement('div')
    label = document.createElement('div')
    root.append(box, label)
    document.documentElement.append(host)
    down = null
    dragging = false
    listen(true)
  }
  function stop() {
    if (!host) return
    host.remove()
    host = null
    // The click that follows this pointerup still needs swallowing.
    setTimeout(() => host || listen(false), 0)
  }
  window.__recurCapture = { start, stop }
})()
