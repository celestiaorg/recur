<script lang="ts">
  // Compact CodeMirror surface for project setup/teardown scripts, dotenv,
  // and short markdown (plan body). Themed to the editor palette that matches
  // app chrome — not the selected editor theme.
  import { onMount, onDestroy } from 'svelte'
  import { get } from 'svelte/store'
  import { EditorView, keymap, placeholder as placeholderExt, lineNumbers, drawSelection } from '@codemirror/view'
  import { EditorState, Compartment } from '@codemirror/state'
  import { history, historyKeymap, indentWithTab } from '@codemirror/commands'
  import { recurDefaultKeymap } from '../lib/cm-keymap'
  import { bundlesEpoch } from '../lib/commands'
  import { editorChromeOnlyExtension, editorThemeIdForAppChrome } from '../lib/cm-themes'
  import { miniScriptHighlight } from '../lib/cm-mini-script-highlight'
  import { textmateLanguageData } from '../lib/cm-lang-data'
  import { isLightTheme, theme } from '../lib/stores'
  import { ensureTextMateBootstrap } from '../lib/tm-bootstrap'
  import { tmRootContentExtension } from '../lib/tm-theme'
  import {
    ensureChromePairedTmEngine,
    getParsedTheme,
    resolveScopeForPath,
  } from '../lib/tm-registry'

  let {
    value = '',
    lang = 'shell',
    placeholder = '',
    minLines = 4,
    maxLines = 16,
    fill = false,
    readonly = false,
    ariaLabel = '',
    onChange,
    onModEnter,
  }: {
    value?: string
    /** shell → source.shell; env → source.dotenv; md → text.html.markdown */
    lang?: 'shell' | 'env' | 'md'
    placeholder?: string
    minLines?: number
    maxLines?: number
    /** Stretch to the parent height instead of capping at maxLines. */
    fill?: boolean
    readonly?: boolean
    ariaLabel?: string
    onChange?: (value: string) => void
    onModEnter?: () => void
  } = $props()

  let host: HTMLDivElement | undefined = $state()
  let view: EditorView | undefined = $state()
  const langCompartment = new Compartment()
  const themeCompartment = new Compartment()
  const readOnlyCompartment = new Compartment()
  let lastEmitted = ''
  let applyingExternal = false
  let applyGen = 0

  // `.env` is a basename match in tm-registry (ext alone is "env").
  const path = $derived(lang === 'env' ? '.env' : lang === 'md' ? 'plan.md' : 'setup.sh')
  const prose = $derived(lang === 'md')

  const lineH = 18
  const padY = 16

  function layoutChrome() {
    const minH = minLines * lineH + padY
    const maxH = maxLines * lineH + padY
    const fontSize = prose ? 12.5 : 12
    return EditorView.theme({
      '&': {
        height: fill ? '100%' : 'auto',
        minHeight: `${minH}px`,
        maxHeight: fill ? '100%' : `${maxH}px`,
        fontSize: `${fontSize}px`,
      },
      '.cm-scroller': {
        fontFamily: prose ? 'inherit' : 'var(--mono)',
        lineHeight: String(lineH / fontSize),
        overflow: 'auto',
      },
      '.cm-content': {
        padding: fill ? '4px 0' : prose ? '6px 0' : '8px 0',
        minHeight: fill ? '100%' : `${minH - 2}px`,
      },
      '.cm-line': { padding: '0 10px 0 4px' },
      '.cm-gutters': {
        minHeight: `${minH - 2}px`,
      },
      '.cm-lineNumbers .cm-gutterElement': {
        minWidth: '2.2ch',
        padding: '0 8px 0 6px',
        fontSize: '10.5px',
      },
      '&.cm-focused': { outline: 'none' },
      '.cm-placeholder': { color: 'var(--faint)', fontStyle: 'italic' },
    })
  }

  function fallbackChrome() {
    const light = isLightTheme(get(theme))
    return [
      EditorView.darkTheme.of(!light),
      EditorView.theme({
        '&': { backgroundColor: 'transparent', color: 'var(--text)' },
        '.cm-gutters': {
          backgroundColor: 'transparent',
          border: 'none',
          color: 'var(--faint)',
        },
        '.cm-activeLineGutter': { color: 'var(--muted)', backgroundColor: 'transparent' },
        '.cm-activeLine': { backgroundColor: light ? 'rgba(0,0,0,.06)' : 'rgba(109,211,232,.06)' },
        '.cm-cursor, .cm-dropCursor': { borderLeftColor: 'var(--cyan)' },
        '&.cm-focused > .cm-scroller > .cm-selectionLayer .cm-selectionBackground, & > .cm-scroller > .cm-selectionLayer .cm-selectionBackground': {
          background: light ? 'rgba(0,0,0,.18)' : 'rgba(109,211,232,.18)',
        },
        '.cm-line ::selection, .cm-content ::selection': { backgroundColor: 'transparent' },
      }, { dark: !light }),
    ]
  }

  function readOnlyExt(on: boolean) {
    return [EditorState.readOnly.of(on), EditorView.editable.of(!on)]
  }

  function extensions() {
    const save = onModEnter
    return [
      history(),
      lineNumbers(),
      drawSelection(),
      EditorState.allowMultipleSelections.of(true),
      keymap.of([
        ...(save ? [{ key: 'Mod-Enter', run: () => { save(); return true } }] : []),
        ...recurDefaultKeymap,
        ...historyKeymap,
        indentWithTab,
      ]),
      EditorView.lineWrapping,
      layoutChrome(),
      langCompartment.of([]),
      themeCompartment.of(fallbackChrome()),
      readOnlyCompartment.of(readOnlyExt(readonly)),
      placeholder ? placeholderExt(placeholder) : [],
      ariaLabel ? EditorView.contentAttributes.of({ 'aria-label': ariaLabel }) : [],
      EditorView.updateListener.of((u) => {
        if (!u.docChanged || applyingExternal) return
        const text = u.state.doc.toString()
        lastEmitted = text
        onChange?.(text)
      }),
    ]
  }

  function stale(gen: number, appThemeKey: string): boolean {
    return gen !== applyGen || !view || get(theme) !== appThemeKey
  }

  async function applyPairedTheme() {
    const gen = ++applyGen
    const currentPath = path
    const appThemeKey = get(theme)
    try {
      await ensureTextMateBootstrap()
      if (stale(gen, appThemeKey)) return
      const themeId = editorThemeIdForAppChrome(appThemeKey)
      const parsed = getParsedTheme(themeId)
      const engine = await ensureChromePairedTmEngine(themeId)
      if (stale(gen, appThemeKey)) return
      const scope = resolveScopeForPath(currentPath)
      let grammarOk = false
      if (engine && scope) {
        try {
          grammarOk = await engine.ensureGrammar(scope)
        } catch (err) {
          console.error('mini script grammar failed', err)
        }
      }
      if (stale(gen, appThemeKey)) return
      const v = view
      if (!v) return
      const themeExt = parsed
        ? [editorChromeOnlyExtension(themeId), scope ? tmRootContentExtension(scope) : []]
        : fallbackChrome()
      const langExt = engine && scope && parsed && grammarOk
        ? [miniScriptHighlight(scope, engine, parsed, themeId), textmateLanguageData(scope)]
        : []
      v.dispatch({
        effects: [
          themeCompartment.reconfigure(themeExt),
          langCompartment.reconfigure(langExt),
        ],
      })
    } catch (err) {
      console.error('mini script theme failed', err)
      const v = view
      if (stale(gen, appThemeKey) || !v) return
      v.dispatch({ effects: themeCompartment.reconfigure(fallbackChrome()) })
    }
  }

  onMount(() => {
    if (!host) return
    view = new EditorView({
      parent: host,
      state: EditorState.create({
        doc: value ?? '',
        extensions: extensions(),
      }),
    })
    lastEmitted = value ?? ''
  })

  onDestroy(() => {
    view?.destroy()
    view = undefined
  })

  export function focus() {
    view?.focus()
  }

  // External reseeds (switching projects) — don't clobber in-progress typing.
  $effect(() => {
    const next = value ?? ''
    const v = view
    if (!v) return
    if (next === lastEmitted) return
    if (next === v.state.doc.toString()) {
      lastEmitted = next
      return
    }
    applyingExternal = true
    v.dispatch({
      changes: { from: 0, to: v.state.doc.length, insert: next },
    })
    lastEmitted = next
    applyingExternal = false
  })

  $effect(() => {
    const on = readonly
    const v = view
    if (!v) return
    v.dispatch({ effects: readOnlyCompartment.reconfigure(readOnlyExt(on)) })
  })

  $effect(() => {
    void $theme
    void $bundlesEpoch
    void path
    const v = view
    if (!v) return
    void applyPairedTheme()
  })
</script>

<div class="mini-ed" class:ro={readonly} class:prose class:fill bind:this={host}></div>

<style>
  .mini-ed {
    width: 100%;
    border: 1px solid var(--line);
    border-radius: 6px;
    overflow: hidden;
    /* App pane until the paired editor chrome lands (avoids a Charcoal flash on Nord etc.). */
    background: var(--pane);
    transition: border-color .12s ease;
  }
  .mini-ed:focus-within {
    border-color: rgba(242, 179, 61, .5);
  }
  .mini-ed.fill {
    display: flex;
    flex-direction: column;
    height: 100%;
    min-height: 0;
    border: none;
    border-radius: 0;
    background: transparent;
  }
  .mini-ed.fill:focus-within {
    border-color: transparent;
  }
  .mini-ed.fill :global(.cm-editor) {
    flex: 1;
    min-height: 0;
  }
  .mini-ed.ro {
    cursor: default;
  }
  .mini-ed.ro:focus-within {
    border-color: var(--line);
  }
</style>
