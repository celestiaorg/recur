<script lang="ts">
  // Recur-integrated CodeMirror 6 surface — view + edit in one place.
  import { onMount, onDestroy, untrack } from 'svelte'
  import {
    EditorView, keymap,
  } from '@codemirror/view'
  import {
    EditorState, Compartment, Transaction, StateEffect, Prec,
    type Extension, type Text,
  } from '@codemirror/state'
  import {
    findingsGutterExtension,
    setEditorFindings,
    type EditorFindingMark,
  } from '../lib/cm-findings-gutter'
  import { recurLintGutter } from '../lib/cm-lint-gutter'
  import { runLspGotoDef } from '../lib/lsp'
  import { diffGutterExtension, setEditorDiffBase } from '../lib/cm-diff-gutter'
  import { historyKeymap, indentWithTab, toggleComment } from '@codemirror/commands'
  import { recurDefaultKeymap } from '../lib/cm-keymap'
  import { isCmdOrCtrl } from '../lib/shortcut'
  import {
    captureEditorHistory, captureEditorHistoryFromState, restoreEditorState,
    editorHistoryKey, editorHistoryExtension,
  } from '../lib/cm-editor-history'
  import {
    foldKeymap, indentOnInput, forceParsing, indentUnit,
  } from '@codemirror/language'
  import {
    highlightSelectionMatches, openSearchPanel, closeSearchPanel,
    selectNextOccurrence, searchPanelOpen,
  } from '@codemirror/search'
  import { mentionRangeFromLoc, mentionSelectionFromState, type MentionSelection } from '../lib/cm-mention-sel'
  import { pointerMouseSelection, posAtPointer } from '../lib/cm-pointer-pos'
  import { recurSearchExtension, recurSearchKeymap } from '../lib/cm-search-panel'
  import { recurGotoLine } from '../lib/cm-goto-line'
  import {
    closeBrackets, closeBracketsKeymap, autocompletion, completionKeymap, completeAnyWord,
    snippet, nextSnippetField, prevSnippetField,
  } from '@codemirror/autocomplete'
  import { get } from 'svelte/store'
  import { languageForPathSync, loadLanguageForPath, languageIdForPath } from '../lib/cm-lang'
  import { textmateDocKey, tmDocIdFor } from '../lib/cm-tm-doc'
  import { grammarCatalogEpoch, tmForgetDocs } from '../lib/tm-registry'
  import { syntaxModeEpoch } from '../lib/syntax-mode'
  import { editorChromeExtension, editorThemeExtension } from '../lib/cm-themes'
  import {
    editorFeatureExtensions, keymapExtensionSync, loadKeymapExtension,
  } from '../lib/cm-editor-prefs'
  import {
    editorTheme, editorPrefs, editorFontStack, resolveEditorIndent,
    type EditorPrefs, type EditorIndent,
  } from '../lib/stores'
  import { scopeForPath, type EditorContext, type OutputRouting } from '../lib/commands'
  import {
    applySnippet,
    bundleSnippetCompletionSource,
    commandSnippetRange,
    prefetchSnippets,
    toCodeMirrorSnippet,
    tryExpandSnippet,
    type RegisteredSnippet,
  } from '../lib/snippets'
  import { insertNewlineBetweenPairs } from '../lib/cm-lang-data'
  import { openKeybindingMenu } from '../lib/keybinding-menu'
  import {
    captureFileViewState, scheduleApplyFileViewState, focusPreservingScroll,
    scrollEffectFromViewState, EDITOR_BODY_CAP, safeScrollSnapshot,
    replaceDocKeepingScroll,
    type FileViewState,
  } from '../lib/editor-view-state'
  import { isBufferDirty, bufferMountState, diskBaselineMoved, editorText } from '../lib/editor-dirty'
  import { pruneAllSessionTabParks, sessionTabPark } from '../lib/caches/cm-session-tabs'
  import { onColorMapChange } from '../lib/tm-registry'

  let {
    doc = '',
    path = '',
    readOnly = true,
    savedDoc,
    findingMarks = [] as EditorFindingMark[],
    /** FileAtBase text for SCM gutter hints. `null`/`undefined` = not loaded yet. */
    baseDoc = null as string | null,
    onDocChange,
    onDirtyChange,
    onSave,
    onCopyLine,
    onCursor,
    onGotoDef,
    onSearchOpenChange,
    onOpenFinding,
    viewState = null as FileViewState | null,
    sessionKey = '',
    parkPane = 'main',
  }: {
    doc?: string
    path?: string
    readOnly?: boolean
    /**
     * On-disk (or last-saved) text. When this differs from `doc`, the buffer
     * mounts already dirty — used when restoring an unsaved tab.
     */
    savedDoc?: string
    /** Open findings for this file — severity dots in a slim gutter. */
    findingMarks?: EditorFindingMark[]
    /**
     * Contents at the loop/project base (same document Diff uses).
     * `null` or omitted means the gutter should stay empty (not yet loaded,
     * non-git, scratch). Empty string is a new file — every line is an add.
     */
    baseDoc?: string | null
    /**
     * Full document text, debounced (see EMIT_DEBOUNCE_MS). Callers that need
     * the authoritative text right now — saving, in particular — must call
     * getDoc() instead of reading whatever this last delivered.
     */
    onDocChange?: (value: string) => void
    /** Synchronous, allocation-free "has this buffer diverged from disk". */
    onDirtyChange?: (dirty: boolean) => void
    onSave?: () => void
    onCopyLine?: (line: number) => void
    onCursor?: (line: number, col: number) => void
    /** Word under cursor / Mod-click target → go to definition. */
    onGotoDef?: (word: string) => void
    /** Fired when the Cmd+F find panel opens or closes. */
    onSearchOpenChange?: (open: boolean) => void
    /** Gutter marker click → open this finding in the Findings panel. */
    onOpenFinding?: (findingId: string) => void
    /**
     * Caret/scroll to apply when this path is mounted. Read only on path
     * change — later updates must go through restoreViewState().
     */
    viewState?: FileViewState | null
    /**
     * Workspace identity. Live tab EditorStates are parked under this key on
     * switch (not dropped) so returning to a large buffer does not re-parse.
     */
    sessionKey?: string
    /**
     * Distinguishes main vs split so the two panes never share an EditorState.
     * Process-wide so a brief unmount (cold restore clearing editorPath) still hits.
     */
    parkPane?: string
  } = $props()

  let host: HTMLDivElement | undefined = $state()
  let view: EditorView | undefined
  const langCompartment = new Compartment()
  const roCompartment = new Compartment()
  const editableCompartment = new Compartment()
  const themeCompartment = new Compartment()
  const chromeCompartment = new Compartment()
  const wrapCompartment = new Compartment()
  const indentCompartment = new Compartment()
  const featuresCompartment = new Compartment()
  const keymapCompartment = new Compartment()
  const diffGutterCompartment = new Compartment()
  const lspCompartment = new Compartment()
  let mountedLsp: Extension = []
  let applyingExternal = false
  let mountedPath = ''
  let mountedRO: boolean | undefined
  let mountedDoc = ''
  let unsubTheme: (() => void) | undefined
  let unsubPrefs: (() => void) | undefined
  let unsubColorMap: (() => void) | undefined
  let unsubGrammar: (() => void) | undefined
  let unsubSyntax: (() => void) | undefined
  let lastSearchOpen = false
  /** Applied on the next mount / empty→full hydrate. */
  let pendingViewState: FileViewState | null = null
  let resyncTimer: number | undefined
  /**
   * One EditorView per pane; inactive tabs cache their EditorState (doc,
   * selection, undo history) plus a pixel-exact scroll snapshot. Switching
   * tabs is view.setState(cached) + dispatching the snapshot — CodeMirror's
   * intended tabbed-editor pattern. Nothing is hidden or reparented, so
   * WebKit never zeroes scrollTop and CodeMirror never sees an invisible
   * view whose viewport it could drop.
   */
  type TabSnapshot = {
    state: EditorState
    scroll: StateEffect<unknown>
    dirty: boolean
    /** Disk/last-saved baseline for this tab — not the previously mounted file. */
    clean: Text | undefined
  }
  const tabs = new Map<string, TabSnapshot>()
  const panePark = sessionTabPark<TabSnapshot>(untrack(() => parkPane))
  /** Set when Observatory parked this pane before swapping roots — skip a second park. */
  let parkedOutgoing = ''
  let mountedSession = ''
  /** Last path switch landed on live cached state — the view is already where
   *  the user left it, so a follow-up restoreViewState must not re-scroll. */
  let restoredLive = false
  /**
   * Scroll restore in flight. CodeMirror applies a dispatched scroll target on
   * the next measure frame and keeps only the last one, so until layout
   * settles (scheduleResync's double rAF) any code that would re-dispatch the
   * *current* scroll position — focus handling, grammar arrival, doc drift —
   * must re-dispatch this target instead, or it clobbers the restore with the
   * pre-restore position.
   */
  let settleScroll: StateEffect<unknown> | null = null

  /**
   * The last text this component handed to the parent, kept so the round-trip
   * back in through the `doc` prop can be recognised by reference in O(1).
   *
   * The parent stores exactly the string we give it and feeds it straight back
   * as `doc`, so without this the "did the file change underneath us?" guard
   * below had to serialise the whole document a second time on every keystroke
   * just to discover the answer was no.
   */
  let lastEmitted: string | null = null

  /**
   * Typing pushes a dirty flag synchronously and the text on a trailing
   * debounce. Nothing on screen needs the full string per character — the
   * status bar wants a boolean, and Preview/Diff only render while this editor
   * is read-only — so serialising a rope of arbitrary size on every keypress
   * bought nothing. Save reads through getDoc(), which is always current.
   */
  const EMIT_DEBOUNCE_MS = 120
  let emitTimer: ReturnType<typeof setTimeout> | undefined
  let dirty = false
  /** Document considered in-sync with disk / last save. Undo back to this is clean. */
  let cleanDoc: Text | undefined
  /** Stable ref so the findings gutter extension can call the latest callback. */
  const onOpenFindingRef: { current: ((id: string) => void) | undefined } = { current: undefined }
  $effect(() => {
    onOpenFindingRef.current = onOpenFinding
  })

  $effect(() => {
    const p = path
    if (!p) return
    prefetchSnippets(scopeForPath(p), p)
  })

  function indentString(indent: EditorIndent): string {
    if (indent === 'tabs') return '\t'
    if (indent === 'spaces-4') return '    '
    return '  '
  }

  function wrapExtension(on: boolean): Extension {
    return on ? EditorView.lineWrapping : []
  }

  function indentExtensions(prefs: EditorPrefs, filePath: string): Extension {
    const indent = resolveEditorIndent(prefs, languageIdForPath(filePath))
    return [
      EditorState.tabSize.of(prefs.tabSize),
      indentUnit.of(indentString(indent)),
    ]
  }

  function chromeExtension(prefs: EditorPrefs): Extension {
    return editorChromeExtension(
      prefs.fontSize,
      editorFontStack(prefs.fontFace),
      prefs.lineHeight,
      prefs.thickCaret,
    )
  }

  function featuresExtension(prefs: EditorPrefs): Extension {
    return editorFeatureExtensions(prefs, onCopyLine)
  }

  function prefsEffects(prefs: EditorPrefs, filePath: string) {
    return [
      chromeCompartment.reconfigure(chromeExtension(prefs)),
      wrapCompartment.reconfigure(wrapExtension(prefs.lineWrapping)),
      indentCompartment.reconfigure(indentExtensions(prefs, filePath)),
      featuresCompartment.reconfigure(featuresExtension(prefs)),
      keymapCompartment.reconfigure(keymapExtensionSync(prefs.keymap)),
      diffGutterCompartment.reconfigure(prefs.diffGutter ? diffGutterExtension() : []),
    ]
  }

  /** Fetch vim/emacs on first selection and apply it if still selected. */
  function ensureKeymap(mode: EditorPrefs['keymap']) {
    void loadKeymapExtension(mode).then((ext) => {
      if (!view || !ext) return
      if (get(editorPrefs).keymap !== mode) return
      view.dispatch({ effects: keymapCompartment.reconfigure(ext) })
    })
  }

  function setDirty(next: boolean) {
    if (dirty === next) return
    dirty = next
    onDirtyChange?.(next)
  }

  function rememberClean(doc: Text) {
    cleanDoc = doc
    setDirty(false)
  }

  /** Parent's on-disk snapshot moved (hydrate after an agent write). */
  function applySavedBaseline(text: string) {
    if (!diskBaselineMoved(cleanDoc, text)) return
    cleanDoc = editorText(text)
    if (view) syncDirtyFrom(view.state.doc)
  }

  function syncDirtyFrom(doc: Text) {
    const next = isBufferDirty(doc, cleanDoc)
    setDirty(next)
    if (next) scheduleEmit()
    else flushEmit()
  }

  function scheduleEmit() {
    if (emitTimer) return
    emitTimer = setTimeout(flushEmit, EMIT_DEBOUNCE_MS)
  }

  /** Serialise once and publish; also refreshes the echo guard. */
  function flushEmit() {
    if (emitTimer) {
      clearTimeout(emitTimer)
      emitTimer = undefined
    }
    if (!view) return
    const text = view.state.doc.toString()
    lastEmitted = text
    onDocChange?.(text)
  }

  /**
   * True when `next` is text that originated here (a debounced emit, or the
   * getDoc() the parent just saved) rather than a genuine on-disk change.
   * Ordered cheapest-first: reference hit, then length, then — only for a
   * same-length near-miss, which is rare — a real comparison.
   */
  function isLocalEcho(next: string): boolean {
    if (next === lastEmitted) return true
    if (!view) return false
    if (next.length !== view.state.doc.length) return false
    return next === view.state.doc.toString()
  }

  function wordAt(view: EditorView, pos: number): string {
    const word = view.state.wordAt(pos)
    if (!word) return ''
    return view.state.doc.sliceString(word.from, word.to)
  }

  function baseExtensions(): Extension[] {
    return [
      editorHistoryExtension(),
      indentOnInput(),
      Prec.high(keymap.of([{ key: 'Enter', run: insertNewlineBetweenPairs }])),
      closeBrackets(),
      autocompletion({
        override: [
          bundleSnippetCompletionSource(() => {
            const filePath = mountedPath || path
            return { scope: scopeForPath(filePath), path: filePath }
          }),
          completeAnyWord,
        ],
        defaultKeymap: false,
      }),
      highlightSelectionMatches(),
      pointerMouseSelection(),
      recurSearchExtension(),
      keymap.of([
        ...closeBracketsKeymap,
        ...completionKeymap,
        ...recurDefaultKeymap,
        ...recurSearchKeymap,
        ...historyKeymap,
        ...foldKeymap,
        // snippetKeymap is a Facet, not a KeyBinding[] — bind the commands.
        { key: 'Tab', run: nextSnippetField, shift: prevSnippetField },
        {
          key: 'Tab',
          run: (v) => expandTabSnippet(v),
        },
        indentWithTab,
        { key: 'Mod-/', run: toggleComment },
        { key: 'Mod-d', run: selectNextOccurrence },
        {
          key: 'Mod-s',
          run: () => {
            onSave?.()
            return true
          },
          preventDefault: true,
        },
        {
          key: 'F12',
          run: (v) => {
            if (runLspGotoDef(v)) return true
            const w = wordAt(v, v.state.selection.main.head)
            if (w) onGotoDef?.(w)
            return !!w
          },
        },
        { key: 'Mod-f', run: openSearchPanel },
        // ⌘G = find next (searchKeymap); go-to-line is Recur dialog on ⌥⌘G.
        { key: 'Mod-Alt-g', run: recurGotoLine },
        { key: 'Escape', run: closeSearchPanel },
      ]),
      EditorView.domEventHandlers({
        click: (e, v) => {
          if (!isCmdOrCtrl(e)) return false
          const pos = posAtPointer(v, e).pos
          if (runLspGotoDef(v)) {
            e.preventDefault()
            return true
          }
          const w = wordAt(v, pos)
          if (!w) return false
          e.preventDefault()
          onGotoDef?.(w)
          return true
        },
      }),
      EditorView.updateListener.of((u) => {
        const open = searchPanelOpen(u.state)
        if (open !== lastSearchOpen) {
          lastSearchOpen = open
          onSearchOpenChange?.(open)
        }
        if (applyingExternal) return
        if (u.docChanged) {
          syncDirtyFrom(u.state.doc)
        }
        if (u.docChanged || u.selectionSet) {
          const head = u.state.selection.main.head
          const line = u.state.doc.lineAt(head)
          onCursor?.(line.number, head - line.from + 1)
        }
      }),
    ]
  }

  type ExtensionSlots = {
    theme: Extension
    chrome: Extension
    wrap: Extension
    indent: Extension
    features: Extension
    keymap: Extension
    diffGutter: Extension
    ro: Extension
    editable: Extension
  }

  /** Slot values for a mount or parked restore. */
  function extensionSlots(filePath: string, ro: boolean, themeKey: string, prefs: EditorPrefs): ExtensionSlots {
    return {
      theme: editorThemeExtension(themeKey),
      chrome: chromeExtension(prefs),
      wrap: wrapExtension(prefs.lineWrapping),
      indent: indentExtensions(prefs, filePath),
      features: featuresExtension(prefs),
      keymap: keymapExtensionSync(prefs.keymap),
      diffGutter: prefs.diffGutter ? diffGutterExtension() : [],
      ro: EditorState.readOnly.of(ro),
      editable: EditorView.editable.of(!ro),
    }
  }

  function buildExtensions(filePath: string, slots: ExtensionSlots): Extension[] {
    return [
      ...baseExtensions(),
      themeCompartment.of(slots.theme),
      chromeCompartment.of(slots.chrome),
      wrapCompartment.of(slots.wrap),
      indentCompartment.of(slots.indent),
      findingsGutterExtension(onOpenFindingRef),
      // LSP diagnostics (underlines from @codemirror/lint; sev-* icons here).
      recurLintGutter(),
      featuresCompartment.of(slots.features),
      // SCM strip against the code, like the merge/diff view.
      // Pref-gated: an always-on gutter reserves a column even when Off.
      diffGutterCompartment.of(slots.diffGutter),
      lspCompartment.of(mountedLsp),
      keymapCompartment.of(slots.keymap),
      // Path-stable TM cache key — outside the lang compartment so a grammar
      // swap remounts the highlighter without dropping the token cache. Scoped
      // by pane: both panes can hold the same file, and they must not share
      // one engine document (each would apply the same edit to it).
      textmateDocKey(tmDocIdFor(parkPane, filePath)),
      // Whatever grammar is already resident renders on the first paint;
      // anything else arrives via the reconfigure in ensureLanguage().
      langCompartment.of(languageForPathSync(filePath)),
      roCompartment.of(slots.ro),
      editableCompartment.of(slots.editable),
    ]
  }

  /** Compartment.reconfigure effects that actually move parked slot values.
   *  StateEffect.reconfigure.of(compartment.of(x)) leaves the old map entry.
   *  Lang is not a slot: languageForPathSync() is [] when the grammar is still
   *  loading, and reconfigure would wipe a highlighter ensureLanguage() already
   *  installed (and tear down the TM ViewPlugin under a scrollSnapshot). */
  function slotReconfigureEffects(slots: ExtensionSlots) {
    return [
      themeCompartment.reconfigure(slots.theme),
      chromeCompartment.reconfigure(slots.chrome),
      wrapCompartment.reconfigure(slots.wrap),
      indentCompartment.reconfigure(slots.indent),
      featuresCompartment.reconfigure(slots.features),
      keymapCompartment.reconfigure(slots.keymap),
      diffGutterCompartment.reconfigure(slots.diffGutter),
      roCompartment.reconfigure(slots.ro),
      editableCompartment.reconfigure(slots.editable),
    ]
  }

  function buildState(initialDoc: string, filePath: string, ro: boolean, themeKey: string, prefs: EditorPrefs): EditorState {
    const extensions = buildExtensions(filePath, extensionSlots(filePath, ro, themeKey, prefs))
    // Second-tier undo: the live tab cache is parked across workspace
    // switches, but dies on a cache miss / unmount. Serialized historyField
    // survives — rebuild it into the fresh state when the doc still matches.
    // Its scroll snapshot is deliberately ignored: caret/scroll restore here
    // goes through viewState/scrollSnapshot, never raw scrollDOM writes.
    const restored = restoreEditorState(historyKeyFor(filePath), initialDoc, extensions)
    if (restored) return restored.state
    return EditorState.create({ doc: initialDoc, extensions })
  }

  function historyKeyFor(filePath: string): string {
    return editorHistoryKey(mountedSession, filePath)
  }

  /**
   * Serialize live undo stacks (active view + stashed tabs) into the
   * byte-budgeted per-file cache — called when the live tier is about to die
   * (workspace switch, unmount) or before a rename remaps the keys.
   */
  function captureAllHistory() {
    if (view && mountedPath) captureEditorHistory(historyKeyFor(mountedPath), view)
    for (const [p, snap] of tabs) {
      captureEditorHistoryFromState(historyKeyFor(p), snap.state)
    }
  }

  /** Flush live undo into the per-file cache (rename, before paths remap). */
  /** Live EditorView for LSP Workspace.displayFile / cross-file jumps. */
  export function getView(): EditorView | undefined {
    return view
  }

  /** Swap the LSP plugin extension without remounting the buffer. */
  export function setLspExtension(ext: Extension) {
    mountedLsp = ext ?? []
    if (!view) return
    try {
      view.dispatch({ effects: lspCompartment.reconfigure(mountedLsp) })
    } catch (err) {
      console.warn('[lsp] failed to attach plugin', err)
      mountedLsp = []
      try {
        view.dispatch({ effects: lspCompartment.reconfigure([]) })
      } catch { /* ignore */ }
    }
  }

  export function captureHistory() {
    captureAllHistory()
  }

  function stillThisPath(filePath: string): boolean {
    return !!view && mountedPath === filePath
  }

  function destroyView(v: EditorView | undefined) {
    try { v?.destroy() } catch { /* already gone */ }
  }

  function destroyAllViews() {
    captureAllHistory()
    tabs.clear()
    destroyView(view)
    view = undefined
    settleScroll = null
    mountedPath = ''
    mountedDoc = ''
    parkedOutgoing = ''
  }

  /** Skip huge buffers in the process-wide park — the live `tabs` map still
   *  holds a dirty over-cap EditorState for in-workspace tab switches. */
  function parkableTabs(): Map<string, TabSnapshot> {
    const out = new Map<string, TabSnapshot>()
    for (const [p, snap] of tabs) {
      if (snap.state.doc.length > EDITOR_BODY_CAP) continue
      out.set(p, snap)
    }
    return out
  }

  /**
   * Freeze live tab EditorStates under `session` before Observatory swaps
   * roots. Must run before openFiles changes: retainPaths would otherwise
   * prune this worktree's cache using the incoming workspace's tab list.
   */
  export function parkSession(session: string) {
    if (!session) return
    stashCurrent()
    captureAllHistory()
    panePark.park(session, parkableTabs())
    parkedOutgoing = session
    tabs.clear()
  }

  /**
   * Park the outgoing workspace (unless Observatory already did) and restore
   * the incoming tab map. Keeps the EditorView on a cache hit so setState can
   * reuse a parsed buffer; drops it on a miss so the previous file does not
   * sit on screen while the next one tokenizes.
   */
  function adoptSession(next: string, nextPath: string) {
    if (next === mountedSession) return
    if (parkedOutgoing !== mountedSession) {
      stashCurrent()
      if (mountedSession) {
        captureAllHistory()
        panePark.park(mountedSession, parkableTabs())
      }
    }
    parkedOutgoing = ''
    tabs.clear()
    const restored = panePark.take(next)
    if (restored) {
      for (const [path, snap] of restored) tabs.set(path, snap)
    }
    mountedSession = next
    mountedPath = ''
    if (!tabs.has(nextPath) && view) {
      destroyView(view)
      view = undefined
    }
  }

  /** Remeasure after layout so posAtCoords matches the painted lines. */
  export function resync() {
    view?.requestMeasure()
  }

  /** Drop cached tab states for paths that are no longer open. */
  export function retainPaths(keep: Iterable<string>) {
    const live = new Set(keep)
    const dropped: string[] = []
    for (const p of [...tabs.keys()]) {
      if (!live.has(p)) {
        tabs.delete(p)
        dropped.push(p)
      }
    }
    // Closing a tab frees its tokens (VS Code disposes the TextModel here).
    // Inactive-but-open tabs keep theirs — that is what makes coming back free.
    if (dropped.length > 0) tmForgetDocs(dropped.map((p) => tmDocIdFor(parkPane, p)))
  }

  function scheduleResync(filePath: string) {
    if (resyncTimer != null) cancelAnimationFrame(resyncTimer)
    resyncTimer = requestAnimationFrame(() => {
      resyncTimer = requestAnimationFrame(() => {
        resyncTimer = undefined
        // Layout has settled and the measure frame ran. Re-apply the parked
        // scroll now that the height map is real, then drop it so later
        // focus() snapshots live DOM.
        if (!stillThisPath(filePath)) return
        if (view && settleScroll) {
          try { view.dispatch({ effects: settleScroll }) } catch { /* tile map */ }
        }
        settleScroll = null
        view?.requestMeasure()
      })
    })
  }

  function applyPending(filePath: string, keepIfEmpty = false) {
    const vs = pendingViewState
    if (!view || !vs) {
      scheduleResync(filePath)
      return
    }
    if (keepIfEmpty && view.state.doc.length === 0) {
      scheduleResync(filePath)
      return
    }
    pendingViewState = null
    settleScroll = scrollEffectFromViewState(view.state.doc, vs) ?? null
    scheduleApplyFileViewState(view, vs, () => stillThisPath(filePath))
    scheduleResync(filePath)
  }

  /**
   * Snapshot the active tab so switching back restores doc, selection, undo
   * history, and exact scroll offset. A pending debounced emit is cancelled —
   * the cached state already holds that text, and syncRestoredState pushes it
   * back out to the parent when a dirty tab returns.
   */
  function stashCurrent() {
    if (!view || !mountedPath) return
    if (emitTimer) {
      clearTimeout(emitTimer)
      emitTimer = undefined
    }
    // Huge *clean* buffers: remount from the parent's draft instead of
    // retaining a full EditorState per inactive tab. Dirty huge buffers stay
    // in `tabs` so an in-workspace tab switch keeps undo without serializing
    // the doc (toJSON would copy 1–16 MiB and can evict the history budget).
    if (!dirty && view.state.doc.length > EDITOR_BODY_CAP) return
    // Rapid tab cycling can leave this tab's own restore unapplied (it lands
    // on the next measure frame) — stash that target, not the stale DOM
    // position it is about to replace.
    let scroll = settleScroll
    if (!scroll) {
      try {
        scroll = view.scrollSnapshot()
      } catch {
        scroll = EditorView.scrollIntoView(view.state.selection.main.head, { y: 'nearest' })
      }
    }
    tabs.set(mountedPath, {
      state: view.state,
      scroll,
      dirty,
      clean: cleanDoc,
    })
  }

  /**
   * Bring a restored tab up to date: theme/prefs may have changed while it
   * was inactive, and the file on disk may have moved under it.
   */
  function syncRestoredState(
    filePath: string,
    initialDoc: string,
    ro: boolean,
    snap: TabSnapshot,
    reconfigured = false,
    focus = true,
  ) {
    if (!view) return
    const prefs = get(editorPrefs)
    // mountEditor's cached path folds theme/prefs/RO into the parked state
    // before setState — re-dispatching identical reconfigures here was an
    // extra transaction on every tab switch. Only reconcile when that
    // reconfigure did not happen (its state.update threw).
    if (!reconfigured) {
      try {
        view.dispatch({
          effects: slotReconfigureEffects(extensionSlots(filePath, ro, get(editorTheme), prefs)),
        })
      } catch { /* tile map mid-rebuild */ }
    }
    mountedRO = ro
    settleScroll = snap.scroll
    ensureLanguage(filePath)
    ensureKeymap(prefs.keymap)
    try {
      setEditorFindings(view, findingMarks)
      setEditorDiffBase(view, get(editorPrefs).diffGutter ? (baseDoc ?? null) : null)
    } catch { /* tile map mid-rebuild */ }
    if (snap.dirty) {
      // Unsaved edits outrank the parent's draft (a debounced emit may have
      // been cancelled at stash time) — re-emit the authoritative text
      // instead of clobbering the buffer with a stale prop.
      if (snap.clean) cleanDoc = snap.clean
      try { view.dispatch({ effects: snap.scroll }) } catch { /* ignore */ }
      setDirty(true)
      flushEmit()
    } else if (initialDoc !== view.state.doc.toString() && !isLocalEcho(initialDoc)) {
      replaceDocKeepingScroll(view, initialDoc, snap.scroll)
      rememberClean(view.state.doc)
    } else {
      rememberClean(snap.clean ?? view.state.doc)
      try { view.dispatch({ effects: snap.scroll }) } catch { /* ignore */ }
    }
    mountedDoc = initialDoc
    view.requestMeasure()
    // Always re-dispatch the parked scroll on the next measure frame (the
    // height map is still estimating after setState). Only the actual
    // view.focus() is skipped on a workspace swap — focusing here yanks
    // WebKit to the caret. Observatory focuses after layout instead.
    focusPreservingScroll(view, snap.scroll, () => stillThisPath(filePath), { focus })
  }

  /**
   * Point the pane's single EditorView at `filePath`. Open tabs cache their
   * EditorState + scroll snapshot, so switching back restores caret, undo
   * history, and exact scroll without remounting or hiding any DOM. Click
   * mapping right after the swap is safe even while the rebuilt height map
   * settles — pointerMouseSelection resolves clicks against painted lines.
   */
  function mountEditor(
    initialDoc: string,
    filePath: string,
    ro: boolean,
    themeKey: string,
    prefs: EditorPrefs,
    focus = true,
  ) {
    if (!host) return
    restoredLive = false
    // LSP plugin is path-specific; clear before remount, parent re-attaches.
    mountedLsp = []

    stashCurrent()
    settleScroll = null
    const cached = tabs.get(filePath)
    if (cached) tabs.delete(filePath)

    mountedPath = filePath
    lastEmitted = null

    if (cached) {
      // Fold the reconfigure into the parked state so setState is one
      // transaction. The TM plugin remounts inside it and rebuilds viewport
      // marks synchronously from the engine's warm token cache — the first
      // draw is already highlighted.
      //
      // StateEffect.reconfigure.of(compartment.of(x)) does NOT move an
      // existing compartment — values persist from the parked state. Diff
      // view sets this pane read-only before the path swap, so a dirty tab
      // is parked with editable=false; without compartment.reconfigure the
      // restored Code surface stays contenteditable=false and keystrokes
      // never land (reverting a restored stash cannot clear the dirty dot).
      // Build each slot once and feed it to both the new config (keeps the
      // compartment in the map) and reconfigure (the value that actually wins).
      let next = cached.state
      let reconfigured = false
      try {
        const slots = extensionSlots(filePath, ro, themeKey, prefs)
        next = cached.state.update({
          effects: [
            StateEffect.reconfigure.of(buildExtensions(filePath, slots)),
            ...slotReconfigureEffects(slots),
            cached.scroll,
          ],
        }).state
        reconfigured = true
      } catch { /* keep parked state */ }
      if (view) view.setState(next)
      else view = new EditorView({ parent: host, state: next })
      restoredLive = true
      pendingViewState = null
      syncRestoredState(filePath, initialDoc, ro, cached, reconfigured, focus)
      scheduleResync(filePath)
      return
    }
    // Dirty bit before setState so a remount of a restored stash does not
    // flash dirty=false (parent would drop overlaySplitStash). First mount
    // has no EditorView — never read view.state here.
    const state = buildState(initialDoc, filePath, ro, themeKey, prefs)
    const snap = bufferMountState(initialDoc, savedDoc)
    if (snap.dirty) {
      cleanDoc = snap.clean
      setDirty(true)
    } else {
      rememberClean(state.doc)
    }

    mountedRO = ro
    mountedDoc = initialDoc
    if (view) view.setState(state)
    else view = new EditorView({ parent: host, state })

    ensureLanguage(filePath)
    ensureKeymap(prefs.keymap)
    setEditorFindings(view, findingMarks)
    setEditorDiffBase(view, get(editorPrefs).diffGutter ? (baseDoc ?? null) : null)
    if (pendingViewState) {
      applyPending(filePath, initialDoc.length === 0)
      // Applied (non-empty doc) — Observatory's post-tick restoreViewState
      // must not dispatch a second scrollIntoView on top of it.
      if (!pendingViewState) restoredLive = true
    } else {
      scheduleResync(filePath)
    }
    const parsePath = filePath
    requestAnimationFrame(() => {
      if (!view || mountedPath !== parsePath) return
      forceParsing(view, view.viewport.to, 50)
    })
  }

  /**
   * Load this file's grammar if it isn't resident yet and swap it in when it
   * lands — provided the user hasn't navigated on in the meantime.
   */
  function ensureLanguage(filePath: string) {
    if (!view) return
    const sync = languageForPathSync(filePath)
    if (sync && langCompartment.get(view.state) === sync) return
    const pending = loadLanguageForPath(filePath)
    if (!pending) return
    void pending.then((ext) => {
      if (!view || mountedPath !== filePath) return
      const next = ext ?? languageForPathSync(filePath)
      // Resident grammar already in this view: reconfigure tears down the TM
      // ViewPlugin and a scrollSnapshot during that swap throws
      // "No tile at position". Diff→Code still reconfigures because the Code
      // pane's compartment is empty until this runs.
      if (ext === null && langCompartment.get(view.state) === next) return
      const snap = settleScroll ?? safeScrollSnapshot(view)
      const effects = [langCompartment.reconfigure(next)]
      if (snap) effects.push(snap)
      try {
        view.dispatch({ effects })
      } catch { /* tile map mid-rebuild */ }
      view.requestMeasure()
    })
  }

  /**
   * Scroll to a 1-based line (outline, findings, @file chips). Selects the
   * line, the inclusive line range, or a column span when startCol/endCol
   * are set (partial-line ⌘L chips).
   */
  export function scrollToLine(line: number, endLine = 0, startCol = 0, endCol = 0) {
    if (!view || line < 1) return
    // Explicit navigation supersedes any in-flight scroll restore.
    settleScroll = null
    const range = mentionRangeFromLoc(view.state.doc, {
      startLine: line,
      endLine: endLine > 0 ? endLine : undefined,
      startCol: startCol > 0 ? startCol : undefined,
      endCol: endCol > 0 ? endCol : undefined,
    })
    if (!range) return
    view.dispatch({
      selection: { anchor: range.from, head: range.to },
      effects: EditorView.scrollIntoView(range.from, { y: 'center' }),
    })
    view.focus()
  }

  export function focus() {
    if (view) focusPreservingScroll(view, settleScroll ?? undefined, () => stillThisPath(mountedPath))
  }

  /** Pixel scroll after layout. Assigning before measure clamps to 0; this
   *  runs on the settled scroller then remeasures so clicks match the paint. */
  export function restoreScrollTop(top: number) {
    if (!view || !Number.isFinite(top) || top < 0) return
    const path = mountedPath
    const apply = () => {
      if (!stillThisPath(path) || !view) return
      view.scrollDOM.scrollTop = top
      view.requestMeasure()
    }
    apply()
    requestAnimationFrame(() => requestAnimationFrame(apply))
  }

  export function hasFocus(): boolean {
    return !!view?.hasFocus
  }

  /** Caret + scroll for session restore. Null when the view is not mounted. */
  export function getViewState(): Omit<FileViewState, 'viewMode'> | null {
    if (!view) return null
    const cap = captureFileViewState(view, 'code')
    if (!cap) return null
    const { viewMode: _mode, ...caret } = cap
    return caret
  }

  /** Restore caret/scroll after a remount or Preview→Code. No-op when the
   *  switch restored live cached state — already where the user left it. */
  export function restoreViewState(vs: FileViewState) {
    if (restoredLive) {
      restoredLive = false
      if (view) {
        focusPreservingScroll(view, settleScroll ?? undefined, () => stillThisPath(mountedPath))
        view.requestMeasure()
      }
      return
    }
    pendingViewState = vs
    if (view) applyPending(mountedPath, view.state.doc.length === 0)
  }

  /** Drop a cached tab so the next mount rebuilds from `doc` (reload / discard). */
  export function forgetPath(filePath: string) {
    tabs.delete(filePath)
    // The buffer itself is being discarded, so drop the engine document even
    // if this pane still shows the file. The fingerprint check cannot carry
    // this: it samples part of the text, so a same-length edit on an unsampled
    // line would reuse the pre-reload lines. The plugin re-opens from the new
    // text on its next paint.
    tmForgetDocs([tmDocIdFor(parkPane, filePath)], true)
  }

  /**
   * Replace the live document with `text` as a fresh state — no undo of a
   * discarded buffer. Used by Reload from disk after caches are dropped.
   * `expectPath` must match the mounted file; a stale call during an in-flight
   * tab switch is a no-op (the incoming mount will pick up the new `doc`).
   */
  export function replaceDoc(text: string, expectPath?: string) {
    if (!view || !mountedPath || !host) return
    if (expectPath && mountedPath !== expectPath) return
    tabs.delete(mountedPath)
    lastEmitted = text
    setDirty(false)
    mountedDoc = text
    pendingViewState = null
    restoredLive = false
    settleScroll = null
    const state = buildState(text, mountedPath, mountedRO ?? false, get(editorTheme), get(editorPrefs))
    view.setState(state)
    ensureLanguage(mountedPath)
    ensureKeymap(get(editorPrefs).keymap)
    setEditorFindings(view, findingMarks)
    setEditorDiffBase(view, get(editorPrefs).diffGutter ? (baseDoc ?? null) : null)
    scheduleResync(mountedPath)
  }

  /** Inactive-tab snapshots — Observatory flushes these into tabBuffers on workspace switch. */
  export function cachedBuffers(): { path: string; doc: string; dirty: boolean }[] {
    const out: { path: string; doc: string; dirty: boolean }[] = []
    for (const [p, snap] of tabs) {
      out.push({ path: p, doc: snap.state.doc.toString(), dirty: snap.dirty })
    }
    return out
  }

  /** Open or close the find/replace panel (same as ⌘F / Escape). */
  export function toggleSearch(): boolean {
    if (!view) return false
    return searchPanelOpen(view.state) ? closeSearchPanel(view) : openSearchPanel(view)
  }

  export function isSearchOpen(): boolean {
    return view ? searchPanelOpen(view.state) : false
  }

  /**
   * Current buffer text without touching the emit debounce. Persist and stash
   * reads must not swallow pending keystrokes — those still need to land in
   * the parent's `draft` for Diff / Preview / a later save from Diff.
   */
  export function peekDoc(): string {
    return view?.state.doc.toString() ?? doc
  }

  /**
   * The authoritative current text. Flushes any pending debounced emit so the
   * parent stores what it gets back (this is the save path) and the resulting
   * `doc` prop is recognised as an echo rather than an on-disk change.
   */
  export function getDoc(): string {
    if (!view) return doc
    flushEmit()
    return lastEmitted ?? view.state.doc.toString()
  }

  /** Mark the buffer clean — the parent calls this once a save lands. */
  export function markSaved() {
    if (view) rememberClean(view.state.doc)
    else setDirty(false)
  }

  /** Snapshot the focused editor for Recur Bundle command runners. */
  export function getContext(): EditorContext {
    if (!view) {
      return {
        selectedText: '',
        document: doc,
        currentLine: '',
        lineNumber: 1,
        column: 1,
        currentWord: '',
        scope: scopeForPath(path),
        tabSize: 4,
        softTabs: true,
        path,
      }
    }
    const state = view.state
    const sel = state.selection.main
    const selectedText = state.doc.sliceString(sel.from, sel.to)
    const head = sel.head
    const line = state.doc.lineAt(head)
    const prefs = get(editorPrefs)
    const indent = resolveEditorIndent(prefs, languageIdForPath(path))
    const softTabs = indent.startsWith('spaces')
    const tabSize = indent === 'spaces-2' ? 2 : indent === 'spaces-4' ? 4 : prefs.tabSize
    return {
      selectedText,
      document: state.doc.toString(),
      currentLine: line.text,
      lineNumber: line.number,
      column: head - line.from + 1,
      currentWord: wordAt(view, head),
      scope: scopeForPath(path),
      tabSize,
      softTabs,
      path,
    }
  }

  /**
   * 1-based line/column span for a non-empty selection (Cmd+L → @file chip).
   * Empty / collapsed caret → null (caller attaches the whole file).
   */
  export function getMentionSelection(): MentionSelection | null {
    if (!view) return null
    return mentionSelectionFromState(view.state)
  }

  function expandTabSnippet(v: EditorView): boolean {
    const filePath = mountedPath || path
    const ok = tryExpandSnippet(v, scopeForPath(filePath), filePath, (matches, from, to) => {
      openKeybindingMenu(
        matches.map((s) => ({
          id: s.id,
          label: s.label,
          category: s.bundleName || 'Snippets',
          order: s.order,
        })),
        (item) => {
          const sn = matches.find((s) => s.id === item.id)
          if (!sn || !view) return
          applySnippet(view, sn, from, to, '')
          lastEmitted = view.state.doc.toString()
          syncDirtyFrom(view.state.doc)
        },
      )
    })
    if (ok) {
      lastEmitted = v.state.doc.toString()
      syncDirtyFrom(v.state.doc)
    }
    return ok
  }

  /** Insert a bundle snippet over the current selection (keyEquivalent wrap). */
  export function applyBundleSnippet(sn: RegisteredSnippet): string | null {
    if (!view) return null
    const sel = view.state.selection.main
    applySnippet(view, sn, sel.from, sel.to)
    const next = view.state.doc.toString()
    lastEmitted = next
    syncDirtyFrom(view.state.doc)
    return next
  }

  /**
   * Apply a command's stdout using the declared output routing. Returns the
   * new document text when the buffer changed (so the parent can sync draft).
   */
  export function applyOutput(
    routing: OutputRouting | string,
    text: string,
    inputMode?: string,
  ): string | null {
    if (!view) return null
    const state = view.state
    const sel = state.selection.main
    const docLen = state.doc.length

    // selectResult: keep the replacement highlighted so chained ⌘⇧P commands
    // still have a selection (and so the edit doesn't feel like it "undid" one).
    const replaceRange = (from: number, to: number, insert: string, selectResult: boolean) => {
      const end = from + insert.length
      view!.dispatch({
        changes: { from, to, insert },
        selection: selectResult
          ? { anchor: from, head: end }
          : { anchor: end },
      })
      const next = view!.state.doc.toString()
      lastEmitted = next
      syncDirtyFrom(view!.state.doc)
      return next
    }

    switch (routing) {
      case 'discard':
        return null
      case 'replaceSelection':
        return replaceRange(sel.from, sel.to, text, true)
      case 'replaceDocument':
        return replaceRange(0, docLen, text, false)
      case 'replaceInput':
        if (sel.from !== sel.to) return replaceRange(sel.from, sel.to, text, true)
        if (inputMode === 'word') {
          const word = state.wordAt(sel.head)
          if (word) return replaceRange(word.from, word.to, text, true)
        }
        if (inputMode === 'line') {
          const line = state.doc.lineAt(sel.head)
          return replaceRange(line.from, line.to, text, true)
        }
        return replaceRange(0, docLen, text, false)
      case 'insertText':
        return replaceRange(sel.from, sel.to, text, true)
      case 'insertSnippet': {
        const word = inputMode === 'word' ? state.wordAt(sel.head) : null
        const line = inputMode === 'line' ? state.doc.lineAt(sel.head) : null
        const { from, to } = commandSnippetRange(sel.from, sel.to, inputMode, word, line, docLen)
        try {
          snippet(toCodeMirrorSnippet(text))(view, null as never, from, to)
          syncDirtyFrom(view.state.doc)
          return view.state.doc.toString()
        } catch {
          return replaceRange(from, to, text, true)
        }
      }
      default:
        return null
    }
  }

  onMount(() => {
    unsubTheme = editorTheme.subscribe((key) => {
      if (!view) return
      view.dispatch({
        effects: themeCompartment.reconfigure(editorThemeExtension(key)),
      })
    })
    unsubPrefs = editorPrefs.subscribe((prefs) => {
      if (!view) return
      view.dispatch({ effects: prefsEffects(prefs, mountedPath || path) })
      ensureKeymap(prefs.keymap)
      view.requestMeasure()
    })
    unsubSyntax = syntaxModeEpoch.subscribe(() => {
      if (!view || !mountedPath) return
      ensureLanguage(mountedPath)
    })
    // A parked tab is an EditorState captured under the previous theme. Its
    // extension tree and any state derived from it predate the new palette,
    // and restoring one is what left inactive tabs looking stale. Theme
    // changes are rare, so drop the parked states outright: the next switch
    // rebuilds that tab from scratch, which is the close-and-reopen path
    // that repaints correctly. Dirty tabs are kept — unsaved text outranks
    // colour, and they re-highlight on their next transaction anyway.
    unsubColorMap = onColorMapChange(() => {
      // Chrome (background, gutter, selection) is built once per theme key, so
      // it goes stale when the key is unchanged but the theme is not: mounted
      // before the bundle themes registered (bare dark/light fallback), or the
      // active theme re-read by Reload Bundles. The store cannot signal either
      // (setting it to its current value does not notify), so recover here.
      if (view) {
        const key = get(editorTheme)
        try {
          view.dispatch({ effects: themeCompartment.reconfigure(editorThemeExtension(key)) })
        } catch { /* tile map mid-rebuild */ }
      }
      for (const [p, snap] of [...tabs]) {
        if (!snap.dirty) tabs.delete(p)
      }
      pruneAllSessionTabParks<TabSnapshot>((snap) => snap.dirty)
    })
    unsubGrammar = grammarCatalogEpoch.subscribe(() => {
      if (!view || !mountedPath) return
      ensureLanguage(mountedPath)
    })
  })

  onDestroy(() => {
    unsubTheme?.()
    unsubPrefs?.()
    unsubColorMap?.()
    unsubGrammar?.()
    unsubSyntax?.()
    if (emitTimer) clearTimeout(emitTimer)
    emitTimer = undefined
    if (resyncTimer != null) cancelAnimationFrame(resyncTimer)
    if (mountedSession && parkedOutgoing !== mountedSession) parkSession(mountedSession)
    destroyAllViews()
  })

  // Mount / remount here (not only in onMount) so the first paint always uses the
  // current readOnly prop — onMount + a skipped $effect could leave a stale RO
  // editor when the parent flips writable after open.
  $effect(() => {
    const nextPath = path
    const nextRO = readOnly
    const nextDoc = doc
    const nextSession = sessionKey
    if (!host) return
    const sessionSwap = nextSession !== mountedSession
    if (sessionSwap) {
      adoptSession(nextSession, nextPath)
    }
    if (!view || nextPath !== mountedPath) {
      applyingExternal = true
      try {
        pendingViewState = untrack(() => viewState) ?? null
        mountEditor(nextDoc, nextPath, nextRO, get(editorTheme), get(editorPrefs), !sessionSwap)
      } finally {
        applyingExternal = false
      }
      return
    }
    // Same path, but the incoming doc differs from what's on screen — the repo
    // state moved under us (e.g. a coding agent edited this file on disk while a
    // human/loop had it open). Typing also round-trips through this prop (the
    // parent mirrors our debounced emit back into `doc`), so isLocalEcho has to
    // rule that out first — it does so by reference, without re-serialising.
    if (nextDoc !== mountedDoc && !isLocalEcho(nextDoc)) {
      applyingExternal = true
      try {
        const live = view.state.doc
        if (nextDoc.length === live.length && nextDoc === live.toString()) {
          // Same text, new string identity (hydrate / live-refresh). A full-doc
          // swap + scrollSnapshot here is what desynced the height map so
          // clicks landed on a line way above the pointer.
          lastEmitted = nextDoc
          rememberClean(live)
        } else {
          // Preserve the viewport and (best-effort) cursor position across the
          // full-doc swap, and keep it OUT of the undo history: this fires on
          // every live-refresh while an agent edits the open file, and without
          // addToHistory(false) a reflexive Cmd+Z reverts the agent's on-disk
          // change into a dirty local edit the user could then save, clobbering
          // the agent's work.
          const hydrating = live.length === 0 && nextDoc.length > 0
          const snap = settleScroll ?? safeScrollSnapshot(view)
          settleScroll = null
          replaceDocKeepingScroll(view, nextDoc, snap)
          if (hydrating && pendingViewState) {
            applyPending(mountedPath)
          } else {
            view.requestMeasure()
          }
          // Deliberately NOT forceParsing here. This path runs on every
          // live-refresh cycle while an agent edits the open file, and a
          // synchronous parse budget on that cadence is a periodic stall;
          // CodeMirror's own idle-time parsing catches the viewport up without
          // blocking the keyboard.
          lastEmitted = nextDoc
          rememberClean(view.state.doc)
        }
      } finally {
        applyingExternal = false
      }
      mountedDoc = nextDoc
    } else {
      mountedDoc = nextDoc
    }
    // Path unchanged — only touch readOnly/editable. Reconfiguring the language
    // pack on every parent render was wiping syntax highlighting after save.
    if (nextRO === mountedRO) return
    mountedRO = nextRO
    view.dispatch({
      effects: [
        roCompartment.reconfigure(EditorState.readOnly.of(nextRO)),
        editableCompartment.reconfigure(EditorView.editable.of(!nextRO)),
      ],
    })
    view.requestMeasure()
  })

  $effect(() => {
    const marks = findingMarks
    if (!view) return
    try {
      setEditorFindings(view, marks)
    } catch { /* tile map mid-rebuild */ }
  })

  $effect(() => {
    const enabled = $editorPrefs.diffGutter
    const base = enabled ? baseDoc : null
    if (!view) return
    try {
      setEditorDiffBase(view, base ?? null)
    } catch { /* tile map mid-rebuild */ }
  })

  // savedDoc often lands after a cached tab restore (hydrateFile's await).
  // Re-baseline so undo-to-stash-time-disk does not look clean vs a newer file.
  $effect(() => {
    const nextSaved = savedDoc
    const nextPath = path
    if (nextSaved == null) return
    if (!view || mountedPath !== nextPath) return
    applySavedBaseline(nextSaved)
  })
</script>

<div class="cm-host" bind:this={host}></div>

<style>
  .cm-host {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }
  .cm-host :global(.cm-editor) {
    flex: 1;
    height: 100%;
  }
</style>
