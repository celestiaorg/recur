<script lang="ts">
  import { onDestroy } from 'svelte'
  import { BriefingService, appChatAgentId, onSessionDelta, onSessionStep, onSessionThinking, onSessionToolResult, onSessionToolInput, onSessionTurnEnd, onSessionNotice, onSessionFallback, onSessionCompact, type WorkspaceStep } from '../lib/api'
  import { mapChatItems, mergePersistedWithLive, appendStream as appendStreamRow, applyStream, addTool as addToolRow, attachToolResult, patchToolInput, endTurn as endTurnRow, applyCompact as applyCompactRow, applyNotice as applyNoticeRow, applyFallback as applyFallbackRow, dropLiveAfterFooter, noteThinkingTokens } from '../lib/agentchat'
  import { contextUsageFromChatItems, contextUsageFromTurn, type ContextUsage } from '../lib/contextUsage'
  import { loadModelCatalog, peekModelCatalog, normalizeFlags, flagsForModel, type CatalogModel } from '../lib/models'
  import { resolvedDefaultForMode, chatModeMeta } from '../lib/modes'
  import { settings, setInboxChatOpen, agentsRunning } from '../lib/stores'
  import type { WorkspaceChatTab } from '../../bindings/recur/internal/domain/models'
  import AgentChat, { type AgentMsg } from './AgentChat.svelte'
  import ModelPickers from './ModelPickers.svelte'
  import TabStrip from './TabStrip.svelte'
  import TabChip from './TabChip.svelte'
  import Icon from './Icon.svelte'

  const APP = 'APP'
  const ACTIVE_KEY = 'recur.askActiveTab'
  const ASK_CHIPS = ['What needs me?', 'Where did I leave off?', 'Which workspace should I open next?']

  let { visible = true }: { visible?: boolean } = $props()

  let tabs = $state<WorkspaceChatTab[]>([{ code: APP, title: 'Ask Recur' } as WorkspaceChatTab])
  let active = $state((() => {
    try { return localStorage.getItem(ACTIVE_KEY) || APP } catch { return APP }
  })())
  let messages = $state<AgentMsg[]>([])
  let input = $state('')
  let sendingBy = $state<Record<string, boolean>>({})
  let turnStartedAt = $state(0)
  let turnGotText = $state(false)
  let streamSeq = $state<Record<string, number>>({})
  let contextUsage = $state<ContextUsage | null>(null)
  let catalogModels = $state<CatalogModel[]>(peekModelCatalog())
  let model = $state('')
  let flags = $state<Record<string, string>>({})

  function tabBusy(code: string): boolean {
    return !!sendingBy[code] || $agentsRunning.has(appChatAgentId(code))
  }
  const sending = $derived(tabBusy(active))
  const askMeta = chatModeMeta('ask')

  // Amber tab underline matches Observatory / PR: light only while this dock
  // has focus. Selected-but-unlit still keeps the thick --line rule.
  let rootEl = $state<HTMLElement | undefined>(undefined)
  let focused = $state(false)
  const panelLit = $derived(visible && focused)

  function onPanelFocusIn() {
    focused = true
  }

  function onPanelFocusOut(e: FocusEvent) {
    const next = e.relatedTarget
    if (next instanceof Node && rootEl?.contains(next)) return
    focused = false
  }

  // Transcript / empty chrome aren't focusable, so a click there never fires
  // focusin. Park on the panel (same as Observatory's left dock) so the tab
  // underline lights without stealing into the composer.
  function onPanelMouseDown(e: MouseEvent) {
    if (e.button !== 0) return
    const t = e.target
    if (!(t instanceof Element)) return
    if (
      t.closest(
        'textarea, input, select, button, a, [role="button"], [role="tab"], [contenteditable="true"]',
      )
    ) {
      return
    }
    rootEl?.focus({ preventScroll: true })
  }

  $effect(() => {
    try { localStorage.setItem(ACTIVE_KEY, active) } catch { /* ignore */ }
  })

  function isActive(id: string, code: string) {
    return id === '' && code === active
  }

  function refreshTabs() {
    void BriefingService.ListAppChats().then((list) => {
      if (!list?.length) return
      tabs = list
      if (!list.some((t) => t.code === active)) active = APP
    }).catch(() => {})
  }

  function liveTurnClosed(): boolean {
    return dropLiveAfterFooter(messages, turnStartedAt, sending)
  }

  function applyLiveStream(kind: 'text' | 'thinking', text: string, opts: { seq?: number; replace?: boolean; done?: boolean; parentId?: string; uuid?: string }) {
    if (liveTurnClosed()) return
    const seqKey = `${kind}${opts.parentId ? ':' + opts.parentId : ''}`
    const seq = opts.seq ?? 0
    if (seq > 0) {
      const prev = streamSeq[seqKey] ?? 0
      if (seq <= prev) return
      streamSeq = { ...streamSeq, [seqKey]: seq }
    }
    if (kind === 'text' && text && !opts.parentId) turnGotText = true
    if (opts.parentId) {
      messages = applyStream(messages, kind, text, { done: !!opts.done, parentId: opts.parentId, uuid: opts.uuid })
      return
    }
    if (opts.replace || opts.done) {
      messages = applyStream(messages, kind, text, { done: !!opts.done, uuid: opts.uuid })
    } else if (text) {
      if (kind === 'text') turnGotText = true
      messages = appendStreamRow(messages, kind, text)
    }
  }

  $effect(() => {
    const on = visible
    const code = active
    void BriefingService.SetAppChatFocus(on, code).catch(() => {}).finally(refreshTabs)
    return () => { void BriefingService.SetAppChatFocus(false, code).catch(() => {}) }
  })

  $effect(() => {
    refreshTabs()
  })

  $effect(() => {
    if (catalogModels.length) return
    let cancelled = false
    loadModelCatalog().then((models) => { if (!cancelled) catalogModels = models })
    return () => { cancelled = true }
  })

  // Bumped when the active tab's session is replaced under the same code
  // (closing the primary tab resets it to a fresh chat).
  let transcriptNonce = $state(0)

  // Per-tab transcript + model selection; re-runs when the active tab changes.
  $effect(() => {
    const code = active
    void transcriptNonce
    let cancelled = false
    streamSeq = {}
    BriefingService.AppChatItems(code)
      .then((items) => {
        if (cancelled || code !== active) return
        const mapped = mapChatItems(items)
        messages = sendingBy[code] ? mergePersistedWithLive(mapped, messages) : mapped
        contextUsage = contextUsageFromChatItems(items)
      })
      .catch(() => {})
    BriefingService.AppChatSelection(code)
      .then((sel) => {
        if (cancelled || code !== active) return
        const d = resolvedDefaultForMode('ask', $settings, catalogModels)
        model = sel?.model || d.model
        flags = normalizeFlags(flagsForModel(catalogModels, model), sel?.flags ?? d.flags)
      })
      .catch(() => {})
    return () => { cancelled = true }
  })

  $effect(() => onSessionDelta((id, code, evt) => {
    if (!isActive(id, code)) return
    applyLiveStream('text', evt.text, evt)
  }))
  $effect(() => onSessionThinking((id, code, evt) => {
    if (!isActive(id, code)) return
    if (evt.text || evt.replace || evt.done) applyLiveStream('thinking', evt.text, evt)
    else messages = noteThinkingTokens(messages, evt.tokens, evt.uuid, evt.parentId)
  }))
  $effect(() => onSessionStep((id, code, step: WorkspaceStep) => {
    if (!isActive(id, code)) return
    if (liveTurnClosed()) return
    messages = addToolRow(messages, step)
  }))
  $effect(() => onSessionToolResult((id, code, toolId, result, isError, extras) => {
    if (!isActive(id, code)) return
    messages = attachToolResult(messages, toolId, result, isError, extras)
  }))
  $effect(() => onSessionToolInput((id, code, toolId, extras) => {
    if (!isActive(id, code)) return
    messages = patchToolInput(messages, toolId, extras)
  }))
  $effect(() => onSessionNotice((id, code, text) => {
    if (!isActive(id, code)) return
    messages = applyNoticeRow(messages, text)
  }))
  $effect(() => onSessionFallback((id, code, evt) => {
    if (!isActive(id, code)) return
    messages = applyFallbackRow(messages, evt)
  }))
  $effect(() => onSessionCompact((id, code, evt) => {
    if (!isActive(id, code)) return
    messages = applyCompactRow(messages, evt)
  }))
  $effect(() => onSessionTurnEnd((id, code, turn) => {
    if (id !== '' || !tabs.some((t) => t.code === code)) return
    sendingBy = { ...sendingBy, [code]: false }
    if (code !== active) {
      // Background tab finished — pick up its unseen badge and title.
      refreshTabs()
      return
    }
    messages = endTurnRow(messages, turn.durationMs)
    const usage = contextUsageFromTurn(turn)
    if (usage) contextUsage = usage
    void BriefingService.AppChatItems(code).then((items) => {
      if (code !== active) return
      messages = mergePersistedWithLive(mapChatItems(items), messages)
      contextUsage = contextUsageFromChatItems(items) ?? contextUsage
    }).catch(() => {})
    refreshTabs()
  }))

  function selectTab(code: string) {
    if (code === active) return
    active = code
    tabs = tabs.map((t) => (t.code === code ? { ...t, unseen: false } : t))
  }

  function newTab() {
    void BriefingService.NewAppChat().then((tab) => {
      if (!tab?.code) return
      tabs = [...tabs, tab]
      active = tab.code
    }).catch(() => {})
  }

  function closeTab(code: string) {
    void BriefingService.CloseAppChat(code).then((list) => {
      if (list?.length) tabs = list
      if (active !== code) return
      if (code === APP) {
        // Same code, fresh session — clear and re-pull the empty transcript.
        messages = []
        contextUsage = null
        transcriptNonce++
      } else {
        active = APP
      }
    }).catch(() => {})
  }

  function remember(nextModel: string, nextFlags: Record<string, string>) {
    model = nextModel
    flags = nextFlags
    void BriefingService.RememberAppChatSelection(active, nextModel, nextFlags).catch(() => {})
  }

  function sendChip(text: string) {
    input = text
    queueMicrotask(() => send())
  }

  function send() {
    const text = input.trim()
    if (!text || sending) return
    const code = active
    input = ''
    sendingBy = { ...sendingBy, [code]: true }
    turnStartedAt = Date.now()
    turnGotText = false
    messages = [...messages, { me: true, body: text, optimistic: true, ts: Date.now() }]
    // Mirror the backend's first-message title stamp so the chip labels now.
    tabs = tabs.map((t) => (t.code === code && (!t.title || t.title === 'New chat')
      ? { ...t, title: text.split('\n').map((l) => l.trim()).find(Boolean) ?? t.title }
      : t))
    void BriefingService.SendAppMessage(code, text, model, flags)
      .catch(() => {})
      .finally(() => { sendingBy = { ...sendingBy, [code]: false } })
  }

  function stop() {
    void BriefingService.CancelAppMessage(active).catch(() => {})
  }

  onDestroy(() => { void BriefingService.SetAppChatFocus(false, active).catch(() => {}) })
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div
  class="app-chat"
  tabindex="-1"
  bind:this={rootEl}
  onfocusin={onPanelFocusIn}
  onfocusout={onPanelFocusOut}
  onmousedown={onPanelMouseDown}
>
  <div class="obs-side-head tabbed app-chat-head">
    <button
      type="button"
      class="obs-pane-toggle prd-chat-toggle"
      onclick={() => setInboxChatOpen(false)}
      title="Hide chat panel"
      aria-label="Hide chat panel"
      aria-pressed={true}
    >
      <Icon name="panel-left" />
    </button>
    <TabStrip class="wsv-chat-tabs app-chat-tabs" label="Ask Recur chats" activeKey={active}>
    {#each tabs as tab (tab.code)}
      <TabChip
        class="wsv-chat-tab"
        tabKey={tab.code}
        label={tab.title || 'New chat'}
        title={`${tab.title || 'New chat'} — Ask mode${tabBusy(tab.code) ? ' — running' : ''}${tab.unseen && active !== tab.code ? ' — new response' : ''}`}
        icon={askMeta.icon}
        iconTint={askMeta.tint}
        active={active === tab.code}
        lit={panelLit}
        busy={tabBusy(tab.code)}
        unseen={!!tab.unseen && active !== tab.code}
        closeLabel={`Close ${tab.title || 'chat'}`}
        onclick={() => selectTab(tab.code)}
        onclose={() => closeTab(tab.code)}
      />
    {/each}
    {#snippet trailing()}
      <button
        type="button"
        class="tab-strip-act wsv-chat-new"
        title="New chat"
        aria-label="New Ask Recur chat"
        onclick={newTab}
      >
        <Icon name="plus" />
      </button>
    {/snippet}
    </TabStrip>
  </div>
  <AgentChat
    {messages}
    bind:input
    {sending}
    {turnStartedAt}
    {contextUsage}
    surface="app"
    agentCode={active}
    resetKey={`app:${active}`}
    compact
    emptyText=""
    placeholder="Ask what needs you, or where you left off…"
    onSend={send}
    onStop={stop}
    shellActive={visible}
  >
    {#snippet empty()}
      <div class="ask-empty">
        <div>Ask about Now and Next across your workspaces.</div>
        <div class="ask-chips">
          {#each ASK_CHIPS as c}
            <button type="button" onclick={() => sendChip(c)}>{c}</button>
          {/each}
        </div>
      </div>
    {/snippet}
    {#snippet toolbar()}
      {#if catalogModels.length && model}
        <ModelPickers
          ghost up
          models={catalogModels}
          {model}
          {flags}
          disabled={sending}
          onModelChange={(ref, next) => remember(ref, next)}
          onFlagsChange={(next) => remember(model, next)}
        />
      {/if}
    {/snippet}
  </AgentChat>
</div>
