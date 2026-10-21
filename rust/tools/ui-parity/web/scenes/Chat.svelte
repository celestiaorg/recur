<script lang="ts">
  // Chat scenes (`chat` in scenes.json): AgentChat as the workspace dock mounts it (compact,
  // the Loop or agent tab's copy, a mode toolbar on the `mode-*` scenes) and AppChat, over
  // the fixtures in `content/chat.json` that `recur_view_chat::gallery` reads too. Ready once
  // the catalog, the prompt index and the transcript have painted.
  import { onMount, tick } from 'svelte'
  import AgentChat from '../../../../../frontend/src/components/AgentChat.svelte'
  import AppChat from '../../../../../frontend/src/components/AppChat.svelte'
  import Select from '../../../../../frontend/src/components/Select.svelte'
  import ModelPickers from '../../../../../frontend/src/components/ModelPickers.svelte'
  import LoopPolicyPickers from '../../../../../frontend/src/components/LoopPolicyPickers.svelte'
  import { applyStream, mapChatItems, type AgentMsg } from '../../../../../frontend/src/lib/agentchat'
  import { mergeLoopCockpit } from '../../../../../frontend/src/lib/loop-cockpit'
  import { chatModeOptions, resolvedDefaultForMode } from '../../../../../frontend/src/lib/modes'
  import { flagsForModel, loadModelCatalog, normalizeFlags, type CatalogModel } from '../../../../../frontend/src/lib/models'
  import { permissionRequests, userQuestions, view } from '../../../../../frontend/src/lib/stores'
  import fixtures from '../../content/chat.json'

  let { name, onready }: { name: string; onready: () => void } = $props()
  const variant = name.split('/').slice(1).join('/').replace(/-paper$/, '')
  const mode = variant.startsWith('mode-') ? variant.slice(5) : variant === 'pickers' ? 'agent' : ''
  const code = mode && mode !== 'loop' ? 'PA' : 'MA'
  const fx = (fixtures as Record<string, any>)[variant] ?? {}

  let messages = $state<AgentMsg[]>([])
  let input = $state(fx.draft ?? '')
  let models = $state<CatalogModel[]>([])
  let model = $state('')
  let flags = $state<Record<string, string>>({})
  let chat: ReturnType<typeof AgentChat> | undefined = $state()
  let mounted = $state(false)
  const turnStartedAt = Date.now()

  const sending = !!fx.sending
  const emptyText = code === 'MA'
    ? 'Ask where this loop stands, or steer it — the Maestro can read the worktree, edit the plan, re-run review, and dispatch specialists; rails hold at execution.'
    : 'Ask the agent, or tell it what to change — the mode selector below decides whether it may edit.'
  const placeholder = sending
    ? 'Add a follow-up… (@ files, / skills, ⏎ to queue)'
    : code === 'MA'
      ? 'Ask where things stand, or steer the loop… (@ files, / skills)'
      : 'Message the agent… (@ files, / skills, ⏎ to send)'

  function build(): AgentMsg[] {
    let ma = mapChatItems(fx.items ?? [])
    if (fx.stream) ma = applyStream(ma, 'text', fx.stream, {})
    return mergeLoopCockpit(ma, [], fx.facts ?? [], [], '')
  }

  const frames = (n: number) =>
    new Promise<void>((r) => {
      const step = () => (--n > 0 ? requestAnimationFrame(step) : r())
      requestAnimationFrame(step)
    })

  onMount(async () => {
    view.set('detail')
    if (fx.permission) permissionRequests.set([fx.permission])
    if (fx.question) userQuestions.set([fx.question])
    await document.fonts.ready
    if (mode) {
      models = await loadModelCatalog()
      const d = resolvedDefaultForMode(mode, undefined, models)
      model = d.model
      flags = normalizeFlags(flagsForModel(models, model), d.flags)
    }
    messages = build()
    mounted = true
    await tick()
    if (variant === 'composer') chat?.focusComposer()
    // The prompt index and file icons load on first paint.
    await new Promise((r) => setTimeout(r, 300))
    await frames(3)
    onready()
  })
</script>

<div class="chat-scene">
  {#if !mounted}
  {:else if variant === 'app'}
    <!-- The Briefing body's pane-chrome height, which the tab bar reads. -->
    <div class="app-host"><AppChat /></div>
  {:else}
    <AgentChat
      bind:this={chat}
      {messages}
      bind:input
      {sending}
      canStop={sending}
      {turnStartedAt}
      compact
      resetKey={`LOOP-2231:${code}`}
      {emptyText}
      {placeholder}
      rootPath="/mock/worktrees/LOOP-2231"
      projectId="storefront"
      workspaceId="LOOP-2231"
      agentCode={code}
      onSend={() => {}}
      onStop={() => {}}
      onRetry={() => {}}
      onRemoveQueued={() => {}}
      onCycleMode={() => {}}
      onOpenFile={() => {}}
      onOpenArtifact={() => {}}
      onFork={() => {}}
      onForkToWorkspace={() => {}}
      onPromptAction={() => {}}
    >
      {#snippet toolbar()}
        {#if mode}
          <Select ghost up class="wsv-mode" label="Chat mode" options={chatModeOptions()} value={mode} onchange={() => {}} />
          {#if mode === 'loop'}
            <LoopPolicyPickers
              loopId={fixtures.loops[0].id}
              policyId={fixtures.policies[0].id}
              loops={fixtures.loops}
              policies={fixtures.policies}
            />
          {:else if models.length && model}
            <ModelPickers ghost up {models} {model} {flags} />
          {/if}
        {/if}
      {/snippet}
    </AgentChat>
  {/if}
</div>

<style>
  .chat-scene { position: absolute; inset: 0; display: flex; flex-direction: column; }
  .app-host { --obs-chrome-h: 40px; flex: 1; min-height: 0; display: flex; flex-direction: column; }
</style>
