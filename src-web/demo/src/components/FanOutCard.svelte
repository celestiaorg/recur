<script lang="ts">
  // One wave of the board: the children started together, with live state
  // read from the workspace store. A row opens the child workspace.
  import { workspaces, workspacesRunning, openWorkspace } from '../lib/stores'
  import { workspaceIsRunning, chatModeMeta } from '../lib/modes'
  import Icon from './Icon.svelte'

  type FanOutChild = { id: string; title: string; steps: string[]; mode?: string; state?: string; note?: string }
  type FanOutWave = { wave: number; title?: string; after?: string[]; children: FanOutChild[]; done?: boolean }

  let { data }: { data: string } = $props()

  const wave = $derived.by((): FanOutWave | null => {
    try {
      const w = JSON.parse(data) as FanOutWave
      return w && Array.isArray(w.children) ? w : null
    } catch {
      return null
    }
  })

  type RowState = { key: 'running' | 'reviewing' | 'needs' | 'merged' | 'failed' | 'conflict' | 'queued' | 'done'; label: string; live: boolean }

  function rowState(c: FanOutChild): RowState {
    const ws = $workspaces.find((w) => w.id === c.id)
    const final = (c.state ?? '').toLowerCase()
    if (final.startsWith('conflict')) return { key: 'conflict', label: c.state!, live: false }
    if (final.startsWith('failed') || final.startsWith('stopped')) return { key: 'failed', label: c.state!, live: false }
    if (final === 'closed') return { key: 'done', label: 'closed', live: false }
    if (final === 'merged' || (ws && (ws.state === 'merged' || ws.archived))) return { key: 'merged', label: 'merged', live: false }
    if (!ws) return { key: 'queued', label: c.state || 'starting', live: false }
    const running = workspaceIsRunning(ws, $workspacesRunning)
    // A chat child's row stays 'paused' mid-turn and its phase is the
    // creation placeholder ("editing"), so the live agent set decides
    // "running" and only loop children (step chips) show a phase.
    if (running && ws.state !== 'escalated') {
      const phase = c.steps.length && ws.phase && ws.phase !== 'idle' && ws.phase !== 'starting' ? ws.phase : 'working'
      return { key: 'running', label: phase, live: true }
    }
    if (!c.steps.length && !['failed', 'stopped', 'escalated'].includes(ws.state)) {
      return { key: 'done', label: c.state === 'settled' || ws.phase === 'idle' ? 'finished' : 'idle', live: false }
    }
    switch (ws.state) {
      case 'escalated':
        return { key: 'needs', label: 'needs you', live: false }
      case 'reviewing':
        return { key: 'reviewing', label: 'reviewing', live: running }
      case 'coding':
      case 'planning':
        return { key: 'running', label: ws.phase || ws.statusLabel || 'working', live: running }
      case 'queued':
        return { key: 'queued', label: 'queued', live: false }
      case 'failed':
      case 'stopped':
        return { key: 'failed', label: ws.state, live: false }
      case 'paused':
        return { key: 'done', label: ws.phase === 'artifact' || ws.phase === 'idle' ? 'finished' : 'paused', live: false }
      default:
        return { key: 'running', label: ws.statusLabel || ws.state, live: running }
    }
  }

  const summary = $derived.by(() => {
    if (!wave) return ''
    const n = wave.children.length
    const merged = wave.children.filter((c) => rowState(c).key === 'merged').length
    if (merged === n) return `${n} merged`
    return `${merged} of ${n} merged`
  })
</script>

{#if wave}
  <div class="fanout-card">
    <div class="fanout-head">
      <span class="ico" aria-hidden="true"><Icon name="git-fork" /></span>
      <span class="fanout-title">{wave.title ?? `Wave ${wave.wave}`}</span>
      {#if wave.after?.length}<span class="fanout-after" title="Started when these steps landed">after {wave.after.join(', ')}</span>{/if}
      <span>{wave.children.length} child workspace{wave.children.length === 1 ? '' : 's'}</span>
      <span class="fanout-sum">{summary}</span>
    </div>
    {#each wave.children as c (c.id)}
      {@const st = rowState(c)}
      <button
        type="button"
        class="fanout-row clickable"
        data-child={c.id}
        title={`Open ${c.id}`}
        onclick={() => openWorkspace(c.id)}
      >
        <span class="fanout-steps">
          <span class="fanout-chips">{#if c.mode}{@const mm = chatModeMeta(c.mode)}<span class="tstep" style="color:{mm.tint};background:color-mix(in srgb, {mm.tint} 14%, transparent)">{mm.label}</span>{/if}{#each c.steps as s (s)}<span class="tstep">{s}</span>{/each}</span>
        </span>
        <span class="fanout-name">
          {c.title}
          {#if !c.steps.length && c.note}<span class="fanout-note">{c.note}</span>{/if}
        </span>
        <span class="fanout-state {st.key}">
          {#if st.live}
            <span class="sd" aria-label="running"></span>
          {:else if st.key === 'merged'}
            <Icon name="git-merge" />
          {:else if st.key === 'needs'}
            <Icon name="circle-alert" />
          {:else if st.key === 'failed' || st.key === 'conflict'}
            <Icon name="octagon-x" />
          {/if}
          {st.label}
        </span>
      </button>
    {/each}
  </div>
{/if}
