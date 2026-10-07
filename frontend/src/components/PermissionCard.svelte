<script lang="ts">
  // One live "may I run this tool?" card. Mounted either inline in an agent
  // chat transcript (compact) or as the floating corner panel (default).
  import { answerPermission } from '../lib/api'
  import { permissionToolLabel } from '../lib/permissionTools'
  import Icon from './Icon.svelte'
  import type { PermissionRequest } from '../../bindings/recur/internal/domain/models'

  let {
    request,
    /** Transcript variant — quieter, no jump link, sits in the chat flow. */
    compact = false,
    whereLabel = '',
    onOpenWhere,
    queued = 0,
  }: {
    request: PermissionRequest
    compact?: boolean
    whereLabel?: string
    onOpenWhere?: () => void
    queued?: number
  } = $props()

  let showDetail = $state(false)

  const toolLabel = $derived(permissionToolLabel(request.tool))

  // Engine summary leads with the friendly label; peel that off so the body
  // does not repeat the title ("List findings" / "List findings: open").
  const detailLine = $derived.by(() => {
    const s = (request.summary ?? '').trim()
    if (!s || s === toolLabel || s === request.tool) return ''
    if (request.tool && s.startsWith(request.tool)) {
      const rest = s.slice(request.tool.length).replace(/^:\s*/, '').trim()
      return rest || ''
    }
    if (s.startsWith(toolLabel + ': ')) return s.slice(toolLabel.length + 2).trim()
    if (s.startsWith(toolLabel + ' (')) return s.slice(toolLabel.length + 1).trim()
    return s
  })

  const argsJson = $derived.by(() => {
    if (!request?.input) return ''
    try {
      return JSON.stringify(JSON.parse(request.input), null, 2)
    } catch {
      return request.input
    }
  })

  function deny() {
    void answerPermission(request.id, false, { reason: 'denied by the human' })
  }
  function allowOnce() {
    void answerPermission(request.id, true)
  }
  function allowTurn() {
    void answerPermission(request.id, true, { remember: true })
  }
</script>

<div
  class="perm"
  class:inline={compact}
  class:float={!compact}
  role="group"
  aria-label="Tool permission needed"
>
  <div class="perm-head">
    <Icon name="triangle-alert" class="perm-ico" />
    <div class="perm-head-text">
      <span class="perm-kicker">Permission needed</span>
      {#if !compact && queued > 0}
        <span class="perm-queue-pill">+{queued} waiting</span>
      {/if}
    </div>
  </div>

  <div class="perm-main">
    <div class="perm-tool">{toolLabel}</div>
    {#if detailLine}
      <div class="perm-sum">{detailLine}</div>
    {/if}

    {#if whereLabel && onOpenWhere}
      <button type="button" class="perm-where" onclick={onOpenWhere} title="Open workspace">
        <Icon name="git-pull-request" />
        <span>{whereLabel}</span>
        <Icon name="chevron-right" class="perm-where-chev" />
      </button>
    {/if}
  </div>

  {#if argsJson}
    <div class="perm-args">
      <button
        type="button"
        class="perm-toggle"
        aria-expanded={showDetail}
        onclick={() => (showDetail = !showDetail)}
      >
        <Icon name="chevron-right" class="perm-toggle-chev{showDetail ? ' open' : ''}" />
        {showDetail ? 'Hide arguments' : 'Show arguments'}
      </button>
      {#if showDetail}
        <pre class="perm-detail">{argsJson}</pre>
      {/if}
    </div>
  {/if}

  <div class="perm-acts">
    <button type="button" class="perm-btn deny" onclick={deny}>Deny</button>
    <button type="button" class="perm-btn once" onclick={allowOnce}>Allow once</button>
    <button type="button" class="perm-btn go" onclick={allowTurn}>Allow for this turn</button>
  </div>
</div>
