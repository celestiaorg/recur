// Frontend half of the terminal dock: session bookkeeping, the flow-control ack
// loop, and the xterm.js theme derived from the app's CSS variables.
//
// The Go side (internal/term) coalesces PTY output into ~8ms chunks and stops
// reading a session's PTY once too many bytes are unacked. This module holds up
// the other end of that contract — see ackWriter below.
import { Events } from '@wailsio/runtime'
import type { ITheme } from '@xterm/xterm'
import { get, writable } from 'svelte/store'
import { AckTerminal } from '../../bindings/recur/internal/app/terminalservice'
import { ListTerminalThemes } from '../../bindings/recur/internal/app/bundleservice'
import type { TerminalThemeInfo } from '../../bindings/recur/internal/app/models'
import { isLightTheme, theme } from './stores'

/** A decoded chunk of terminal output. */
export type TermData = { id: string; bytes: Uint8Array }

type DataHandler = (bytes: Uint8Array) => void
type ExitHandler = (exitCode: number) => void

const dataHandlers = new Map<string, DataHandler>()
const exitHandlers = new Map<string, ExitHandler>()
let wired = false

// base64 → bytes. The Go side base64s raw VT because the Wails bridge is JSON, and
// decoding to a Uint8Array (rather than a string) hands xterm.js bytes directly —
// its own UTF-8 decoder then handles multi-byte characters split across chunks,
// which a naive per-chunk atob+string would mangle into replacement characters.
export function decodeTermBytes(b64: string): Uint8Array {
  const bin = atob(b64)
  const out = new Uint8Array(bin.length)
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i)
  return out
}

/**
 * wireTerminalEvents subscribes once to the term.* event stream and fans chunks
 * out to whichever pane owns each session. Called from api.ts on boot.
 */
export function wireTerminalEvents() {
  if (wired) return
  wired = true
  Events.On('term.data', (e: any) => {
    const { id, data } = e.data ?? {}
    if (!id || typeof data !== 'string') return
    dataHandlers.get(id)?.(decodeTermBytes(data))
  })
  Events.On('term.exit', (e: any) => {
    const { id, exitCode } = e.data ?? {}
    if (!id) return
    exitHandlers.get(id)?.(typeof exitCode === 'number' ? exitCode : 0)
  })
}

/** Routes one session's output to a pane until unsubscribed. */
export function onTerminalData(id: string, fn: DataHandler): () => void {
  dataHandlers.set(id, fn)
  return () => { if (dataHandlers.get(id) === fn) dataHandlers.delete(id) }
}

/** Notifies when a session's shell exits. */
export function onTerminalExit(id: string, fn: ExitHandler): () => void {
  exitHandlers.set(id, fn)
  return () => { if (exitHandlers.get(id) === fn) exitHandlers.delete(id) }
}

// ACK_THRESHOLD batches acks so a fast stream doesn't fire a bound-method call per
// chunk. It must stay well under the Go side's 256KB high-water mark, or the PTY
// would stall waiting for an ack this side is still accumulating.
const ACK_THRESHOLD = 32 * 1024

/**
 * ackWriter returns a write function that acks bytes only once xterm.js has
 * actually parsed them.
 *
 * The callback is the load-bearing part: xterm.js parses asynchronously, so
 * acking at call time would measure how fast we can hand it data, not how fast it
 * can absorb it — which is exactly the runaway the flow control exists to stop.
 */
export function ackWriter(id: string, write: (bytes: Uint8Array, done: () => void) => void) {
  let unacked = 0
  let disposed = false
  return {
    push(bytes: Uint8Array) {
      if (disposed) return
      write(bytes, () => {
        if (disposed) return
        unacked += bytes.length
        if (unacked < ACK_THRESHOLD) return
        const n = unacked
        unacked = 0
        // A failed ack would silently wedge the session, but the only realistic
        // cause is the session already being gone — in which case there is nothing
        // left to unblock.
        AckTerminal(id, n).catch(() => {})
      })
    },
    dispose() { disposed = true },
  }
}

// ---- theme ----

function cssVar(name: string, fallback: string): string {
  const v = getComputedStyle(document.documentElement).getPropertyValue(name).trim()
  return v || fallback
}

/**
 * terminalTheme maps the active app theme's CSS variables onto xterm's palette, so
 * the terminal changes colour with the rest of the app instead of sitting in the
 * panel as a black rectangle.
 *
 * xterm takes a plain JS object rather than reading CSS, so this has to be
 * recomputed (and reassigned) whenever the theme changes.
 */
export function terminalTheme(opts?: { transparentBg?: boolean; palette?: ITheme }): ITheme {
  if (opts?.palette) {
    const opaque = opts.palette.background || cssVar('--bg', '#141414')
    return {
      ...opts.palette,
      background: opts.transparentBg ? '#00000000' : (opts.palette.background || opaque),
      cursorAccent: opts.palette.cursorAccent || opaque,
    }
  }
  // --bg, not --pane: the dock body is the editor-level surface, and xterm
  // paints its own cell background, so a mismatch here shows as a rectangle seam.
  // Maximize: cells are transparent so the dock's --term-bg mix (and leftover
  // fit pixels under the last row) is one wash, not a second bar at the bottom.
  const opaque = cssVar('--bg', '#141414')
  const bg = opts?.transparentBg ? '#00000000' : opaque
  const fg = cssVar('--text', '#e6e4e1')
  const red = cssVar('--coral', '#f87171')
  const green = cssVar('--emerald', '#4ade80')
  const yellow = cssVar('--warn', '#fbbf24')
  const blue = cssVar('--cyan', '#6eb6ff')
  const magenta = cssVar('--indigo', '#a78bfa')
  const cyan = cssVar('--cyan', '#6eb6ff')
  const dim = cssVar('--faint', '#686662')
  const muted = cssVar('--muted', '#969490')
  const light = isLightTheme(get(theme))

  return {
    background: bg,
    foreground: fg,
    cursor: cssVar('--amber', '#e07a5f'),
    cursorAccent: opaque,
    // A tinted block rather than a solid fill, so selected text stays readable.
    selectionBackground: light ? 'rgba(0,0,0,.12)' : 'rgba(255,255,255,.18)',
    black: cssVar('--line-bright', '#3d3d3d'),
    red, green, yellow, blue, magenta, cyan,
    white: muted,
    brightBlack: dim,
    brightRed: red,
    brightGreen: green,
    brightYellow: yellow,
    brightBlue: blue,
    brightMagenta: magenta,
    brightCyan: cyan,
    brightWhite: fg,
  }
}

export const terminalThemeCatalog = writable<TerminalThemeInfo[]>([])

let termThemeBoot: Promise<void> | null = null

/** Load [[terminalTheme]] palettes once. Safe to call from mount and Settings. `reload` re-reads bundles. */
export function ensureTerminalThemes(reload = false): Promise<void> {
  if (termThemeBoot && !reload) return termThemeBoot
  termThemeBoot = ListTerminalThemes()
    .then((list) => { terminalThemeCatalog.set(list ?? []) })
    .catch((err) => {
      termThemeBoot = null
      console.warn('ListTerminalThemes failed', err)
      terminalThemeCatalog.set([])
    })
  return termThemeBoot
}

function paletteFor(id: string): ITheme | undefined {
  if (!id) return
  const t = get(terminalThemeCatalog).find((x) => x.id === id)
  if (!t?.palette) return
  const out: ITheme = {}
  for (const [k, v] of Object.entries(t.palette)) {
    if (typeof v === 'string' && v) (out as Record<string, string>)[k] = v
  }
  return out.background && out.foreground ? out : undefined
}

/** Bundle palette when `id` is set; otherwise the app-chrome mapping. */
export function resolveTerminalTheme(id: string, opts?: { transparentBg?: boolean }): ITheme {
  return terminalTheme({ transparentBg: opts?.transparentBg, palette: paletteFor(id) })
}

/** Opaque cell fill for chrome around xterm (padding, frost tint). */
export function terminalSurfaceBg(id: string): string {
  return resolveTerminalTheme(id).background || cssVar('--bg', '#141414')
}

/** Maximize frost (transparent cells) only on dark palettes. */
export function terminalFrosts(id: string): boolean {
  if (!id) return !isLightTheme(get(theme))
  return get(terminalThemeCatalog).find((t) => t.id === id)?.uiTheme !== 'light'
}
