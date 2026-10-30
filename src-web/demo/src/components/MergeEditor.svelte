<script lang="ts">
  // Unified merge view for observatory Diff mode: @codemirror/merge compares
  // current worktree content against the base (spin-off) document.
  import { onMount, onDestroy } from 'svelte'
  import { get } from 'svelte/store'
  import { EditorView, keymap } from '@codemirror/view'
  import { EditorState, Compartment, type Extension } from '@codemirror/state'
  import { history, historyKeymap, indentWithTab, toggleComment } from '@codemirror/commands'
  import { foldKeymap, indentOnInput, indentUnit } from '@codemirror/language'
  import {
    highlightSelectionMatches, openSearchPanel, closeSearchPanel, selectNextOccurrence, searchPanelOpen,
  } from '@codemirror/search'
  import { unifiedMergeView } from '@codemirror/merge'
  import { recurDefaultKeymap } from '../lib/cm-keymap'
  import { recurSearchExtension, recurSearchKeymap } from '../lib/cm-search-panel'
  import { mergeRevertHover, revertHunkCommand } from '../lib/cm-merge-revert'
  import { lineOrientedDiff } from '../lib/cm-line-diff'
  import { suppressFullLineWordMarks } from '../lib/cm-merge-full-line-marks'
  import { editorChromeExtension, editorThemeExtension, editorThemeKind } from '../lib/cm-themes'
  import { editorFeatureExtensions } from '../lib/cm-editor-prefs'
  import { editorTheme, editorPrefs, EDITOR_FONT_STACK } from '../lib/stores'

  /** Editor catalog kind: independent of app chrome light/dark. */
  const edKind = $derived(editorThemeKind($editorTheme))

  let {
    doc = '',
    original = '',
    path = '',
    readOnly = true,
    onDocChange,
    onSearchOpenChange,
  }: {
    doc?: string
    original?: string
    path?: string
    readOnly?: boolean
    onDocChange?: (value: string) => void
    /** Fired when the Cmd+F find panel opens or closes. */
    onSearchOpenChange?: (open: boolean) => void
  } = $props()

  let host: HTMLDivElement | undefined = $state()
  let view: EditorView | undefined
  const themeCompartment = new Compartment()
  const roCompartment = new Compartment()
  let lastSearchOpen = false

  function readOnlyExtension(ro: boolean): Extension {
    return [EditorState.readOnly.of(ro), EditorView.editable.of(!ro)]
  }

  function mergeExtension(orig: string): Extension {
    return [
      unifiedMergeView({
        original: orig,
        mergeControls: false,
        gutter: true,
        // Word marks inside hunks: lineOrientedDiff refines replaces so these
        // aren't whole-line underlines.
        highlightChanges: true,
        // Line Myers (like git), not CM's default char Myers + scanLimit:500:
        // that limiter collapses big edits into one whole-file hunk.
        diffConfig: { override: lineOrientedDiff },
        collapseUnchanged: { margin: 3, minSize: 6 },
      }),
      mergeRevertHover(),
      // When word marks would paint an entire line, tag it: CSS clears the fill.
      suppressFullLineWordMarks(),
      // Layout-only merge chrome. Colors (line washes, word fills, collapsed)
      // live in the host stylesheet: CM merge baseTheme (`&.cm-merge-b
      // .cm-changedLine` → rgba(100,160,128,.08)) beats EditorView.theme.
      EditorView.theme({
        '.cm-deletedChunk': {
          position: 'relative',
          boxSizing: 'border-box',
        },
        // Deleted lines are custom DOM (not .cm-line): force full content width
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

  function reapplyTheme() {
    view?.dispatch({ effects: themeCompartment.reconfigure(editorThemeExtension(get(editorTheme))) })
  }

  export function scrollToLine(line: number) {
    if (!view || line < 1 || line > view.state.doc.lines) return
    const l = view.state.doc.line(line)
    view.dispatch({ selection: { anchor: l.from, head: l.to }, effects: EditorView.scrollIntoView(l.from, { y: 'center' }) })
    view.focus()
  }

  /** Open or close the find/replace panel (same as ⌘F / Escape). */
  export function toggleSearch(): boolean {
    if (!view) return false
    return searchPanelOpen(view.state) ? closeSearchPanel(view) : openSearchPanel(view)
  }

  export function isSearchOpen(): boolean {
    return view ? searchPanelOpen(view.state) : false
  }

  // Observatory keys this pane per path, so the doc, base and path are fixed
  // for the lifetime of one view.
  const unsubs: (() => void)[] = []
  onMount(() => {
    const prefs = get(editorPrefs)
    view = new EditorView({
      parent: host,
      state: EditorState.create({
        doc,
        extensions: [
          history(),
          indentOnInput(),
          highlightSelectionMatches(),
          recurSearchExtension(),
          keymap.of([
            indentWithTab,
            ...recurDefaultKeymap,
            ...recurSearchKeymap,
            ...historyKeymap,
            ...foldKeymap,
            { key: 'Mod-/', run: toggleComment },
            { key: 'Mod-d', run: selectNextOccurrence },
            { key: 'Mod-f', run: openSearchPanel },
            { key: 'Mod-Alt-z', run: revertHunkCommand },
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
          roCompartment.of(readOnlyExtension(readOnly)),
          themeCompartment.of(editorThemeExtension(get(editorTheme))),
          editorChromeExtension(prefs.fontSize, EDITOR_FONT_STACK, prefs.lineHeight, prefs.thickCaret),
          prefs.lineWrapping ? EditorView.lineWrapping : [],
          EditorState.tabSize.of(prefs.tabSize),
          indentUnit.of(/\.go$/.test(path) ? '\t' : '    '),
          editorFeatureExtensions(prefs),
          mergeExtension(original),
        ],
      }),
    })
    unsubs.push(editorTheme.subscribe(reapplyTheme))
  })

  onDestroy(() => {
    for (const u of unsubs) u()
    view?.destroy()
  })

  $effect(() => {
    const ro = readOnly
    view?.dispatch({ effects: roCompartment.reconfigure(readOnlyExtension(ro)) })
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
  /* Collapsed unchanged: keyed to editor theme kind (not app chrome / .cm-dark).
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
  /* Line washes: beat `&.cm-merge-b .cm-changedLine` / `&.cm-merge-a .cm-deletedChunk`.
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
  /* Word marks: kill CM merge underline gradients (`background: linear-gradient…
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
  /* Whole-line word marks: line wash is enough (see suppressFullLineWordMarks). */
  .merge-host :global(.cm-fullLineChange .cm-changedText) {
    background: transparent !important;
    background-image: none !important;
  }
  /* Deleted side: whole line wrapped in .cm-deletedText only → clear fill. */
  .merge-host :global(.cm-deletedLine:not(:has(span:not(.cm-deletedText))) .cm-deletedText) {
    background: transparent !important;
    background-image: none !important;
  }
  /* Revert chip: floats just below the hunk (hover or caret). Hidden when RO.
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
