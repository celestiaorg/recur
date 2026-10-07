<script lang="ts">
  import { filterSymbols } from '../lib/tm-symbols'
  import {
    searchFiles,
    subscribeFileIndexStatus,
    SEARCH_CANCELLED,
    type FileSearchHit,
  } from '../lib/file-index'
  import { setChatOccluder } from '../lib/chat-occlusion'
  import { createHoverGate, hoverGateAllows, resetHoverGate } from '../lib/hoverGate'
  import { untrack } from 'svelte'
  import Kbd from './Kbd.svelte'

  type Mode = 'file' | 'symbol'

  let {
    open = $bindable(false),
    mode = 'file' as Mode,
    projectId = '',
    workspaceId = '',
    symbols = [] as { name: string; kind: string; line: number }[],
    onPickFile,
    onPickSymbol,
  }: {
    open?: boolean
    mode?: Mode
    projectId?: string
    workspaceId?: string
    symbols?: { name: string; kind: string; line: number }[]
    onPickFile?: (path: string, line?: number) => void
    onPickSymbol?: (name: string, line: number) => void
  } = $props()

  let q = $state('')
  let sel = $state(0)
  let inputEl = $state<HTMLInputElement | null>(null)
  let listEl = $state<HTMLDivElement | null>(null)
  let fileHits = $state<FileSearchHit[]>([])
  let fileLoading = $state(false)
  let searchGen = 0
  let indexTick = $state(0)
  // Ignore hover selection until the pointer actually moves — opening under a
  // stationary cursor fires a synthetic mouseenter on whatever row is beneath it.
  let hoverGate = createHoverGate()

  // Full-screen Observatory overlay — selected-tab finishes stay unseen (D-attention).
  $effect(() => {
    setChatOccluder('quick-open', open)
    return () => setChatOccluder('quick-open', false)
  })

  $effect(() => {
    if (!workspaceId) return
    return subscribeFileIndexStatus(workspaceId, () => { indexTick++ })
  })

  const pathQuery = $derived.by(() => {
    const colon = q.lastIndexOf(':')
    if (colon > 0 && /^\d+$/.test(q.slice(colon + 1))) {
      return q.slice(0, colon)
    }
    return q
  })

  // Do not read fileHits here — assigning a fresh hits array would re-trigger
  // this effect in a SearchFiles busy-loop while Quick Open is open.
  $effect(() => {
    void indexTick
    if (!open || mode !== 'file' || !projectId || !workspaceId) {
      if (mode !== 'file') fileHits = []
      return
    }
    const query = pathQuery
    const my = ++searchGen
    fileLoading = untrack(() => fileHits.length) === 0
    void searchFiles(projectId, workspaceId, query, 50, true).then((res) => {
      if (res === SEARCH_CANCELLED || my !== searchGen) return
      fileHits = res.hits.filter((h) => !h.dir)
      fileLoading = res.building && res.hits.length === 0
    })
  })

  const hits = $derived.by((): FileSearchHit[] | ReturnType<typeof filterSymbols> => {
    if (mode === 'symbol') {
      return filterSymbols(symbols, q, 40)
    }
    return fileHits
  })

  $effect(() => {
    if (open) {
      q = ''
      sel = 0
      setTimeout(() => inputEl?.focus(), 0)
    }
  })
  $effect(() => {
    if (sel >= hits.length) sel = Math.max(0, hits.length - 1)
  })

  // Re-arm whenever the palette opens/closes or its rows change so a stationary
  // cursor never steals the keyboard default (index 0).
  $effect(() => {
    void open
    void hits
    resetHoverGate(hoverGate)
  })

  $effect(() => {
    void sel
    void hits
    queueMicrotask(() => {
      listEl?.querySelector<HTMLElement>('.qo-row.on')?.scrollIntoView({ block: 'nearest' })
    })
  })

  function onRowMouseMove(e: MouseEvent, i: number) {
    if (hoverGateAllows(hoverGate, e.clientX, e.clientY)) sel = i
  }

  function onRowMouseEnter(i: number) {
    if (hoverGate.armed) sel = i
  }

  function parseLine(): number | undefined {
    const colon = q.lastIndexOf(':')
    if (colon > 0 && /^\d+$/.test(q.slice(colon + 1))) {
      return Number(q.slice(colon + 1))
    }
    return undefined
  }

  function run(i: number) {
    const h = hits[i]
    if (!h) return
    open = false
    if (mode === 'symbol') {
      const s = h.item as { name: string; kind: string; line: number }
      onPickSymbol?.(s.name, s.line)
    } else {
      onPickFile?.(h.item as string, parseLine())
    }
  }

  function onKey(e: KeyboardEvent) {
    if (e.metaKey || e.ctrlKey || e.altKey) return
    if (e.key === 'ArrowDown') {
      e.preventDefault()
      // Disarm hover so scroll-under-pointer synthetic mouse events cannot
      // snap sel back to the row under a stationary cursor.
      resetHoverGate(hoverGate)
      sel = Math.min(sel + 1, Math.max(0, hits.length - 1))
    } else if (e.key === 'ArrowUp') {
      e.preventDefault()
      resetHoverGate(hoverGate)
      sel = Math.max(sel - 1, 0)
    } else if (e.key === 'Enter') {
      e.preventDefault()
      run(sel)
    } else if (e.key === 'Escape') {
      e.preventDefault()
      open = false
    }
  }
</script>

{#if open}
  <div class="qo-back" role="presentation" onclick={(e) => { if (e.target === e.currentTarget) open = false }}>
    <div class="qo" role="dialog" tabindex="-1" aria-label={mode === 'symbol' ? 'Go to symbol' : 'Quick open'}>
      <div class="qo-in">
        <input
          bind:this={inputEl}
          bind:value={q}
          onkeydown={onKey}
          placeholder={mode === 'symbol' ? 'Go to symbol in file…' : 'Go to file…'}
          spellcheck="false"
          autocomplete="off"
        />
        <Kbd class="qo-esc" text="Esc" />
      </div>
      <div class="qo-list" role="listbox" bind:this={listEl}>
        {#if mode === 'file' && fileLoading && !hits.length}
          <div class="qo-empty">Loading files…</div>
        {:else}
          {#each hits as h, i (h.label + i)}
            <button
              type="button"
              class="qo-row"
              class:on={i === sel}
              role="option"
              aria-selected={i === sel}
              onmousemove={(e) => onRowMouseMove(e, i)}
              onmouseenter={() => onRowMouseEnter(i)}
              onclick={() => run(i)}
            >
              {#if mode === 'symbol'}
                {@const s = h.item as { name: string; kind: string; line: number }}
                <span class="k">{s.kind}</span>
                <span class="n">{s.name}</span>
                <span class="ln">{s.line}</span>
              {:else}
                <span class="n">{h.item as string}</span>
              {/if}
            </button>
          {:else}
            <div class="qo-empty">{q.trim() ? 'No matches' : (mode === 'symbol' ? 'No symbols in this file' : 'Type to filter files')}</div>
          {/each}
        {/if}
      </div>
    </div>
  </div>
{/if}

<style>
  .qo-back {
    position: fixed;
    inset: 0;
    z-index: 80;
    background: color-mix(in srgb, var(--bg) 55%, transparent);
    display: grid;
    place-items: start center;
    padding-top: 12vh;
  }
  .qo {
    width: min(560px, 92vw);
    background: var(--pane);
    border: 1px solid var(--line-bright);
    border-radius: 10px;
    box-shadow: 0 18px 48px rgba(0, 0, 0, 0.35);
    overflow: hidden;
  }
  .qo-in {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 10px 12px;
    border-bottom: 1px solid var(--line);
  }
  .qo-in input {
    flex: 1;
    background: transparent;
    border: none;
    color: var(--text);
    font-family: var(--mono);
    font-size: 13px;
    outline: none;
  }
  .qo-list {
    max-height: 360px;
    overflow: auto;
    padding: 4px;
  }
  .qo-row {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    text-align: left;
    padding: 7px 10px;
    border: none;
    border-radius: 6px;
    background: transparent;
    color: var(--text);
    font-family: var(--mono);
    font-size: 12px;
    cursor: pointer;
  }
  .qo-row.on { background: var(--amber-d); }
  .qo-row .k { color: var(--faint); font-size: 10px; width: 52px; flex: none; text-transform: uppercase; }
  .qo-row .n { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .qo-row .ln { color: var(--faint); font-size: 11px; }
  .qo-empty { padding: 16px; color: var(--faint); font-size: 12px; text-align: center; }
</style>
