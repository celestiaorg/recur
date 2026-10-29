<script lang="ts">
  import { onMount } from 'svelte'
  import { Dialogs } from '@wailsio/runtime'
  import { ArtifactsService, onArtifactCreated, onArtifactPresented, call, callOk } from '../lib/api'
  import { artifactIcon } from '../lib/artifacts'
  import { portalToBody } from '../lib/menu-space'
  import Icon from './Icon.svelte'

  /** Row both workspace artifacts and plan attachments can satisfy. */
  type ArtifactRow = {
    id: string
    title?: string
    kind: string
    path: string
  }

  let {
    workspaceId = '',
    onOpen,
    items,
    empty = 'No artifacts yet. Agents create them with create_artifact.',
    selectedId = '',
    onRenamed,
    onDeleted,
  }: {
    workspaceId?: string
    onOpen?: (artifact: ArtifactRow) => void
    /** When set, skip fetching and render this list (plan attachments). */
    items?: ArtifactRow[]
    empty?: string
    /** Artifact currently open in the editor (scratch://artifact-<id>.…). */
    selectedId?: string
    onRenamed?: (artifact: ArtifactRow) => void
    onDeleted?: (id: string) => void
  } = $props()

  let menu = $state<null | { x: number; y: number; row: ArtifactRow }>(null)
  let renamingId = $state('')
  let armedDeleteId = $state('')
  let deleteArmTimer: ReturnType<typeof setTimeout> | undefined

  let listed = $state<ArtifactRow[]>([])
  let err = $state('')
  let loading = $state(false)
  const rows = $derived(items ?? listed)

  async function refresh() {
    if (items !== undefined) return
    if (!workspaceId) {
      listed = []
      return
    }
    loading = true
    err = ''
    const list = await call(ArtifactsService.ListArtifacts(workspaceId), 'Could not list artifacts')
    listed = list ?? []
    loading = false
  }

  onMount(() => {
    if (items !== undefined) return
    // List refresh is driven by $effect(workspaceId); only wire bus here.
    const off1 = onArtifactCreated((ws) => { if (ws === workspaceId) void refresh() })
    // Observatory handles present_artifact (open pane + preview). Refresh the list here.
    const off2 = onArtifactPresented((ws) => {
      if (ws !== workspaceId) return
      void refresh()
    })
    return () => { off1(); off2() }
  })

  function openMenu(e: MouseEvent, row: ArtifactRow) {
    if (items !== undefined) return
    e.preventDefault()
    const pad = 8
    const x = Math.min(e.clientX, window.innerWidth - 220 - pad)
    const y = Math.min(e.clientY, window.innerHeight - 120 - pad)
    menu = { x: Math.max(pad, x), y: Math.max(pad, y), row }
  }

  $effect(() => {
    if (!menu) return
    const onDown = (ev: MouseEvent) => {
      if (!(ev.target as HTMLElement | null)?.closest?.('.obs-ctx')) menu = null
    }
    const onKey = (ev: KeyboardEvent) => {
      if (ev.key === 'Escape') menu = null
    }
    window.addEventListener('mousedown', onDown, true)
    window.addEventListener('keydown', onKey, true)
    return () => {
      window.removeEventListener('mousedown', onDown, true)
      window.removeEventListener('keydown', onKey, true)
    }
  })

  async function commitRename(row: ArtifactRow, title: string) {
    renamingId = ''
    title = title.trim()
    if (!title || title === (row.title || row.id)) return
    const next = await call(ArtifactsService.RenameArtifact(workspaceId, row.id, title), 'Could not rename artifact')
    if (!next) return
    onRenamed?.(next)
    await refresh()
  }

  async function deleteArtifact(row: ArtifactRow) {
    menu = null
    const name = row.title || row.id
    let choice = ''
    try {
      // window.confirm is a no-op in WKWebView.
      choice = await Dialogs.Question({
        Title: 'Delete',
        Message: `Delete “${name}”? This cannot be undone.`,
        Buttons: [
          { Label: 'Cancel', IsCancel: true },
          { Label: 'Delete', IsDefault: true },
        ],
      })
    } catch {
      return
    }
    if (choice !== 'Delete') return
    await removeArtifact(row)
  }

  function disarmDelete() {
    clearTimeout(deleteArmTimer)
    armedDeleteId = ''
  }

  function onDeleteClick(row: ArtifactRow) {
    if (armedDeleteId !== row.id) {
      armedDeleteId = row.id
      clearTimeout(deleteArmTimer)
      deleteArmTimer = setTimeout(disarmDelete, 3000)
      return
    }
    disarmDelete()
    void removeArtifact(row)
  }

  async function removeArtifact(row: ArtifactRow) {
    if (!(await callOk(ArtifactsService.DeleteArtifact(workspaceId, row.id), 'Could not delete artifact'))) return
    onDeleted?.(row.id)
    await refresh()
  }

  function reveal(row: ArtifactRow) {
    menu = null
    void callOk(ArtifactsService.RevealArtifact(workspaceId, row.id), 'Could not reveal artifact')
  }

  $effect(() => {
    void items
    if (items !== undefined) return
    void workspaceId
    void refresh()
  })
</script>

<div class="art-panel">
  {#if err}
    <div class="art-empty">{err}</div>
  {:else if loading && !rows.length}
    <div class="art-empty">Loading…</div>
  {:else if !rows.length}
    <div class="art-empty">{empty}</div>
  {:else}
    <ul class="art-list">
      {#each rows as a (a.id)}
        {@const delArmed = armedDeleteId === a.id}
        <li
          class="art-item"
          class:armed={delArmed}
          onpointerleave={() => { if (delArmed) disarmDelete() }}
        >
          {#if renamingId === a.id}
            <div class="art-row on">
              <Icon name={artifactIcon(a.kind)} size={14} />
              <!-- svelte-ignore a11y_autofocus -->
              <input
                class="art-rename"
                value={a.title || a.id}
                aria-label="Artifact title"
                autofocus
                onfocus={(e) => e.currentTarget.select()}
                onkeydown={(e) => {
                  if (e.key === 'Enter') void commitRename(a, e.currentTarget.value)
                  else if (e.key === 'Escape') renamingId = ''
                }}
                onblur={(e) => { if (renamingId === a.id) void commitRename(a, e.currentTarget.value) }}
              />
              <span class="art-kind">{a.kind}</span>
            </div>
          {:else}
          <button
            type="button"
            class="art-row"
            class:on={a.id === selectedId}
            aria-current={a.id === selectedId ? 'true' : undefined}
            onclick={() => onOpen?.(a)}
            oncontextmenu={(e) => openMenu(e, a)}
          >
            <Icon name={artifactIcon(a.kind)} size={14} />
            <span class="art-title">{a.title || a.id}</span>
            <span class="art-kind">{a.kind}</span>
          </button>
          {#if items === undefined}
            <button
              type="button"
              class="art-del"
              class:armed={delArmed}
              title={delArmed ? 'Confirm delete' : 'Delete'}
              aria-label={delArmed ? `Confirm delete ${a.title || a.id}` : `Delete ${a.title || a.id}`}
              onclick={() => onDeleteClick(a)}
            >
              <Icon name="trash" />
            </button>
          {/if}
          {/if}
        </li>
      {/each}
    </ul>
  {/if}
</div>

{#if menu}
  {@const row = menu.row}
  <div class="obs-ctx" style="left:{menu.x}px;top:{menu.y}px" role="menu" {@attach portalToBody}>
    <button type="button" role="menuitem" onclick={() => { renamingId = row.id; menu = null }}>
      <span class="obs-ctx-label">Rename…</span>
    </button>
    <button type="button" role="menuitem" onclick={() => reveal(row)}>
      <span class="obs-ctx-label">Reveal in Finder</span>
    </button>
    <div class="obs-ctx-sep"></div>
    <button type="button" role="menuitem" onclick={() => deleteArtifact(row)}>
      <span class="obs-ctx-label">Delete</span>
    </button>
  </div>
{/if}

<style>
  .art-panel { display: flex; flex-direction: column; flex: 1; height: 100%; min-height: 0; }
  .art-empty { padding: 16px 12px; font-size: 12px; opacity: .65; line-height: 1.4; }
  .art-list { list-style: none; margin: 0; padding: 6px; overflow: auto; }
  .art-row {
    width: 100%;
    display: grid;
    grid-template-columns: 14px 1fr auto;
    gap: 8px;
    align-items: center;
    text-align: left;
    border: none;
    background: transparent;
    color: inherit;
    padding: 8px;
    border-radius: 8px;
    cursor: pointer;
  }
  .art-row:hover,
  .art-row.on { background: var(--pane-2); }
  .art-row :global(svg) {
    width: 14px;
    height: 14px;
    min-width: 14px;
    color: var(--muted);
  }
  .art-title { font-size: 13px; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .art-item { position: relative; }
  .art-del {
    position: absolute;
    top: 50%;
    right: 6px;
    transform: translateY(-50%);
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 22px;
    height: 22px;
    padding: 0;
    border: none;
    border-radius: 4px;
    background: transparent;
    color: var(--faint);
    opacity: 0;
    pointer-events: none;
    cursor: pointer;
  }
  .art-item:hover .art-del,
  .art-item:focus-within .art-del,
  .art-item.armed .art-del {
    opacity: 1;
    pointer-events: auto;
  }
  .art-item:hover .art-kind,
  .art-item:focus-within .art-kind,
  .art-item.armed .art-kind { visibility: hidden; }
  .art-del:hover,
  .art-del:focus-visible { color: var(--coral); }
  .art-del.armed,
  .art-del.armed:hover {
    background: var(--coral);
    color: #0A0C10;
  }
  .art-del :global(svg) { width: 12px; height: 12px; }
  .art-rename {
    min-width: 0;
    font: inherit;
    font-size: 13px;
    color: inherit;
    background: var(--pane);
    border: 1px solid var(--line-bright);
    border-radius: 4px;
    padding: 1px 4px;
  }
  .art-kind { font-size: 11px; opacity: .55; text-transform: uppercase; }
</style>
