<script lang="ts">
  // Recur-integrated CodeMirror 6 surface: view + edit in one place.
  import { onMount, onDestroy } from 'svelte'
  import { get } from 'svelte/store'
  import { EditorView, keymap } from '@codemirror/view'
  import { EditorState, Compartment, type Extension } from '@codemirror/state'
  import { history, historyKeymap, indentWithTab, toggleComment } from '@codemirror/commands'
  import { foldKeymap, indentOnInput, indentUnit } from '@codemirror/language'
  import {
    highlightSelectionMatches, openSearchPanel, closeSearchPanel, selectNextOccurrence, searchPanelOpen,
  } from '@codemirror/search'
  import { closeBrackets, closeBracketsKeymap } from '@codemirror/autocomplete'
  import { findingsGutterExtension, setEditorFindings, type EditorFindingMark } from '../lib/cm-findings-gutter'
  import { diffGutterExtension, setEditorDiffBase } from '../lib/cm-diff-gutter'
  import { recurDefaultKeymap } from '../lib/cm-keymap'
  import { recurSearchExtension, recurSearchKeymap } from '../lib/cm-search-panel'
  import { editorChromeExtension, editorThemeExtension } from '../lib/cm-themes'
  import { editorFeatureExtensions } from '../lib/cm-editor-prefs'
  import { editorTheme, editorPrefs, EDITOR_FONT_STACK } from '../lib/stores'

  let {
    doc = '',
    path = '',
    readOnly = true,
    findingMarks = [] as EditorFindingMark[],
    /** FileAtBase text for SCM gutter hints. `null` = not loaded yet. */
    baseDoc = null as string | null,
    onDocChange,
    onSearchOpenChange,
    /** Gutter marker click → open this finding in the Findings panel. */
    onOpenFinding,
  }: {
    doc?: string
    path?: string
    readOnly?: boolean
    findingMarks?: EditorFindingMark[]
    baseDoc?: string | null
    onDocChange?: (value: string) => void
    onSearchOpenChange?: (open: boolean) => void
    onOpenFinding?: (findingId: string) => void
  } = $props()

  let host: HTMLDivElement | undefined = $state()
  let view: EditorView | undefined
  const roCompartment = new Compartment()
  const themeCompartment = new Compartment()
  let mountedPath = ''
  let lastSearchOpen = false
  const onOpenFindingRef: { current: ((id: string) => void) | undefined } = { current: undefined }
  $effect(() => {
    onOpenFindingRef.current = onOpenFinding
  })

  function readOnlyExtension(ro: boolean): Extension {
    return [EditorState.readOnly.of(ro), EditorView.editable.of(!ro)]
  }

  function buildState(text: string, filePath: string, ro: boolean): EditorState {
    const prefs = get(editorPrefs)
    return EditorState.create({
      doc: text,
      extensions: [
        history(),
        indentOnInput(),
        closeBrackets(),
        highlightSelectionMatches(),
        recurSearchExtension(),
        keymap.of([
          ...closeBracketsKeymap,
          ...recurDefaultKeymap,
          ...recurSearchKeymap,
          ...historyKeymap,
          ...foldKeymap,
          indentWithTab,
          { key: 'Mod-/', run: toggleComment },
          { key: 'Mod-d', run: selectNextOccurrence },
          { key: 'Mod-f', run: openSearchPanel },
          { key: 'Escape', run: closeSearchPanel },
        ]),
        EditorView.updateListener.of((u) => {
          const open = searchPanelOpen(u.state)
          if (open !== lastSearchOpen) {
            lastSearchOpen = open
            onSearchOpenChange?.(open)
          }
          if (u.docChanged) onDocChange?.(u.state.doc.toString())
        }),
        themeCompartment.of(editorThemeExtension(get(editorTheme))),
        editorChromeExtension(prefs.fontSize, EDITOR_FONT_STACK, prefs.lineHeight, prefs.thickCaret),
        prefs.lineWrapping ? EditorView.lineWrapping : [],
        EditorState.tabSize.of(prefs.tabSize),
        indentUnit.of(/\.go$/.test(filePath) ? '\t' : '    '),
        findingsGutterExtension(onOpenFindingRef),
        editorFeatureExtensions(prefs),
        prefs.diffGutter ? diffGutterExtension() : [],
        roCompartment.of(readOnlyExtension(ro)),
      ],
    })
  }

  function reapplyTheme() {
    view?.dispatch({ effects: themeCompartment.reconfigure(editorThemeExtension(get(editorTheme))) })
  }

  /** Scroll to and select a 1-based line. */
  export function scrollToLine(line: number) {
    if (!view || line < 1 || line > view.state.doc.lines) return
    const l = view.state.doc.line(line)
    view.dispatch({ selection: { anchor: l.from, head: l.to }, effects: EditorView.scrollIntoView(l.from, { y: 'center' }) })
    view.focus()
  }

  export function focus() {
    view?.focus()
  }

  export function resync() {
    view?.requestMeasure()
  }

  /** Open or close the find/replace panel (same as ⌘F / Escape). */
  export function toggleSearch(): boolean {
    if (!view) return false
    return searchPanelOpen(view.state) ? closeSearchPanel(view) : openSearchPanel(view)
  }

  export function isSearchOpen(): boolean {
    return view ? searchPanelOpen(view.state) : false
  }

  onMount(() => editorTheme.subscribe(reapplyTheme))
  onDestroy(() => view?.destroy())

  $effect(() => {
    const nextPath = path
    const nextDoc = doc
    const ro = readOnly
    if (!host) return
    if (!view || nextPath !== mountedPath) {
      mountedPath = nextPath
      const state = buildState(nextDoc, nextPath, ro)
      if (view) view.setState(state)
      else view = new EditorView({ parent: host, state })
      setEditorFindings(view, findingMarks)
      if (get(editorPrefs).diffGutter) setEditorDiffBase(view, baseDoc)
      return
    }
    if (nextDoc !== view.state.doc.toString()) {
      view.dispatch({ changes: { from: 0, to: view.state.doc.length, insert: nextDoc } })
    }
    view.dispatch({ effects: roCompartment.reconfigure(readOnlyExtension(ro)) })
  })

  $effect(() => {
    const marks = findingMarks
    if (view) setEditorFindings(view, marks)
  })

  $effect(() => {
    const base = baseDoc
    if (view && get(editorPrefs).diffGutter) setEditorDiffBase(view, base)
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
