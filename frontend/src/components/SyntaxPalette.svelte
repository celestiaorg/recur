<script lang="ts">
  /**
   * Language-mode picker (editor footer / ⌘K "Change language mode").
   * Same overlay chrome as ⌘⇧P. Per-file pick is persisted; "Always open
   * *.ext as…" writes a file-type association (VS Code files.associations).
   */
  import { untrack } from 'svelte'
  import { syntaxModeOpen } from '../lib/stores'
  import { currentEditorFocus } from '../lib/commands'
  import { setChatOccluder } from '../lib/chat-occlusion'
  import { createHoverGate, hoverGateAllows, resetHoverGate } from '../lib/hoverGate'
  import { prepareFuzzy, fuzzyFilterPrepared } from '../lib/fuzzy'
  import {
    applyLanguageMode,
    detectScopeForPath,
    grammarCatalogEpoch,
    languageLabelForScope,
    listSyntaxLanguages,
    resolveScopeForPath,
  } from '../lib/tm-registry'
  import {
    SYNTAX_PLAIN,
    fileTypeAssociationForPath,
    setSyntaxForFileType,
    setSyntaxForPath,
    syntaxFileTypeOverride,
    syntaxModeEpoch,
    syntaxPathOverride,
    type FileTypeAssociation,
  } from '../lib/syntax-mode'
  import Icon from './Icon.svelte'
  import Kbd from './Kbd.svelte'

  type Row = {
    id: string
    label: string
    psub?: string
    current?: boolean
  }

  let q = $state('')
  let sel = $state(0)
  let inputEl = $state<HTMLInputElement | null>(null)
  let listEl = $state<HTMLDivElement | null>(null)
  let associate = $state(false)
  let path = $state('')
  let hoverGate = createHoverGate()
  let wasOpen = false

  $effect(() => {
    setChatOccluder('syntax-mode', $syntaxModeOpen)
    return () => setChatOccluder('syntax-mode', false)
  })

  $effect(() => {
    const open = $syntaxModeOpen
    if (open && !wasOpen) {
      untrack(() => {
        path = currentEditorFocus().path
        q = ''
        sel = 0
        associate = false
      })
      setTimeout(() => inputEl?.focus(), 0)
    }
    wasOpen = open
  })

  const langs = $derived.by(() => {
    void $grammarCatalogEpoch
    return listSyntaxLanguages()
  })

  const prepared = $derived(prepareFuzzy(langs, (l) => `${l.label} ${l.language} ${l.scopeName}`))

  const detected = $derived(path ? detectScopeForPath(path) : null)
  const currentScope = $derived.by(() => {
    void $syntaxModeEpoch
    return path ? resolveScopeForPath(path) : null
  })
  const pathOverride = $derived.by(() => {
    void $syntaxModeEpoch
    return path ? syntaxPathOverride(path) : undefined
  })
  const assoc: FileTypeAssociation | null = $derived(path ? fileTypeAssociationForPath(path) : null)
  const assocScope = $derived.by(() => {
    void $syntaxModeEpoch
    return assoc ? syntaxFileTypeOverride(assoc.key) : undefined
  })

  const actions = $derived.by((): Row[] => {
    if (!path) return []
    if (associate) {
      return [{
        id: 'plain',
        label: 'Plain Text',
        current: assocScope === SYNTAX_PLAIN,
      }]
    }
    const rows: Row[] = [
      {
        id: 'auto',
        label: 'Auto Detect',
        psub: detected ? languageLabelForScope(detected) : 'Plain Text',
        current: pathOverride === undefined && assocScope === undefined,
      },
      {
        id: 'plain',
        label: 'Plain Text',
        current: pathOverride === SYNTAX_PLAIN || (pathOverride === undefined && !currentScope),
      },
    ]
    if (assoc) {
      rows.push({
        id: 'assoc',
        label: `Always open ${assoc.label} as…`,
        psub: assocScope != null
          ? (assocScope ? languageLabelForScope(assocScope) : 'Plain Text')
          : undefined,
      })
      if (assocScope !== undefined) {
        rows.push({
          id: 'assoc-clear',
          label: `Clear ${assoc.label} association`,
        })
      }
    }
    return rows
  })

  const langRows = $derived.by((): Row[] => {
    const hits = q.trim()
      ? fuzzyFilterPrepared(prepared, q, 80).map((h) => h.item)
      : langs
    return hits.map((l) => ({
      id: l.scopeName,
      label: l.label,
      current: !associate && currentScope === l.scopeName,
    }))
  })

  const visibleActions = $derived.by(() => {
    const query = q.trim().toLowerCase()
    if (!query) return actions
    return actions.filter((r) =>
      r.label.toLowerCase().includes(query) || (r.psub ?? '').toLowerCase().includes(query),
    )
  })

  const flat = $derived<Row[]>([...visibleActions, ...langRows])

  $effect(() => {
    const max = Math.max(0, flat.length - 1)
    if (sel > max) sel = max
  })

  $effect(() => {
    void $syntaxModeOpen
    void flat
    resetHoverGate(hoverGate)
  })

  $effect(() => {
    void sel
    void flat
    queueMicrotask(() => {
      listEl?.querySelector<HTMLElement>('.pal-row.sel')?.scrollIntoView({ block: 'nearest' })
    })
  })

  function close() {
    syntaxModeOpen.set(false)
    associate = false
  }

  function pickAuto() {
    if (!path) return
    applyLanguageMode(path, null)
    close()
  }

  function pickScope(scope: string) {
    if (!path) return
    if (associate && assoc) {
      setSyntaxForFileType(assoc.key, scope)
      setSyntaxForPath(path, null)
    } else {
      applyLanguageMode(path, scope)
    }
    close()
  }

  function activate(id: string) {
    if (id === 'auto') pickAuto()
    else if (id === 'plain') pickScope(SYNTAX_PLAIN)
    else if (id === 'assoc') {
      associate = true
      q = ''
      sel = 0
    } else if (id === 'assoc-clear') {
      if (assoc) setSyntaxForFileType(assoc.key, null)
      close()
    } else {
      pickScope(id)
    }
  }

  function run(i: number) {
    const row = flat[i]
    if (row) activate(row.id)
  }

  function onRowMouseMove(e: MouseEvent, i: number) {
    if (hoverGateAllows(hoverGate, e.clientX, e.clientY)) sel = i
  }

  function onKey(e: KeyboardEvent) {
    if (e.metaKey || e.ctrlKey || e.altKey) return
    if (e.key === 'ArrowDown') {
      e.preventDefault()
      sel = Math.min(flat.length - 1, sel + 1)
    } else if (e.key === 'ArrowUp') {
      e.preventDefault()
      sel = Math.max(0, sel - 1)
    } else if (e.key === 'Enter') {
      e.preventDefault()
      run(sel)
    } else if (e.key === 'Escape') {
      e.preventDefault()
      if (associate) {
        associate = false
        q = ''
        sel = 0
      } else {
        close()
      }
    }
  }

  const placeholder = $derived(
    associate && assoc ? `Select language for ${assoc.label}…` : 'Select language mode…',
  )
  const enterHint = $derived(associate ? 'set association' : 'this file')
</script>

<div
  class="palette-bg"
  class:open={$syntaxModeOpen}
  onclick={(e) => { if (e.target === e.currentTarget) close() }}
  role="presentation"
>
  <div class="palette cmd-palette" role="dialog" aria-modal="true" aria-label="Change language mode">
    <div class="pal-in">
      <Icon name="code" />
      <input
        bind:this={inputEl}
        bind:value={q}
        onkeydown={onKey}
        placeholder={placeholder}
        aria-label="Search languages"
        autocomplete="off"
        spellcheck="false"
      />
      <span class="esc">esc</span>
    </div>
    <div class="pal-list" bind:this={listEl}>
      {#if !path}
        <div class="pal-empty">Open a file to change its language</div>
      {:else if flat.length === 0}
        <div class="pal-empty">{q.trim() ? 'No matching languages' : 'No languages loaded'}</div>
      {:else}
        {#if visibleActions.length}
          <div class="pal-g">This file</div>
          {#each visibleActions as r, i (r.id)}
            <div
              class="pal-row"
              class:sel={i === sel}
              role="button"
              tabindex="-1"
              onmousemove={(e) => onRowMouseMove(e, i)}
              onclick={() => run(i)}
              onkeydown={(e) => e.key === 'Enter' && run(i)}
            >
              <span class="pl">{r.label}</span>
              {#if r.psub}<span class="psub">{r.psub}</span>{/if}
              {#if r.current}<span class="pk">current</span>{/if}
            </div>
          {/each}
        {/if}
        {#if langRows.length}
          <div class="pal-g">{associate && assoc ? assoc.label : 'Languages'}</div>
          {#each langRows as r, i (`lang:${r.id}`)}
            {@const idx = visibleActions.length + i}
            <div
              class="pal-row"
              class:sel={idx === sel}
              role="button"
              tabindex="-1"
              onmousemove={(e) => onRowMouseMove(e, idx)}
              onclick={() => run(idx)}
              onkeydown={(e) => e.key === 'Enter' && run(idx)}
            >
              <span class="pl">{r.label}</span>
              {#if r.psub}<span class="psub">{r.psub}</span>{/if}
              {#if r.current}<span class="pk">current</span>{/if}
            </div>
          {/each}
        {/if}
      {/if}
    </div>
    <div class="pal-foot">
      <span><Kbd class="k" text="↑↓" />navigate</span>
      <span><Kbd class="k" text="↵" />{enterHint}</span>
      <span><Kbd class="k" text="esc" />close</span>
    </div>
  </div>
</div>
