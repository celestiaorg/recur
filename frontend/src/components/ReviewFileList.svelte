<script lang="ts">
  import { tick } from 'svelte'
  import { theme } from '../lib/stores'
  import { fileIconClass } from '../lib/file-icons'
  import { buildTreeFromPaths } from '../lib/changed-tree'
  import { scrollDeltaToReveal } from '../lib/workspace-nav'
  import { findingFlagForPath, severityRank, type FindingFlag } from '../lib/findings-ui'
  import { findingsForPath } from '../lib/finding-paths'
  import { commentCountForPath, commentsForPath } from '../lib/comment-thread'
  import type { FileDiff, Finding, TreeEntry } from '../../bindings/recur/internal/domain/models'
  import { fileDiffPathLabel, fileDiffTreeLabel, type FileListMode } from '../lib/review-diff'
  import Icon from './Icon.svelte'

  let {
    files,
    selectedPath = '',
    onSelect,
    mode = 'tree',
    findingsByPath = undefined as ReadonlyMap<string, Finding[]> | undefined,
    commentsByPath = undefined as ReadonlyMap<string, { line?: number }[]> | undefined,
    dirtyPaths = undefined as ReadonlySet<string> | undefined,
    reveal = 0,
  }: {
    files: FileDiff[]
    selectedPath?: string
    onSelect: (path: string, opts?: { highlight?: number }) => void
    mode?: FileListMode
    findingsByPath?: ReadonlyMap<string, Finding[]>
    commentsByPath?: ReadonlyMap<string, { line?: number }[]>
    dirtyPaths?: ReadonlySet<string>
    reveal?: number
  } = $props()

  const byPath = $derived(new Map(files.map((f) => [f.path, f])))
  const tree = $derived(buildTreeFromPaths(files.map((f) => f.path)))
  let collapsed = $state<Set<string>>(new Set())
  const rowEls = new Map<string, HTMLElement>()

  const findingFlags = $derived.by(() => {
    const m = new Map<string, FindingFlag>()
    if (!findingsByPath) return m
    for (const [p, fs] of findingsByPath) {
      if (!fs.length) continue
      let severity = 'info'
      for (const f of fs) {
        const sev = String(f.severity || 'info')
        if (severityRank(sev) > severityRank(severity)) severity = sev
      }
      m.set(p, { count: fs.length, severity })
    }
    return m
  })

  function flagFor(path: string, isDir: boolean): FindingFlag {
    if (!isDir) {
      const fs = findingsForPath(findingsByPath, path)
      if (fs.length) {
        let severity = 'info'
        for (const f of fs) {
          const sev = String(f.severity || 'info')
          if (severityRank(sev) > severityRank(severity)) severity = sev
        }
        return { count: fs.length, severity }
      }
    }
    return findingFlagForPath(path, isDir, findingFlags)
  }

  function notesFor(path: string, isDir: boolean): number {
    return commentCountForPath(commentsByPath, path, isDir)
  }

  function jumpToNotes(path: string, e: MouseEvent) {
    e.stopPropagation()
    const notes = commentsForPath(commentsByPath, path)
    onSelect(path, { highlight: notes[0]?.line ?? 0 })
  }

  function toggleDir(path: string) {
    const next = new Set(collapsed)
    if (next.has(path)) next.delete(path)
    else next.add(path)
    collapsed = next
  }

  function uncollapseAncestors(path: string) {
    const parts = path.split('/')
    const next = new Set(collapsed)
    let acc = ''
    let changed = false
    for (let i = 0; i < parts.length - 1; i++) {
      acc = acc ? `${acc}/${parts[i]}` : parts[i]
      if (next.has(acc)) {
        next.delete(acc)
        changed = true
      }
    }
    if (changed) collapsed = next
  }

  function registerRow(path: string) {
    return (el: HTMLElement) => {
      rowEls.set(path, el)
      return () => { rowEls.delete(path) }
    }
  }

  $effect(() => {
    void reveal
    const path = selectedPath
    if (!reveal || !path) return
    if (mode === 'tree') uncollapseAncestors(path)
    void tick().then(() => {
      const row = rowEls.get(path)
      if (!row) return
      const pane = row.closest('.fl-body') as HTMLElement | null
      if (!pane) {
        row.scrollIntoView({ block: 'nearest' })
        return
      }
      const view = pane.getBoundingClientRect()
      const rowRect = row.getBoundingClientRect()
      const rowH = parseFloat(getComputedStyle(pane).getPropertyValue('--tree-row-h')) || 24
      const ancestors = mode === 'tree' ? Math.max(0, path.split('/').filter(Boolean).length - 1) : 0
      const delta = scrollDeltaToReveal(rowRect.top, rowRect.bottom, view.top, view.bottom, ancestors * rowH, 0)
      if (delta === 0) return
      const max = Math.max(0, pane.scrollHeight - pane.clientHeight)
      pane.scrollTop = Math.min(max, Math.max(0, pane.scrollTop + delta))
    })
  })

  function stats(e: TreeEntry): { add: number; del: number } {
    if (!e.dir) {
      const f = byPath.get(e.path)
      return { add: f?.diff?.add ?? 0, del: f?.diff?.del ?? 0 }
    }
    let add = 0
    let del = 0
    for (const c of e.children ?? []) {
      const s = stats(c)
      add += s.add
      del += s.del
    }
    return { add, del }
  }

  function rowTitle(path: string, base: string): string {
    return dirtyPaths?.has(path) ? `${base} · has uncommitted edits` : base
  }
</script>

{#if mode === 'tree'}
  {#each tree as e (e.path)}
    {@render treeNode(e, 0)}
  {/each}
{:else}
  {#each files as f (f.path)}
    {@const st = f.status || ''}
    {@const flag = flagFor(f.path, false)}
    {@const notes = notesFor(f.path, false)}
    {@const dirty = !!dirtyPaths?.has(f.path)}
    <div
      class="fl-row"
      class:on={f.path === selectedPath}
      role="button"
      tabindex="0"
      title={rowTitle(f.path, fileDiffPathLabel(f))}
      {@attach registerRow(f.path)}
      onclick={() => onSelect(f.path)}
      onkeydown={(e) => e.key === 'Enter' && onSelect(f.path)}
    >
      <span class="fp">{fileDiffPathLabel(f)}</span>
      {#if notes}
        <span
          class="obs-comment-flag"
          title="{notes} unresolved comment{notes === 1 ? '' : 's'}"
          onclick={(e) => jumpToNotes(f.path, e)}
        >
          <Icon name="message-square" strokeWidth={2.2} />
        </span>
      {/if}
      {#if flag.count}
        <span class="obs-finding-flag {flag.severity}" title="{flag.count} open finding{flag.count === 1 ? '' : 's'}">
          <Icon name="hat-glasses" strokeWidth={2.2} />
        </span>
      {/if}
      <span class="fd"><span class="a">+{f.diff?.add ?? 0}</span><span class="r">−{f.diff?.del ?? 0}</span></span>
      <span class="fst {st}" class:wip={dirty}>{st ? st.toUpperCase() : '\u00a0'}</span>
    </div>
  {/each}
{/if}

{#snippet treeNode(e: TreeEntry, depth: number)}
  {@const open = e.dir && !collapsed.has(e.path)}
  {@const f = e.dir ? null : byPath.get(e.path)}
  {@const st = f?.status || ''}
  {@const s = stats(e)}
  {@const flag = flagFor(e.path, !!e.dir)}
  {@const notes = notesFor(e.path, !!e.dir)}
  {@const dirty = !e.dir && !!dirtyPaths?.has(e.path)}
  {#snippet treeRow()}
    <div
      class="fl-row fl-tree"
      class:dir={e.dir}
      class:open={open}
      class:on={!e.dir && e.path === selectedPath}
      role="button"
      tabindex="0"
      style="--d:{depth}"
      title={e.dir ? e.path : rowTitle(e.path, f ? fileDiffPathLabel(f) : e.path)}
      {@attach registerRow(e.path)}
      onclick={() => { if (e.dir) toggleDir(e.path); else onSelect(e.path) }}
      onkeydown={(ev) => {
        if (ev.key !== 'Enter') return
        if (e.dir) toggleDir(e.path)
        else onSelect(e.path)
      }}
    >
      <span class="fl-main">
        {#if e.dir}
          <Icon class="car" name="chevron-down" strokeWidth={2.4} />
        {:else}
          <span class="fico {fileIconClass(e.name, $theme)}" aria-hidden="true"></span>
        {/if}
        <span class="fp">{f ? fileDiffTreeLabel(f) : e.name}</span>
      </span>
      {#if notes}
        <span
          class="obs-comment-flag"
          class:dir={e.dir}
          title="{notes} unresolved comment{notes === 1 ? '' : 's'}"
          onclick={(ev) => {
            ev.stopPropagation()
            if (!e.dir) jumpToNotes(e.path, ev)
          }}
        >
          <Icon name="message-square" strokeWidth={2.2} />
        </span>
      {/if}
      {#if flag.count}
        <span
          class="obs-finding-flag {flag.severity}"
          class:dir={e.dir}
          title="{flag.count} open finding{flag.count === 1 ? '' : 's'}"
        >
          <Icon name="hat-glasses" strokeWidth={2.2} />
        </span>
      {/if}
      <span class="fd"><span class="a">+{s.add}</span><span class="r">−{s.del}</span></span>
      <span class="fst {st}" class:wip={dirty}>{st ? st.toUpperCase() : '\u00a0'}</span>
    </div>
  {/snippet}
  {#if e.dir}
    <div class="fl-branch" style="--d:{depth}">
      {@render treeRow()}
      {#if open}
        {#each e.children ?? [] as c (c.path)}
          {@render treeNode(c, depth + 1)}
        {/each}
      {/if}
    </div>
  {:else}
    {@render treeRow()}
  {/if}
{/snippet}

<style>
  .fl-tree.dir :global(.car) {
    width: 12px;
    height: 12px;
    flex: none;
    color: var(--faint);
    transform: rotate(-90deg);
  }
  .fl-tree.dir.open :global(.car) {
    transform: none;
  }
</style>
