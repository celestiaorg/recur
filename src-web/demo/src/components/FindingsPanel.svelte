<script lang="ts">
  // Reviewer findings: master–detail list + inspector with Ignore / Mark
  // invalid / Reopen. Engine-owned "resolved" is read-only for status.
  import type { Finding } from '../lib/types'
  import MarkdownHtml from './MarkdownHtml.svelte'
  import Icon from './Icon.svelte'
  import { findingBodyNeeded, findingSevIcon, findingTitle } from '../lib/findings-ui'
  import { formatRelativeAge } from '../lib/format'

  export type FindingsStatusFilter = 'open' | 'ignored' | 'invalid' | 'resolved' | 'all'

  let {
    findings = [],
    filter = 'open',
    onOpen,
    onSetStatus,
  }: {
    findings?: Finding[]
    filter?: FindingsStatusFilter
    onOpen: (f: Finding) => void
    onSetStatus: (f: Finding, status: string) => void
  } = $props()

  let selectedId = $state('')

  const visible = $derived(
    filter === 'all'
      ? findings
      : findings.filter((f) => (f.status || 'open') === filter),
  )
  const selected = $derived(visible.find((f) => f.id === selectedId) ?? visible[0] ?? null)

  function select(f: Finding) {
    selectedId = f.id
    if (f.file) onOpen(f)
  }

  // Compact age for list + status attribution; full stamp lives in the title tooltip.
  function statusWhen(iso: string | undefined): { rel: string; full: string } | null {
    if (!iso) return null
    return formatRelativeAge(Date.parse(iso))
  }

  function statusByLine(status: string, updatedBy: string | undefined): string {
    const by =
      updatedBy === 'coder' ? ' by coder'
      : updatedBy === 'reviewer' ? ' by reviewer'
      : updatedBy === 'engine' ? ' by the loop'
      : updatedBy === 'human' ? ' by you'
      : ''
    switch (status) {
      case 'resolved': return `Resolved${by}`
      case 'invalid': return `Marked invalid${by}`
      case 'ignored': return `Ignored${by}`
      default: return ''
    }
  }

  function locLabel(f: Finding): string {
    const name = (f.file || '').split('/').pop() ?? ''
    const loc = !f.line || f.line <= 0 ? '' : f.endLine && f.endLine > f.line ? `:${f.line}–${f.endLine}` : `:${f.line}`
    return name ? `${name}${loc}` : loc.replace(/^:/, '')
  }

  /** Show status in the list only when the filter doesn't already imply it. */
  function showListStatus(status: string): boolean {
    return filter === 'all' || status !== filter
  }
</script>

<div class="findings-panel">
  {#if !findings.length}
    <div class="obs-empty">No findings yet: run /review.</div>
  {:else if !visible.length}
    <div class="obs-empty">No {filter} findings.</div>
  {:else}
    <div class="findings-split" style="--findings-split-pct:60%">
      <div class="findings-list" role="list" aria-label="Findings">
        {#each visible as f (f.id)}
          {@const status = f.status || 'open'}
          {@const sel = f.id === selected?.id}
          {@const when = statusWhen(f.updatedAt || f.createdAt)}
          <div class="finding-row-wrap" role="listitem" data-finding-id={f.id}>
            <button
              type="button"
              class="finding-row-md"
              class:sel
              aria-current={sel ? 'true' : undefined}
              onclick={() => select(f)}
            >
              <span class="finding-sev {f.severity}" aria-hidden="true">
                <Icon name={findingSevIcon(f.severity)} />
              </span>
              <span class="finding-row-main">
                <span class="finding-title">{findingTitle(f.message)}</span>
                <span class="finding-sub">
                  <span class="finding-loc" title={f.file}>{locLabel(f)}</span>
                  {#if showListStatus(status)}
                    <span class="finding-status {status}">{status}</span>
                  {/if}
                </span>
              </span>
              <span class="finding-row-trail">
                {#if when}
                  <span class="finding-ago" title={when.full}>{when.rel}</span>
                {/if}
              </span>
            </button>
          </div>
        {/each}
      </div>

      <div class="hsplit findings-hsplit" aria-hidden="true"></div>

      <div class="finding-inspector" data-finding-detail>
        {#if selected}
          {@const f = selected}
          {@const status = f.status || 'open'}
          <div class="finding-inspector-head">
            <div class="finding-inspector-meta">
              <span class="tc-sev {f.severity}">{f.severity}</span>
              <h3 class="finding-inspector-title" title={findingTitle(f.message)}>{findingTitle(f.message)}</h3>
              {#if showListStatus(status) || status !== 'open'}
                <span class="finding-status {status}">{status}</span>
              {/if}
            </div>
            <div class="finding-actions">
              {#if status === 'open'}
                <button type="button" class="finding-act-btn" onclick={() => onSetStatus(f, 'ignored')}>
                  <Icon name="eye-off" />
                  Ignore
                </button>
                <button type="button" class="finding-act-btn danger" onclick={() => onSetStatus(f, 'invalid')}>
                  <Icon name="bug-off" />
                  Mark invalid
                </button>
              {:else if status === 'ignored' || status === 'invalid'}
                <button type="button" class="finding-act-btn" onclick={() => onSetStatus(f, 'open')}>
                  <Icon name="undo" />
                  Reopen
                </button>
              {/if}
            </div>
          </div>
          <div class="finding-inspector-scroll">
            <button
              type="button"
              class="finding-inspector-loc"
              title={f.file || undefined}
              disabled={!f.file}
              onclick={() => f.file && onOpen(f)}
            >{locLabel(f)}</button>
            {#if findingBodyNeeded(f.message || '')}
              <MarkdownHtml text={f.message} class="chat-canvas finding-body" />
            {/if}
            {#if f.statusReason}
              <p class="finding-reason"><span class="finding-reason-label">Reason</span> {f.statusReason}</p>
            {/if}
            {#if status !== 'open'}
              {@const when = statusWhen(f.updatedAt)}
              {@const by = statusByLine(status, f.updatedBy)}
              {#if by}
                <p class="finding-note" title={when?.full ?? undefined}>
                  {by}{#if when}<span class="finding-when"> · {when.rel}</span>{/if}
                </p>
              {/if}
            {/if}
          </div>
        {/if}
      </div>
    </div>
  {/if}
</div>
