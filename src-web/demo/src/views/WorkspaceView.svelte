<script lang="ts">
  // One workspace: the code Observatory with the agent chat docked on the left.
  import { workspaces, settings, pendingChatFocus } from '../lib/stores'
  import { call, service } from '../lib/api'
  import { mapChatItems } from '../lib/agentchat'
  import { chatModeOptions, chatModeMeta, normalizeChatMode, wsRunActive, type ChatMode } from '../lib/modes'
  import { loadModelCatalog, peekModelCatalog, normalizeFlags, flagsForModel, type CatalogModel, type FlagMap } from '../lib/models'
  import { LOOPS, POLICIES, workspaceLoopId, workspacePolicyId } from '../lib/loops'
  import { isPairChatCode, isLoopCockpitTab, mergeLoopCockpit, type FeedSess } from '../lib/loop-cockpit'
  import { simplifyEnabled } from '../lib/simplify'
  import Select from '../components/Select.svelte'
  import LoopPolicyPickers from '../components/LoopPolicyPickers.svelte'
  import Observatory from '../components/Observatory.svelte'
  import AgentChat, { type AgentMsg } from '../components/AgentChat.svelte'
  import SessionCard from '../components/SessionCard.svelte'
  import ModelPickers from '../components/ModelPickers.svelte'
  import TabStrip from '../components/TabStrip.svelte'
  import TabChip from '../components/TabChip.svelte'
  import Icon from '../components/Icon.svelte'
  import type { WorkspaceChatTab, FeedItem, ChatItem } from '../lib/types'

  type SessionView = { id: string; code: string; ownerCode?: string; name: string; kind: string; createdAt: number; items: ChatItem[] }

  let { workspaceId }: { workspaceId: string } = $props()

  const ws = $derived($workspaces.find((w) => w.id === workspaceId))
  const isMain = $derived(!!ws?.isMain)
  const isPRHost = $derived(!!(ws?.hostPrId ?? '').trim())

  let tabs = $state<WorkspaceChatTab[]>([])
  let activeCode = $state('')
  let observatory = $state<Observatory | undefined>()
  let catalogModels = $state<CatalogModel[]>(peekModelCatalog())

  type Pick = { mode: ChatMode; model: string; flags: FlagMap; simplify: boolean }
  let picks = $state<Record<string, Pick>>({})
  let chats = $state<Record<string, AgentMsg[]>>({})
  let feeds = $state<Record<string, { sessions: FeedSess[]; facts: FeedItem[] }>>({})

  $effect(() => {
    void loadModelCatalog().then((m) => (catalogModels = m)).catch(() => {})
    void call(service.ListWorkspaceChats(workspaceId), 'Could not load chats').then((list) => {
      tabs = list?.length ? list : [{ code: 'PA', title: 'New chat' } as WorkspaceChatTab]
      if (!activeCode) activeCode = tabs[0].code
    })
  })

  $effect(() => {
    const pending = $pendingChatFocus
    if (pending?.workspaceId !== workspaceId) return
    pendingChatFocus.set(null)
    activeCode = pending.code
  })

  $effect(() => {
    const code = activeCode
    if (!code || code in chats) return
    void service.WorkspaceChatItems(workspaceId, code).then((items) => {
      chats = { ...chats, [code]: mapChatItems(items) }
    })
    void service.WorkspaceChatSelection(workspaceId, code).then((sel) => {
      picks = {
        ...picks,
        [code]: {
          mode: normalizeChatMode(sel?.mode || tabs.find((t) => t.code === code)?.mode || (code === 'MA' ? 'loop' : '')),
          model: sel?.model || '',
          flags: sel?.flags,
          simplify: sel?.simplify ?? simplifyEnabled($settings),
        },
      }
    })
    // Loop cockpit rows: specialist sessions and engine facts owned by this tab.
    const owns = (owner?: string) => (owner ? owner === code : code === 'PA' || code === 'MA')
    void Promise.all([
      service.WorkspaceSessions(workspaceId, code, '').catch(() => null),
      service.WorkspaceFacts(workspaceId).catch(() => null),
    ]).then(([sv, fx]: [SessionView[] | null, FeedItem[] | null]) => {
      feeds = {
        ...feeds,
        [code]: {
          sessions: (sv ?? [])
            .filter((s) => !isPairChatCode(s.code) && owns(s.ownerCode))
            .map((s) => ({ id: s.id, code: s.code, name: s.name, kind: s.kind, ts: s.createdAt, messages: mapChatItems(s.items) })),
          facts: (fx ?? []).filter((f) => owns(f.chatCode)),
        },
      }
    })
  })

  const pick = $derived(picks[activeCode])
  const chatMode = $derived(pick?.mode ?? normalizeChatMode(activeCode === 'MA' ? 'loop' : ''))
  const showLoopPickers = $derived(!!ws && !isMain && (chatMode === 'loop' || activeCode === 'MA'))
  const activeTab = $derived(tabs.find((t) => t.code === activeCode))
  const showLoopCockpit = $derived(isLoopCockpitTab(activeCode, !!activeTab?.loopStarted))
  const ma = $derived(chats[activeCode] ?? [])
  const feed = $derived(feeds[activeCode] ?? { sessions: [], facts: [] })
  const fanouts = $derived(feed.facts.filter((f) => f.card === 'fanout'))
  const chat = $derived(
    showLoopCockpit
      ? mergeLoopCockpit(ma, feed.sessions, feed.facts, new Set(), ws?.goalDisplay || ws?.goal || '')
      : fanouts.length ? mergeLoopCockpit(ma, [], fanouts, new Set()) : ma,
  )
  const waitingSubagents = $derived.by(() => {
    const n = $workspaces.filter((w) =>
      w.parentId === workspaceId && !w.archived && (w.fanOut?.chat || 'PA') === activeCode &&
      (!w.fanOut?.outcome || w.fanOut.outcome === 'report-pending'),
    ).length
    return n ? `Waiting for ${n} subagent${n === 1 ? '' : 's'}…` : ''
  })

  function setPick(next: Partial<Pick>) {
    if (pick) picks = { ...picks, [activeCode]: { ...pick, ...next } }
  }

  function send(text: string) {
    chats = { ...chats, [activeCode]: [...ma, { me: true, body: text, optimistic: true, ts: Date.now() }] }
  }

  async function showTurnDiff(messageId: string) {
    const turn = await call(service.WorkspaceChatTurnDiff(workspaceId, activeCode, messageId), 'Could not load this turn\'s changes')
    if (turn?.files?.length) observatory?.openTurnReview(turn)
  }
</script>

<div class="wsv">
  {#if ws}
    <div class="wsv-obs">
      <Observatory bind:this={observatory} projectId={ws.projectId} workspaceId={ws.id} loopScoped leftTitle="Agent">
        {#snippet leftHead(lit: boolean)}
          <TabStrip class="wsv-chat-tabs" label="Agent chats" activeKey={activeCode}>
            {#each tabs as tab (tab.code)}
              {@const meta = chatModeMeta(tab.code === 'MA' ? 'loop' : (picks[tab.code]?.mode ?? tab.mode))}
              <TabChip
                class="wsv-chat-tab"
                tabKey={tab.code}
                label={tab.title || 'New chat'}
                title={`${tab.title || 'New chat'}: ${meta.label} mode`}
                icon={meta.icon}
                iconTint={meta.tint}
                active={activeCode === tab.code}
                {lit}
                unseen={!!tab.unseen && activeCode !== tab.code}
                labelKey={`${workspaceId}:${tab.code}`}
                onclick={() => (activeCode = tab.code)}
              />
            {/each}
          </TabStrip>
        {/snippet}
        {#snippet leftPanel()}
          <AgentChat
            messages={chat}
            statusText={waitingSubagents}
            onSend={send}
            canStop={showLoopCockpit && wsRunActive(ws)}
            compact
            resetKey={`${workspaceId}:${activeCode}`}
            emptyText={activeCode === 'MA'
              ? 'Ask where this loop stands, or steer it — the Maestro can read the worktree, edit the plan, re-run review, and dispatch specialists; rails hold at execution.'
              : 'Ask the agent, or tell it what to change — the mode selector below decides whether it may edit.'}
            placeholder={activeCode === 'MA'
              ? 'Ask where things stand, or steer the loop… (@ files, / skills)'
              : 'Message the agent… (@ files, / skills, ⏎ to send)'}
            onTurnDiff={showTurnDiff}
          >
            {#snippet sessionCard(log)}
              <SessionCard code={log.code} name={log.name} messages={log.messages} live={log.live} />
            {/snippet}
            {#snippet toolbar()}
              {#if pick && catalogModels.length}
                <Select
                  ghost up
                  class="wsv-mode"
                  label="Chat mode"
                  options={activeCode === 'MA' ? chatModeOptions() : chatModeOptions(isMain || isPRHost, isPRHost)}
                  value={chatMode}
                  onchange={(v) => setPick({ mode: normalizeChatMode(v) })}
                />
                {#if showLoopPickers}
                  <LoopPolicyPickers
                    loopId={activeTab?.loopId?.trim() || workspaceLoopId(ws, $settings)}
                    policyId={activeTab?.policyId?.trim() || workspacePolicyId(ws, $settings)}
                    loops={LOOPS}
                    policies={POLICIES}
                    pinned={!!activeTab?.loopStarted}
                  />
                {:else}
                  <ModelPickers
                    ghost up
                    models={catalogModels}
                    model={pick.model}
                    flags={normalizeFlags(flagsForModel(catalogModels, pick.model), pick.flags)}
                    onModelChange={(model, flags) => setPick({ model, flags })}
                    onFlagsChange={(flags) => setPick({ flags })}
                  />
                {/if}
              {:else}
                <div class="model-pickers model-pickers-pending" aria-busy="true" aria-label="Loading models">
                  <span class="mpp-skel"></span>
                  <span class="mpp-skel mpp-skel-sm"></span>
                </div>
              {/if}
              <button
                type="button"
                class="wsv-simplify"
                class:on={pick?.simplify}
                aria-pressed={!!pick?.simplify}
                aria-label="Simplify mode"
                title={pick?.simplify ? 'Simplify on' : 'Simplify off'}
                disabled={!pick}
                onclick={() => setPick({ simplify: !pick?.simplify })}
              >
                <Icon name="feather" />
                <span class="wsv-simplify-label">Simplify</span>
              </button>
            {/snippet}
          </AgentChat>
        {/snippet}
      </Observatory>
    </div>
  {/if}
</div>

<style>
  .wsv { display: flex; flex-direction: column; height: 100%; min-height: 0; }
  .wsv-obs { flex: 1; min-height: 0; display: flex; overflow: hidden; }
  /* Reserve dock space while the catalogue loads — avoids a raw-ref flash. */
  .model-pickers-pending {
    display: inline-flex; align-items: center; gap: 6px;
    height: 24px; padding: 0 6px; min-width: 118px;
  }
  .mpp-skel {
    display: block; height: 10px; width: 72px; border-radius: 4px;
    background: var(--pane-2);
  }
  .mpp-skel-sm { width: 44px; opacity: .75; }
</style>
