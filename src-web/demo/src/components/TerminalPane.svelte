<script lang="ts" module>
  import { SearchAddon, type ISearchOptions } from '@xterm/addon-search'
  import { PREVIEW_FIND_CAP } from '../lib/preview-find'

  const searchByTerm = new WeakMap<object, SearchAddon>()
  const findCbByTerm = new WeakMap<object, (index: number, total: number, valid: boolean) => void>()

  function cssHex(name: string, fallback: string): string {
    const v = getComputedStyle(document.documentElement).getPropertyValue(name).trim()
    return /^#[0-9A-Fa-f]{6}$/.test(v) ? v : fallback
  }

  function mixHex(fg: string, bg: string, a: number): string {
    const ch = (h: string, i: number) => parseInt(h.slice(i, i + 2), 16)
    const n = (x: number) => Math.round(x).toString(16).padStart(2, '0')
    return `#${n(ch(fg, 1) * a + ch(bg, 1) * (1 - a))}${n(ch(fg, 3) * a + ch(bg, 3) * (1 - a))}${n(ch(fg, 5) * a + ch(bg, 5) * (1 - a))}`
  }

  function searchDecorations(): NonNullable<ISearchOptions['decorations']> {
    const amber = cssHex('--amber', '#F2B33D')
    const bg = cssHex('--bg', '#1a1a1a')
    return {
      matchBackground: mixHex(amber, bg, 0.4),
      activeMatchBackground: amber,
      matchOverviewRuler: amber,
      activeMatchColorOverviewRuler: amber,
    }
  }

  function loadSearch(t: object & { loadAddon(addon: SearchAddon): void }): SearchAddon {
    const existing = searchByTerm.get(t)
    if (existing) return existing
    const s = new SearchAddon({ highlightLimit: PREVIEW_FIND_CAP })
    t.loadAddon(s)
    s.onDidChangeResults((ev) => {
      const total = ev.resultCount
      const index = ev.resultIndex >= 0 ? ev.resultIndex + 1 : (total > 0 ? 1 : 0)
      findCbByTerm.get(t)?.(index, total, true)
    })
    searchByTerm.set(t, s)
    return s
  }
</script>

<script lang="ts">
  // One xterm.js surface bound to one Go PTY session.
  //
  // The pane owns the attach/detach lifecycle: mounting attaches (and repaints
  // from the replay buffer the Go side kept while nothing was listening),
  // unmounting detaches without killing the shell. Everything about keeping this
  // fast lives on the two ends of the stream — the WebGL renderer here, the
  // coalescing and flow control in internal/term.
  import { onMount, onDestroy } from 'svelte'
  import { Terminal } from '@xterm/xterm'
  import { FitAddon } from '@xterm/addon-fit'
  import { WebglAddon } from '@xterm/addon-webgl'
  import { WebLinksAddon } from '@xterm/addon-web-links'
  import '@xterm/xterm/css/xterm.css'
  import {
    editorPrefs, terminalPrefs, resolveTerminalFont, editorFontStack, theme as appTheme,
    workspaces,
  } from '../lib/stores'
  import { isBackquote } from '../lib/shortcut'
  import {
    onTerminalData, onTerminalExit, ackWriter, resolveTerminalTheme, decodeTermBytes,
    terminalThemeCatalog,
  } from '../lib/terminal'
  import { termFitSize, termHostInnerSize, TERM_FIT_SCROLLBAR_WIDTH } from '../lib/term-fit'
  import { termDockOwnsFocus } from '../lib/term-focus'
  import { openExternalUrl } from '../lib/open-external'
  import {
    AttachTerminal, DetachTerminal, WriteTerminal, ResizeTerminal, ClearTerminal,
  } from '../../bindings/recur/internal/app/terminalservice'
  import {
    bindTermPaneHooks,
    releaseParkedTerminal,
    shouldDisposeTerminal,
    stashParkedTerminal,
    takeParkedTerminal,
    termPaneHooks,
    type ParkedTerm,
  } from '../lib/caches/term-park'
  import { endFileDrag, fileDragPaths, isFileDrag, terminalDropText } from '../lib/file-drag'

  let {
    id,
    workspaceId = '',
    /** Hidden panes stay mounted (and attached) so background tabs keep their screen. */
    active = true,
    /**
     * WebGL is only for the live workbench's visible tab. Each addon is a GPU
     * context; WKWebView drops the oldest at ~8–16, which blanked parked
     * xterms and made miss-warm look like a 600–1400ms compositor hitch.
     */
    gpu = true,
    /** Transparent cells + WebGL alpha — only while maximized on a dark palette. */
    frosted = false,
    onTitle,
    onExit,
    onFindResults,
  }: {
    id: string
    workspaceId?: string
    active?: boolean
    gpu?: boolean
    frosted?: boolean
    onTitle?: (title: string) => void
    onExit?: (exitCode: number) => void
    onFindResults?: (index: number, total: number, valid: boolean) => void
  } = $props()

  let host: HTMLDivElement
  $effect(() => {
    const hooks = { onTitle, onExit }
    return bindTermPaneHooks(id, hooks)
  })
  let term: Terminal | undefined
  let fit: FitAddon | undefined
  let search: ReturnType<typeof loadSearch> | undefined
  let webgl: WebglAddon | undefined
  let writer: ReturnType<typeof ackWriter> | undefined
  let disposers: (() => void)[] = []
  let lastCols = 0
  let lastRows = 0
  let resizeTimer: ReturnType<typeof setTimeout> | undefined
  let destroyed = false
  let mountGen = 0

  /** Terminal font settings, with the editor's values filling in "match editor". */
  const font = $derived(resolveTerminalFont($terminalPrefs, $editorPrefs))
  const termThemeId = $derived($terminalPrefs.theme)
  const workspaceRoot = $derived($workspaces.find((w) => w.id === workspaceId)?.worktreePath ?? '')

  /**
   * pushResize tells the PTY its new geometry. Debounced because a drag on the
   * dock's resize handle fires continuously, and every SIGWINCH makes full-screen
   * programs repaint — an undebounced drag turns vim into a strobe light.
   */
  function pushResize(cols: number, rows: number) {
    if (cols === lastCols && rows === lastRows) return
    lastCols = cols
    lastRows = rows
    clearTimeout(resizeTimer)
    resizeTimer = setTimeout(() => { ResizeTerminal(id, cols, rows).catch(() => {}) }, 100)
  }

  function refit() {
    if (!term || !fit) return
    // A pane in a collapsed or hidden container measures as 0x0, and fitting to
    // that would resize the PTY down to its clamped minimum and reflow the shell's
    // scrollback for nothing.
    if (!host?.clientWidth || !host?.clientHeight) return
    const cell = term.dimensions?.css?.cell
    if (!cell?.width || !cell?.height) return
    // FitAddon uses the host border box and ignores `.term-pane` padding, which
    // painted the last row under the dock's overflow:hidden. Size to the content
    // box so the bottom row stays fully visible; leftover pixels sit at the top.
    const inner = termHostInnerSize(host)
    const bar = term.options.scrollback === 0 || term.options.scrollbar?.showScrollbar === false
      ? 0
      : (term.options.scrollbar?.width ?? TERM_FIT_SCROLLBAR_WIDTH)
    const { cols, rows } = termFitSize(inner.width, inner.height, cell.width, cell.height, bar)
    if (cols === term.cols && rows === term.rows) return
    try { term.resize(cols, rows) } catch { /* element mid-layout */ }
  }

  /**
   * Wait for the terminal's font to actually be available before xterm measures it.
   *
   * xterm sizes the character cell ONCE, when the terminal opens, and bakes that
   * measurement into its glyph atlas. The app's mono faces are Google webfonts
   * served with `display=swap` (index.html), so they are not there on first paint:
   * open too early and xterm measures the fallback, then the real face swaps in and
   * gets drawn on a grid built for different metrics. The result reads as two fonts
   * mixed together in one terminal.
   *
   * Bold is loaded too — shell prompts, ls colours and git output lean on it, and a
   * missing bold face falls back independently of the regular one.
   */
  async function awaitFont(family: string, size: number) {
    if (!document.fonts) return
    try {
      await Promise.all([
        document.fonts.load(`${size}px "${family}"`),
        document.fonts.load(`bold ${size}px "${family}"`),
      ])
    } catch { /* not a real family (e.g. System Mono) — the stack still resolves */ }
    try { await document.fonts.ready } catch { /* ignore */ }
  }

  onMount(() => { void mount() })

  let hostUnwatch: (() => void) | undefined

  function watchHost() {
    hostUnwatch?.()
    const el = host
    if (!el) return
    const ro = new ResizeObserver(() => refit())
    ro.observe(el)
    hostUnwatch = () => {
      ro.disconnect()
      hostUnwatch = undefined
    }
  }

  function pushTheme() {
    if (!term) return
    term.options.theme = resolveTerminalTheme(termThemeId, { transparentBg: frosted })
    try { term.refresh(0, term.rows - 1) } catch { /* mid-layout */ }
    try { term.clearTextureAtlas() } catch { /* DOM renderer has no atlas */ }
  }

  function adoptParked(p: ParkedTerm) {
    term = p.term as Terminal
    fit = p.fit as FitAddon
    search = loadSearch(term)
    bindFindCb()
    writer = p.writer as ReturnType<typeof ackWriter>
    disposers = p.disposers
    lastCols = p.lastCols
    lastRows = p.lastRows
    const screen = term.element
    if (screen) host.appendChild(screen)
    watchHost()
    if (active && gpu) ensureWebgl()
    pushTheme()
    refit()
    if (active) queueMicrotask(tryFocusIfDockOwnsKeys)
  }

  function snapshot(): ParkedTerm | null {
    if (!term || !writer || !fit) return null
    return {
      term,
      fit,
      writer,
      disposers,
      lastCols,
      lastRows,
      workspaceId,
    }
  }

  function killWidget(sessionId: string, p?: ParkedTerm | null) {
    hostUnwatch?.()
    const payload = p ?? (term && writer && fit ? snapshot() : null)
    if (payload) releaseParkedTerminal(payload)
    DetachTerminal(sessionId).catch(() => {})
    dropWebgl()
    disposers = []
  }

  async function mount() {
    const gen = ++mountGen
    await awaitFont(font.fontFace, font.fontSize)
    if (destroyed || gen !== mountGen) return

    const parked = takeParkedTerminal(id)
    if (parked) {
      adoptParked(parked)
      return
    }

    term = new Terminal({
      allowProposedApi: true,
      cursorBlink: true,
      // Locked at open(); stay on so maximize can frost via theme without a rebuild.
      allowTransparency: true,
      // A real scrollback lives here rather than in Go: the replay ring only has to
      // cover what a detached session missed, not the user's whole history.
      scrollback: 10000,
      fontFamily: editorFontStack(font.fontFace),
      fontSize: font.fontSize,
      lineHeight: font.lineHeight,
      theme: resolveTerminalTheme(termThemeId, { transparentBg: frosted }),
      // macOS-only app, so Option should compose characters the way every other
      // Mac terminal does rather than send Meta.
      macOptionIsMeta: false,
      macOptionClickForcesSelection: true,
    })

    // Keys the app owns rather than the shell. ⌘ combos are app-level on macOS
    // (palette, new task, save), and ⌃` / ⌃⇧` are the dock show and maximize
    // toggles — returning false stops xterm consuming them so they reach the
    // window handler in App.svelte. Every other ⌃ chord stays with the shell;
    // off macOS the dock claims its own Ctrl+Shift chords in capture phase
    // before this handler runs (TerminalDock.onDockKey).
    term.attachCustomKeyEventHandler((e) => {
      if (e.metaKey) return false
      if (e.ctrlKey && !e.altKey && isBackquote(e)) return false
      return true
    })

    // termenv (wails3/pterm, …) probes OSC 10/11/12 for theme colours. Go answers
    // those on the PTY and strips them from this stream (internal/term/queries.go);
    // if a probe still reaches xterm, swallow it so we don't also reply via the
    // async onData → WriteTerminal path and dump `11;rgb:…` onto the prompt.
    for (const code of [10, 11, 12]) {
      term.parser.registerOscHandler(code, () => true)
    }

    fit = new FitAddon()
    term.loadAddon(fit)
    search = loadSearch(term)
    bindFindCb()
    term.loadAddon(new WebLinksAddon((ev, uri) => {
      ev.preventDefault()
      void openExternalUrl(uri)
    }))
    term.open(host)

    // WebGL is the whole performance story on the render side. Contexts do get
    // lost on Macs (GPU switch, sleep/wake), and a lost context leaves a
    // permanently blank canvas — so dispose the addon and let xterm fall back
    // rather than leave the user staring at nothing. Only the live visible tab
    // holds a context; parked / hidden panes keep the canvas renderer.
    if (active && gpu) ensureWebgl()

    refit()

    const t = term
    const w = ackWriter(id, (bytes, done) => t.write(bytes, done))
    writer = w

    // Chunks can land between subscribing and the replay arriving, so hold them
    // until the replay is on screen — otherwise the terminal repaints history
    // over the top of newer output.
    let replayed = false
    const queued: Uint8Array[] = []
    disposers.push(onTerminalData(id, (bytes) => {
      if (!replayed) { queued.push(bytes); return }
      w.push(bytes)
    }))
    disposers.push(onTerminalExit(id, (code) => {
      termPaneHooks(id)?.onExit?.(code)
      // Parked pane: hooks are gone, so release the widget or it lives until the byte LRU drops it.
      queueMicrotask(() => {
        const leftover = takeParkedTerminal(id)
        if (!leftover) return
        releaseParkedTerminal(leftover)
        DetachTerminal(id).catch(() => {})
      })
    }))
    for (const d of [
      t.onData((data) => { WriteTerminal(id, data).catch(() => {}) }),
      t.onTitleChange((title) => termPaneHooks(id)?.onTitle?.(title)),
      t.onResize(({ cols, rows }) => pushResize(cols, rows)),
    ]) {
      disposers.push(() => d.dispose())
    }

    AttachTerminal(id)
      .then((b64) => {
        if (!term) return
        // Replay is written straight to the terminal, not through the ack writer:
        // the Go side hands it back out of band and does not count it against the
        // flow-control budget, so acking it would credit bytes that were never
        // debited.
        if (b64) t.write(decodeTermBytes(b64))
        replayed = true
        for (const b of queued) w.push(b)
        queued.length = 0
        // The PTY was opened at the size we guessed before layout settled; push the
        // measured one now that there is something on screen.
        refit()
        pushResize(term.cols, term.rows)
      })
      .catch(() => { replayed = true })

    watchHost()

    // A freshly spawned tab mounts already-active; the active-$effect below only
    // re-runs when `active` flips, so it would miss this and leave focus on the
    // dock chrome (or nowhere). Take keys only when the dock already owns them
    // — spawn() parks there first. A workspace-switch remount must not steal
    // from the editor or agent (focus is on body after the previous dock dies).
    if (active) queueMicrotask(tryFocusIfDockOwnsKeys)
  }

  function dropWebgl() {
    try { webgl?.dispose() } catch { /* already gone */ }
    webgl = undefined
  }

  function ensureWebgl() {
    if (!term || webgl) return
    try {
      const addon = new WebglAddon()
      addon.onContextLoss(() => { dropWebgl() })
      term.loadAddon(addon)
      webgl = addon
    } catch { /* no WebGL2 — xterm keeps its canvas renderer */ }
  }

  function tryFocusIfDockOwnsKeys() {
    refit()
    if (termDockOwnsFocus(host)) term?.focus()
  }

  onDestroy(() => {
    destroyed = true
    clearTimeout(resizeTimer)
    hostUnwatch?.()
    dropWebgl()
    search?.clearDecorations()
    if (term) findCbByTerm.delete(term)
    const snap = snapshot()
    if (shouldDisposeTerminal(id) || !snap) {
      if (snap || term) killWidget(id, snap)
      term = undefined
      fit = undefined
      search = undefined
      writer = undefined
      return
    }
    // Keep Go attached and the xterm instance alive. Warm miss reparents
    // term.element instead of Attach + replay.
    const evicted = stashParkedTerminal(id, snap)
    term = undefined
    fit = undefined
    search = undefined
    writer = undefined
    disposers = []
    for (const gone of evicted) killWidget(gone.id, gone.payload)
  })

  $effect(() => {
    const want = active && gpu
    if (!term) return
    if (want) ensureWebgl()
    else dropWebgl()
  })

  // Live-follow font changes from Settings, without a remount.
  $effect(() => {
    const { fontFace, fontSize, lineHeight } = font
    if (!term) return
    void awaitFont(fontFace, fontSize).then(() => {
      if (!term) return
      term.options.fontFamily = editorFontStack(fontFace)
      term.options.fontSize = fontSize
      term.options.lineHeight = lineHeight
      // The WebGL renderer caches rasterised glyphs; without dropping the atlas the
      // old face keeps being drawn at the new metrics.
      try { term.clearTextureAtlas() } catch { /* DOM renderer has no atlas */ }
      refit()
    })
  })

  // xterm holds a JS palette rather than reading CSS, so a theme switch has to be
  // pushed in explicitly. Frost is transparentBg on the same instance.
  $effect(() => {
    void $appTheme
    void $terminalPrefs.theme
    void $terminalThemeCatalog
    void frosted
    pushTheme()
  })

  // Becoming visible after a tab switch: the pane was 0x0 while hidden, so it has
  // to re-measure before it can be focused. Skip if the dock does not already
  // own the keys — otherwise a workspace-switch remount steals from the last
  // chrome (editor / agent) that this workspace had focused.
  $effect(() => {
    if (!active || !term) return
    queueMicrotask(tryFocusIfDockOwnsKeys)
  })

  export function focus() { term?.focus() }

  /** Selected text in the viewport, for the dock's Ctrl+Shift+C off macOS. */
  export function getSelection(): string { return term?.getSelection() ?? '' }

  /** Paste as the user typing it (bracketed paste when the shell asks). */
  export function paste(text: string) { term?.paste(text) }

  /** Clears the screen and the Go-side replay buffer, so re-attaching stays clear. */
  export function clear() {
    term?.clear()
    ClearTerminal(id).catch(() => {})
  }

  type FindOpts = { caseSensitive?: boolean; regexp?: boolean; wholeWord?: boolean; incremental?: boolean }

  function searchOpts(opts: FindOpts = {}) {
    return {
      caseSensitive: !!opts.caseSensitive,
      regex: !!opts.regexp,
      wholeWord: !!opts.wholeWord,
      incremental: !!opts.incremental,
      decorations: searchDecorations(),
    }
  }

  function bindFindCb() {
    if (!term) return
    findCbByTerm.set(term, (index, total, valid) => onFindResults?.(index, total, valid))
  }

  let lastFindKey = ''

  function runFind(dir: 1 | -1, query: string, opts?: FindOpts): boolean {
    if (!search) return false
    bindFindCb()
    const o = searchOpts(opts)
    // The addon never re-highlights the same term when only options change.
    const key = `${o.caseSensitive}|${o.regex}|${o.wholeWord}`
    if (key !== lastFindKey) search.clearDecorations()
    lastFindKey = key
    try {
      return dir > 0 ? search.findNext(query, o) : search.findPrevious(query, o)
    } catch {
      search.clearDecorations()
      if (term) findCbByTerm.get(term)?.(0, 0, false)
      return false
    }
  }

  export function findNext(query: string, opts?: FindOpts): boolean {
    return runFind(1, query, opts)
  }

  export function findPrevious(query: string, opts?: FindOpts): boolean {
    return runFind(-1, query, opts)
  }

  export function clearSearch() {
    search?.clearDecorations()
  }

  function onDragOver(e: DragEvent) {
    if (!isFileDrag(e.dataTransfer)) return
    e.preventDefault()
    if (e.dataTransfer) e.dataTransfer.dropEffect = 'copy'
  }

  function onDrop(e: DragEvent) {
    const paths = fileDragPaths(e.dataTransfer)
    endFileDrag()
    if (!paths.length) return
    e.preventDefault()
    e.stopPropagation()
    const text = terminalDropText(paths, workspaceRoot)
    if (!text) return
    WriteTerminal(id, text).catch(() => {})
    term?.focus()
  }
</script>

<div
  class="term-pane"
  class:hidden={!active}
  bind:this={host}
  ondragentercapture={onDragOver}
  ondragovercapture={onDragOver}
  ondropcapture={onDrop}
></div>

<style>
  .term-pane {
    width: 100%;
    height: 100%;
    min-height: 0;
    padding: 6px 0 6px 8px;
    background: transparent;
    /* Pin the grid to the bottom of the pane: leftover sub-row pixels (and any
       overflow) clip or gap at the top so the last row stays fully visible. */
    display: flex;
    flex-direction: column;
    justify-content: flex-end;
  }
  /* Hidden tabs stay mounted so their shells keep streaming into a live screen;
     display:none would zero the measurement and force a reflow on every switch. */
  .term-pane.hidden {
    visibility: hidden;
    position: absolute;
    pointer-events: none;
    z-index: -1;
  }
</style>
