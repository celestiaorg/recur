<script lang="ts">
  // Unified merge view for observatory Diff mode — @codemirror/merge compares
  // current worktree content against the base (spin-off) document.
  import { onMount, onDestroy } from 'svelte'
  import { EditorView, keymap } from '@codemirror/view'
  import { EditorState, Compartment, Transaction, Prec, type Extension } from '@codemirror/state'
  import { historyKeymap, indentWithTab, toggleComment } from '@codemirror/commands'
  import { nextSnippetField, prevSnippetField } from '@codemirror/autocomplete'
  import { recurDefaultKeymap } from '../lib/cm-keymap'
  import {
    captureEditorHistory, restoreEditorState, editorHistoryKey, editorHistoryExtension,
  } from '../lib/cm-editor-history'
  import { foldKeymap, indentOnInput, indentUnit } from '@codemirror/language'
  import {
    highlightSelectionMatches, openSearchPanel, closeSearchPanel,
    selectNextOccurrence, searchPanelOpen, getSearchQuery, setSearchQuery,
  } from '@codemirror/search'
  import { pointerMouseSelection } from '../lib/cm-pointer-pos'
  import { mentionRangeFromLoc, mentionSelectionFromState, type MentionSelection } from '../lib/cm-mention-sel'
  import { recurSearchExtension, recurSearchKeymap } from '../lib/cm-search-panel'
  import { recurGotoLine } from '../lib/cm-goto-line'
  import { insertNewlineBetweenPairs } from '../lib/cm-lang-data'
  import { unifiedMergeView } from '@codemirror/merge'
  import { mergeRevertHover, revertHunkCommand } from '../lib/cm-merge-revert'
  import { get } from 'svelte/store'
  import { languageForPathSync, loadLanguageForPath, languageIdForPath } from '../lib/cm-lang'
  import { lineOrientedDiff } from '../lib/cm-line-diff'
  import { suppressFullLineWordMarks } from '../lib/cm-merge-full-line-marks'
  import { suppressTmTokenBackgrounds } from '../lib/cm-tm-paint'
  import {
    editorChromeExtension, editorThemeExtension, editorThemeKind,
  } from '../lib/cm-themes'
  import { onColorMapChange } from '../lib/tm-registry'
  import { syntaxModeEpoch } from '../lib/syntax-mode'
  import { editorFeatureExtensions } from '../lib/cm-editor-prefs'
  import {
    editorTheme, editorPrefs, editorFontStack, resolveEditorIndent,
    type EditorPrefs,
  } from '../lib/stores'
  import {
    captureFileViewState, scheduleApplyFileViewState,
    type FileViewState,
  } from '../lib/editor-view-state'
  import { scopeForPath } from '../lib/commands'
  import { applySnippet, prefetchSnippets, tryExpandSnippet } from '../lib/snippets'
  import { openKeybindingMenu } from '../lib/keybinding-menu'

  /** Editor catalog kind — independent of app chrome light/dark. */
  const edKind = $derived(editorThemeKind($editorTheme))

  let {
    doc = '',
    original = '',
    path = '',
    readOnly = true,
    onDocChange,
    onSave,
    onCopyLine,
    onSearchOpenChange,
    historyScope = '',
    viewState = null as FileViewState | null,
  }: {
    doc?: string
    original?: string
    path?: string
    readOnly?: boolean
    onDocChange?: (value: string) => void
    onSave?: () => void
    /** Click a gutter line number → copy `path:line` (same as Code). */
    onCopyLine?: (line: number) => void
    /** Fired when the Cmd+F find panel opens or closes. */
    onSearchOpenChange?: (open: boolean) => void
    /** Project/workspace prefix so two loops with the same relative path don't share undo. */
    historyScope?: string
    /** Caret/scroll to restore on first mount (the pane is keyed per path). */
    viewState?: FileViewState | null
  } = $props()

  let host: HTMLDivElement | undefined = $state()
  let view: EditorView | undefined
  const langCompartment = new Compartment()
  const themeCompartment = new Compartment()
  const chromeCompartment = new Compartment()
  const wrapCompartment = new Compartment()
  const indentCompartment = new Compartment()
  const mergeCompartment = new Compartment()
  const featuresCompartment = new Compartment()
  const roCompartment = new Compartment()
  const editableCompartment = new Compartment()
  let mountedPath = ''
  let mountedDoc = ''
  let mountedOrigKey = ''
  let mountedRO: boolean | undefined
  let applyingExternal = false
  /** Last text we published via onDocChange — used to ignore parent echo. */
  let lastEmitted = ''
  let unsubTheme: (() => void) | undefined
  let unsubPrefs: (() => void) | undefined
  let unsubColorMap: (() => void) | undefined
  let unsubSyntax: (() => void) | undefined
  let lastSearchOpen = false

  $effect(() => {
    const p = path
    if (!p) return
    prefetchSnippets(scopeForPath(p), p)
  })

  function origKey(orig: string): string {
    return `${orig.length}\0${orig.slice(0, 64)}\0${orig.slice(-64)}`
  }

  function indentString(indent: string): string {
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

  function mergeExtension(orig: string): Extension {
    return [
      unifiedMergeView({
        original: orig,
        mergeControls: false,
        gutter: true,
        // Word marks inside hunks — lineOrientedDiff refines replaces so these
        // aren't whole-line underlines.
        highlightChanges: true,
        // Line Myers (like git), not CM's default char Myers + scanLimit:500 —
        // that limiter collapses big edits into one whole-file hunk.
        diffConfig: { override: lineOrientedDiff },
        collapseUnchanged: { margin: 3, minSize: 6 },
      }),
      mergeRevertHover(),
      // When word marks would paint an entire line, tag it — CSS clears the fill.
      suppressFullLineWordMarks(),
      // String/token washes (e.g. Made of Code) fight green/red hunk fills.
      suppressTmTokenBackgrounds(),
      // Layout-only merge chrome. Colors (line washes, word fills, collapsed)
      // live in the host stylesheet — CM merge baseTheme (`&.cm-merge-b
      // .cm-changedLine` → rgba(100,160,128,.08)) beats EditorView.theme.
      EditorView.theme({
        '.cm-deletedChunk': {
          position: 'relative',
          boxSizing: 'border-box',
        },
        // Deleted lines are custom DOM (not .cm-line) — force full content width
        // so the chunk is one continuous band, not text-width strips.
        '.cm-deletedChunk .cm-deletedLine': {
          display: 'block',
          width: '100%',
          boxSizing: 'border-box',
        },
        '.cm-changeGutter': {
          paddingLeft: '0',
        },
        '.cm-changeGutter .cm-gutterElement.cm-changedLineGutter': {
          backgroundColor: 'transparent',
          boxShadow: 'inset -2px 0 0 var(--git-a)',
        },
        '.cm-changeGutter .cm-gutterElement.cm-deletedLineGutter': {
          backgroundColor: 'transparent',
          boxShadow: 'inset -2px 0 0 var(--git-d)',
        },
        '.cm-collapsedLines': {
          padding: '5px 14px',
          cursor: 'pointer',
          fontFamily: 'var(--mono)',
          fontSize: '10.5px',
          fontWeight: '500',
          letterSpacing: '0.02em',
          lineHeight: '1.4',
          userSelect: 'none',
          backgroundImage: 'none',
          '&:before': {
            content: '"·"',
            marginInlineEnd: '8px',
          },
          '&:after': {
            content: '"·"',
            marginInlineStart: '8px',
          },
        },
      }),
    ]
  }

  function historyKeyFor(filePath: string): string {
    return editorHistoryKey(historyScope, filePath)
  }

  function captureMountedHistory() {
    if (!view || !mountedPath) return
    captureEditorHistory(historyKeyFor(mountedPath), view)
  }

  /** Flush live undo into the per-file cache (rename, before the path prop changes). */
  export function captureHistory() {
    captureMountedHistory()
  }

  function mount(current: string, orig: string, filePath: string, ro: boolean, themeKey: string, prefs: EditorPrefs) {
    if (!host) return
    // This pane is remounted per path/base ({#key} in Observatory) — stash the
    // outgoing file's undo so re-entering Diff doesn't start a blank stack.
    captureMountedHistory()
    // Same-path rebuild (base content refreshed under us): the user's live
    // position outranks any saved viewState — hold it across the remount.
    let liveVS: FileViewState | null = null
    let liveContentFocus = false
    let liveActive: Element | null = null
    let liveSearchOpen = false
    let liveSearch: ReturnType<typeof getSearchQuery> | null = null
    let liveSearchField: 'search' | 'replace' | null = null
    let liveSearchSel = { start: 0, end: 0 }
    if (view && mountedPath === filePath) {
      liveVS = captureFileViewState(view, 'diff')
      liveContentFocus = view.hasFocus
      liveActive = document.activeElement
      liveSearchOpen = searchPanelOpen(view.state)
      liveSearch = getSearchQuery(view.state)
      if (
        liveActive instanceof HTMLInputElement
        && (liveActive.name === 'search' || liveActive.name === 'replace')
        && liveActive.closest('.cm-recur-search')
      ) {
        liveSearchField = liveActive.name
        liveSearchSel = {
          start: liveActive.selectionStart ?? 0,
          end: liveActive.selectionEnd ?? 0,
        }
      }
    }
    view?.destroy()
    mountedPath = filePath
    mountedDoc = current
    mountedOrigKey = origKey(orig)
    mountedRO = ro
    lastEmitted = current
    const extensions: Extension[] = [
      editorHistoryExtension(),
      indentOnInput(),
      Prec.high(keymap.of([{ key: 'Enter', run: insertNewlineBetweenPairs }])),
      highlightSelectionMatches(),
      pointerMouseSelection(),
      recurSearchExtension(),
      keymap.of([
        { key: 'Tab', run: nextSnippetField, shift: prevSnippetField },
        {
          key: 'Tab',
          run: (v) => {
            const filePath = mountedPath || path
            return tryExpandSnippet(v, scopeForPath(filePath), filePath, (matches, from, to) => {
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
                },
              )
            })
          },
        },
        indentWithTab,
        ...recurDefaultKeymap,
        ...recurSearchKeymap,
        ...historyKeymap,
        ...foldKeymap,
        { key: 'Mod-/', run: toggleComment },
        { key: 'Mod-d', run: selectNextOccurrence },
        {
          key: 'Mod-s',
          run: () => {
            onSave?.()
            return true
          },
        },
        { key: 'Mod-f', run: openSearchPanel },
        // ⌘G = find next (searchKeymap); go-to-line is Recur dialog on ⌥⌘G.
        { key: 'Mod-Alt-g', run: recurGotoLine },
        { key: 'Mod-Alt-z', run: revertHunkCommand },
        { key: 'Escape', run: closeSearchPanel },
      ]),
      EditorView.updateListener.of((u) => {
        const open = searchPanelOpen(u.state)
        if (open !== lastSearchOpen) {
          lastSearchOpen = open
          onSearchOpenChange?.(open)
        }
        if (applyingExternal) return
        if (u.docChanged) {
          const text = u.state.doc.toString()
          lastEmitted = text
          onDocChange?.(text)
        }
      }),
      roCompartment.of(EditorState.readOnly.of(ro)),
      editableCompartment.of(EditorView.editable.of(!ro)),
      themeCompartment.of(editorThemeExtension(themeKey)),
      chromeCompartment.of(chromeExtension(prefs)),
      wrapCompartment.of(wrapExtension(prefs.lineWrapping)),
      indentCompartment.of(indentExtensions(prefs, filePath)),
      // Same pref-driven chrome as Code (Ln, fold, active line, whitespace…).
      // Before the merge change gutter so Ln/fold sit on the left, SCM strip
      // against the code. B-side numbers only — deleted hunk widgets are not
      // in the doc (D-file-mention-span).
      featuresCompartment.of(featuresExtension(prefs)),
      langCompartment.of(languageForPathSync(filePath)),
      mergeCompartment.of(mergeExtension(orig)),
    ]
    const restored = restoreEditorState(historyKeyFor(filePath), current, extensions)
    view = new EditorView({
      parent: host,
      state: restored?.state ?? EditorState.create({
        doc: current,
        extensions,
      }),
    })
    // Position precedence: live view (same-path rebuild) > restored history
    // snapshot's scroll > saved per-file viewState (fresh mount of this pane).
    const vs = liveVS ?? (restored ? null : viewState)
    if (vs) {
      const v = view
      scheduleApplyFileViewState(v, vs, () => view === v && mountedPath === filePath)
    } else if (restored) {
      // Raw scrollTop is this pane's house style (see the doc-swap branch
      // below); re-apply after first layout since the merge view measures late.
      const { scrollTop, scrollLeft } = restored
      view.scrollDOM.scrollTop = scrollTop
      view.scrollDOM.scrollLeft = scrollLeft
      const restorePath = filePath
      requestAnimationFrame(() => {
        if (!view || mountedPath !== restorePath) return
        view.scrollDOM.scrollTop = scrollTop
        view.scrollDOM.scrollLeft = scrollLeft
      })
    }
    if (liveSearchOpen) openSearchPanel(view)
    if (liveSearch && !liveSearch.eq(getSearchQuery(view.state))) {
      view.dispatch({ effects: setSearchQuery.of(liveSearch) })
    }
    if (liveSearchField) {
      const field = view.dom.querySelector<HTMLInputElement>(`.cm-recur-search [name="${liveSearchField}"]`)
      if (field) {
        field.focus({ preventScroll: true })
        field.setSelectionRange(liveSearchSel.start, liveSearchSel.end)
      }
    } else if (liveContentFocus) {
      view.focus()
    } else if (
      liveActive instanceof HTMLElement
      && document.contains(liveActive)
      && document.activeElement !== liveActive
    ) {
      liveActive.focus({ preventScroll: true })
    }
    ensureLanguage(filePath)
  }

  /** True when `next` is text that originated here (local edit / parent echo). */
  function isLocalEcho(next: string): boolean {
    if (next === lastEmitted) return true
    if (!view) return false
    if (next.length !== view.state.doc.length) return false
    return next === view.state.doc.toString()
  }

  /** Swap in the grammar once it loads, unless the pane moved on meanwhile. */
  function ensureLanguage(filePath: string) {
    if (!view) return
    const sync = languageForPathSync(filePath)
    if (sync && langCompartment.get(view.state) === sync) return
    const pending = loadLanguageForPath(filePath)
    if (!pending) return
    void pending.then((ext) => {
      if (!view || mountedPath !== filePath) return
      const next = ext ?? languageForPathSync(filePath)
      if (ext === null && langCompartment.get(view.state) === next) return
      try {
        view.dispatch({ effects: langCompartment.reconfigure(next) })
      } catch { /* tile map mid-rebuild */ }
    })
  }

  export function getDoc(): string {
    return view?.state.doc.toString() ?? doc
  }

  /**
   * 1-based line/column span for a non-empty selection (Cmd+L → @file chip).
   * Maps onto the current (B-side) document — deleted hunk widgets are not
   * in the doc, so a highlight in Diff tags the live worktree lines.
   */
  export function getMentionSelection(): MentionSelection | null {
    if (!view) return null
    return mentionSelectionFromState(view.state)
  }

  /**
   * Scroll to a 1-based line or column span so Diff participates in @file
   * chip clicks the same way Code does.
   */
  export function scrollToLine(line: number, endLine = 0, startCol = 0, endCol = 0) {
    if (!view || line < 1) return
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

  /** Caret + scroll so Diff mode participates in per-file view restore. */
  export function getViewState(): Omit<FileViewState, 'viewMode'> | null {
    if (!view) return null
    const cap = captureFileViewState(view, 'diff')
    if (!cap) return null
    const { viewMode: _mode, ...caret } = cap
    return caret
  }

  export function focus() {
    view?.focus()
  }

  /** Open or close the find/replace panel (same as ⌘F / Escape). */
  export function toggleSearch(): boolean {
    if (!view) return false
    return searchPanelOpen(view.state) ? closeSearchPanel(view) : openSearchPanel(view)
  }

  export function isSearchOpen(): boolean {
    return view ? searchPanelOpen(view.state) : false
  }

  onMount(() => {
    const prefs = get(editorPrefs)
    const themeKey = get(editorTheme)
    mount(doc, original, path, readOnly, themeKey, prefs)
    unsubTheme = editorTheme.subscribe((key) => {
      view?.dispatch({ effects: themeCompartment.reconfigure(editorThemeExtension(key)) })
    })
    // Same catch-up as CodeEditor: a session restored straight into Diff can
    // mount before the bundle themes register, and Reload Bundles can re-read
    // the active theme. The store cannot signal either (setting it to its
    // current value does not notify).
    unsubColorMap = onColorMapChange(() => {
      if (!view) return
      try {
        view.dispatch({ effects: themeCompartment.reconfigure(editorThemeExtension(get(editorTheme))) })
      } catch { /* tile map mid-rebuild */ }
    })
    unsubPrefs = editorPrefs.subscribe((p) => {
      view?.dispatch({
        effects: [
          chromeCompartment.reconfigure(chromeExtension(p)),
          wrapCompartment.reconfigure(wrapExtension(p.lineWrapping)),
          indentCompartment.reconfigure(indentExtensions(p, path)),
          featuresCompartment.reconfigure(featuresExtension(p)),
        ],
      })
    })
    unsubSyntax = syntaxModeEpoch.subscribe(() => {
      if (!view || !mountedPath) return
      ensureLanguage(mountedPath)
    })
  })

  onDestroy(() => {
    unsubTheme?.()
    unsubColorMap?.()
    unsubPrefs?.()
    unsubSyntax?.()
    captureMountedHistory()
    view?.destroy()
    view = undefined
  })

  // Remount when path, base, or the current doc changes from outside (agent /
  // disk). collapseUnchanged only inits on EditorState.create — an in-place
  // whole-document replace maps those widgets away and paints the full file
  // until Code → Diff remounts. Map undo onto the new buffer first so
  // restoreEditorState can keep ⌘Z (it drops history when the doc differs).
  // isLocalEcho skips remount on parent echoes of local edits (cursor).
  // Read-only toggles reconfigure compartments (no remount).
  // Wait for onMount's first mount (`view`) so this effect doesn't race it.
  $effect(() => {
    const nextPath = path
    const nextOrig = original
    const nextRO = readOnly
    const nextDoc = doc
    if (!view || !host) return
    const nextOrigKey = origKey(nextOrig)
    if (nextPath !== mountedPath || nextOrigKey !== mountedOrigKey) {
      applyingExternal = true
      try {
        mount(nextDoc, nextOrig, nextPath, nextRO, get(editorTheme), get(editorPrefs))
      } finally {
        applyingExternal = false
      }
      return
    }
    if (nextRO !== mountedRO) {
      mountedRO = nextRO
      view.dispatch({
        effects: [
          roCompartment.reconfigure(EditorState.readOnly.of(nextRO)),
          editableCompartment.reconfigure(EditorView.editable.of(!nextRO)),
        ],
      })
    }
    if (nextDoc !== mountedDoc && !isLocalEcho(nextDoc)) {
      applyingExternal = true
      try {
        if (view.state.doc.toString() !== nextDoc) {
          const prevHead = view.state.selection.main.head
          view.dispatch({
            changes: { from: 0, to: view.state.doc.length, insert: nextDoc },
            selection: { anchor: Math.min(prevHead, nextDoc.length) },
            annotations: Transaction.addToHistory.of(false),
          })
        }
        mount(nextDoc, nextOrig, nextPath, nextRO, get(editorTheme), get(editorPrefs))
      } finally {
        applyingExternal = false
      }
      return
    }
    mountedDoc = nextDoc
  })
</script>

<div
  class="cm-host merge-host"
  class:ed-dark={edKind === 'dark'}
  class:ed-light={edKind === 'light'}
  class:ro={readOnly}
  bind:this={host}
></div>

<style>
  .merge-host {
    flex: 1;
    min-height: 0;
    overflow: hidden;
  }
  .merge-host :global(.cm-editor) {
    height: 100%;
  }
  /* Collapsed unchanged — keyed to editor theme kind (not app chrome / .cm-dark).
     !important beats CM merge baseTheme light/dark gradients.
     No display:flex + flex-grow (thrashes block-widget measure). */
  .merge-host.ed-dark :global(.cm-collapsedLines) {
    color: rgba(230, 228, 225, 0.62) !important;
    background: rgba(255, 255, 255, 0.075) !important;
    background-image: none !important;
    border-top: 1px solid rgba(255, 255, 255, 0.1);
    border-bottom: 1px solid rgba(255, 255, 255, 0.1);
  }
  .merge-host.ed-dark :global(.cm-collapsedLines:hover) {
    color: rgba(230, 228, 225, 0.88) !important;
    background: rgba(255, 255, 255, 0.11) !important;
  }
  .merge-host.ed-light :global(.cm-collapsedLines) {
    color: rgba(26, 28, 32, 0.5) !important;
    background: rgba(0, 0, 0, 0.045) !important;
    background-image: none !important;
    border-top: 1px solid rgba(0, 0, 0, 0.08);
    border-bottom: 1px solid rgba(0, 0, 0, 0.08);
  }
  .merge-host.ed-light :global(.cm-collapsedLines:hover) {
    color: rgba(26, 28, 32, 0.78) !important;
    background: rgba(0, 0, 0, 0.07) !important;
  }
  /* Line washes — beat `&.cm-merge-b .cm-changedLine` / `&.cm-merge-a .cm-deletedChunk`.
     Active line must compose with the hunk tint; the base rule's !important
     would otherwise replace `--cm-active-line` and leave a leftover sliver. */
  .merge-host :global(.cm-changedLine),
  .merge-host :global(.cm-inlineChangedLine) {
    background-color: color-mix(in srgb, var(--git-a) 10%, transparent) !important;
  }
  .merge-host :global(.cm-activeLine.cm-changedLine),
  .merge-host :global(.cm-activeLine.cm-inlineChangedLine) {
    background-color: color-mix(in srgb, var(--git-a) 10%, var(--cm-active-line, transparent)) !important;
  }
  .merge-host :global(.cm-changeGutter) {
    padding-left: 0;
  }
  .merge-host :global(.cm-changeGutter .cm-gutterElement.cm-changedLineGutter) {
    background-color: transparent;
    box-shadow: inset -2px 0 0 var(--git-a);
  }
  .merge-host :global(.cm-changeGutter .cm-gutterElement.cm-deletedLineGutter) {
    background-color: transparent;
    box-shadow: inset -2px 0 0 var(--git-d);
  }
  .merge-host :global(.cm-deletedChunk) {
    background-color: color-mix(in srgb, var(--git-d) 10%, transparent) !important;
  }
  /* Word marks — kill CM merge underline gradients (`background: linear-gradient…
     bottom/100% 2px`). Must beat `&dark.cm-merge-b .cm-changedText` specificity. */
  .merge-host :global(.cm-changedText),
  .merge-host :global(.cm-insertedLine .cm-changedText) {
    background: color-mix(in srgb, var(--git-a) 12%, transparent) !important;
    background-image: none !important;
    border-radius: 2px;
    padding-block: 0.05em;
  }
  .merge-host :global(.cm-deletedChunk .cm-deletedText),
  .merge-host :global(.cm-deletedText) {
    background: color-mix(in srgb, var(--git-d) 14%, transparent) !important;
    background-image: none !important;
    border-radius: 2px;
    text-decoration: none;
  }
  /* Whole-line word marks — line wash is enough (see suppressFullLineWordMarks). */
  .merge-host :global(.cm-fullLineChange .cm-changedText) {
    background: transparent !important;
    background-image: none !important;
  }
  /* Deleted side: whole line wrapped in .cm-deletedText only → clear fill. */
  .merge-host :global(.cm-deletedLine:not(:has(span:not(.cm-deletedText))) .cm-deletedText) {
    background: transparent !important;
    background-image: none !important;
  }
  /* Revert chip — floats just below the hunk (hover or caret). Hidden when RO.
     Opaque editor-surface lift so hunk green/red cannot show through. */
  .merge-host.ro :global(.cm-revertHunk) {
    display: none;
  }
  .merge-host :global(.cm-revertHunk) {
    margin: 0;
    padding: 1px 8px;
    border-radius: 5px;
    font-family: var(--ui, inherit);
    font-size: 10.5px;
    font-weight: 500;
    letter-spacing: 0.02em;
    line-height: 16px;
    cursor: pointer;
  }
  .merge-host :global(.cm-revertHunk-key) {
    margin-left: 6px;
    opacity: 0.62;
    font-size: 10px;
    font-weight: 500;
    letter-spacing: 0.02em;
  }
  .merge-host.ed-dark :global(.cm-revertHunk) {
    color: rgba(230, 228, 225, 0.92) !important;
    background: color-mix(in srgb, var(--cm-tm-source-bg, #1a1a1a) 78%, white) !important;
    border: 1px solid rgba(255, 255, 255, 0.2) !important;
  }
  .merge-host.ed-dark :global(.cm-revertHunk:hover) {
    color: rgba(230, 228, 225, 0.98) !important;
    background: color-mix(in srgb, var(--cm-tm-source-bg, #1a1a1a) 68%, white) !important;
  }
  .merge-host.ed-light :global(.cm-revertHunk) {
    color: rgba(26, 28, 32, 0.86) !important;
    background: color-mix(in srgb, var(--cm-tm-source-bg, #f4f4f4) 86%, black) !important;
    border: 1px solid rgba(0, 0, 0, 0.16) !important;
  }
  .merge-host.ed-light :global(.cm-revertHunk:hover) {
    color: rgba(26, 28, 32, 0.94) !important;
    background: color-mix(in srgb, var(--cm-tm-source-bg, #f4f4f4) 78%, black) !important;
  }
</style>
