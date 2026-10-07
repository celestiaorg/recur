<script lang="ts">
  import { view, go, openProject, openNewTask, openSettings, projects, workspaces, settings, pushToast } from '../lib/stores'
  import { SettingsService, call } from '../lib/api'
  import { Events } from '@wailsio/runtime'
  import { CHAT_MODES, wsHasRun, type ChatMode } from '../lib/modes'
  import type { Workspace } from '../../bindings/recur/internal/domain/models'
  import { providerEnabled, PROVIDER_CLAUDE, PROVIDER_CURSOR, PROVIDER_CODEX } from '../lib/providers'
  import { untrack, type Snippet } from 'svelte'
  import Icon from '../components/Icon.svelte'
  import ProviderIcon from '../components/ProviderIcon.svelte'
  import ProjectPic from '../components/ProjectPic.svelte'
  import Kbd from '../components/Kbd.svelte'
  import wordmark from '../assets/recur-wordmark.png'

  // All three are probed live: a ChatGPT-login toggle alone does not mean
  // `codex login` ever ran, and a Cursor key without the SDK bridge cannot
  // run anything. The view stays mounted, so probes run only while it is shown
  // (and again on each return from Settings), and only for providers that are on.
  let detected = $state('')
  let detectFailed = $state(false)
  let codexAuth = $state<{ authenticated?: boolean; email?: string; loginMethod?: string } | null>(null)
  let cursorStatus = $state<{ bridgeInstalled?: boolean; authenticated?: boolean; email?: string; account?: string } | null>(null)
  let claudeChecked = $state(false)
  let codexChecked = $state(false)
  let cursorChecked = $state(false)
  function checkAgents() {
    if (claudeOn) {
      SettingsService.DetectAgent()
        .then((d) => { detected = d || ''; detectFailed = !d })
        .catch(() => { detectFailed = true })
        .finally(() => { claudeChecked = true })
    }
    if (codexOn) {
      SettingsService.GetCodexStatus()
        .then((st) => { codexAuth = st ?? null })
        .catch(() => { codexAuth = null })
        .finally(() => { codexChecked = true })
    }
    if (cursorOn) {
      SettingsService.GetCursorStatus()
        .then((st) => { cursorStatus = st ?? null })
        .catch(() => { cursorStatus = null })
        .finally(() => { cursorChecked = true })
    }
  }
  const settingsLoaded = $derived($settings !== null)
  $effect(() => {
    if ($view === 'welcome' && settingsLoaded) untrack(checkAgents)
  })

  // The Cursor SDK bridge (~50 MB download) is fetched on demand; progress
  // arrives as `cursor.install` bus events while InstallCursorBridge runs.
  let installing = $state(false)
  let installPct = $state(-1)
  let installMsg = $state('')
  $effect(() => {
    const off = Events.On('cursor.install', (e: { data?: { message?: string; percent?: number } }) => {
      installMsg = e.data?.message ?? ''
      installPct = typeof e.data?.percent === 'number' ? e.data.percent : -1
    })
    return () => { off?.() }
  })
  function installCursorSdk() {
    installing = true
    installMsg = 'Starting…'
    installPct = -1
    call(SettingsService.InstallCursorBridge(), 'Could not install the Cursor SDK')
      .then((st) => {
        if (!st) return
        cursorStatus = st
        if (st.bridgeInstalled) pushToast('success', 'Cursor SDK installed')
      })
      .finally(() => { installing = false; installMsg = ''; installPct = -1 })
  }

  const claudeOn = $derived(providerEnabled($settings?.disabledProviders, PROVIDER_CLAUDE))
  const cursorOn = $derived(providerEnabled($settings?.disabledProviders, PROVIDER_CURSOR))
  const codexOn = $derived(providerEnabled($settings?.disabledProviders, PROVIDER_CODEX))
  const claudeReady = $derived(claudeOn && !!detected && !detectFailed)
  const cursorSignedIn = $derived(cursorOn && !!$settings?.cursorApiKey)
  const cursorNeedsSdk = $derived(cursorSignedIn && cursorChecked && !cursorStatus?.bridgeInstalled)
  const cursorReady = $derived(cursorSignedIn && !!cursorStatus?.bridgeInstalled)
  const codexReady = $derived(codexOn && !!codexAuth?.authenticated)
  const hasProvider = $derived(claudeReady || cursorReady || codexReady)
  const claudeDetail = $derived(detected.replace(/^Claude Code(\s*·\s*)?/, '') || 'Signed in')
  const codexDetail = $derived(codexAuth?.email || codexAuth?.loginMethod || 'Signed in')

  let skipAgent = $state(false)
  const gitProjects = $derived($projects.filter((p) => p.isGit))
  const hasGitProject = $derived(gitProjects.length > 0)
  // A task counts once the user has actually sent something to an agent: a
  // loop engaged with a goal, a chat turn that finished (LastActivityAt), or
  // the first message renaming the workspace away from its "New Workspace"
  // placeholder. The New-workspace modal eagerly creates (and on cancel
  // deletes) a record, so a bare record must not flip this step.
  const taskStarted = (w: Workspace) =>
    !w.archived && !w.isMain && !(w.hostPrId ?? '').trim()
    && (wsHasRun(w) || (w.lastActivityAt ?? 0) > 0 || (!!w.goal?.trim() && w.goal.trim() !== 'New Workspace'))
  const liveWorkspaces = $derived($workspaces.filter(taskStarted).length)
  const hasWorkspace = $derived(liveWorkspaces > 0)

  // The flow is derived from real state, so it resumes wherever the user left off.
  // Until the first probes settle, a started task is taken as proof an agent works.
  const probing = $derived((claudeOn && !claudeChecked) || (codexOn && !codexChecked) || (cursorOn && !cursorChecked))
  const step = $derived(
    !(hasProvider || skipAgent || (probing && hasWorkspace)) ? 1
      : !hasGitProject ? 2
        : !hasWorkspace ? 3
          : 4,
  )

  const EXAMPLES: Record<ChatMode, { line: string; prompt: string }> = {
    ask: { line: 'Answers about your code. No edits.', prompt: 'What are the riskiest parts of this codebase, and why?' },
    plan: { line: 'An approach before any change.', prompt: 'Plan how to add end-to-end tests for the main user flow.' },
    agent: { line: 'Edits files and runs checks with you.', prompt: 'Refactor the largest function in this repo into smaller pieces.' },
    review: { line: 'Findings on a diff.', prompt: 'Review the last commit for bugs and missing tests.' },
    loop: { line: 'A whole process, hands-off: coding, review cycles, docs, or your own.', prompt: 'Audit error handling across the codebase, fix what is unsafe, and re-review until clean.' },
  }

  function startWith(mode: ChatMode) {
    openNewTask(gitProjects[0]?.id ?? '', { text: '', mode })
  }

  const EXPLORE = [
    { icon: 'inbox', name: 'Inbox', line: 'What needs you, across every workspace.', run: () => go('briefing') },
    { icon: 'git-pull-request', name: 'Pull requests', line: 'Review, comment, and merge without leaving Recur.', run: () => go('pr') },
    { icon: 'refresh-cw', name: 'Loops', line: 'Pick the default loop and how cautious it should be.', run: () => openSettings('loops') },
    { icon: 'braces', name: 'Bundles', line: 'Commands, prompts, grammars, and themes you can extend.', run: () => openSettings('bundle-commands') },
  ] as const
</script>

{#snippet stepHead(n: number, title: string, detail?: Snippet)}
  <div class="ws-head">
    <span class={['ws-num', { done: step > n, current: step === n }]} aria-hidden="true">
      {#if step > n}<Icon name="check" size={13} />{:else}{n}{/if}
    </span>
    <span class="ws-title">{title}</span>
    {#if step > n && detail}<span class="ws-chips">{@render detail()}</span>{/if}
  </div>
{/snippet}

{#snippet providerChips()}
  {#if claudeReady}<span class="ws-chip"><span class="ws-chip-ico"><ProviderIcon provider="claude" /></span>Claude Code</span>{/if}
  {#if cursorReady}<span class="ws-chip"><span class="ws-chip-ico"><ProviderIcon provider="cursor" /></span>Cursor</span>{/if}
  {#if codexReady}<span class="ws-chip"><span class="ws-chip-ico"><ProviderIcon provider="codex" /></span>Codex</span>{/if}
{/snippet}

{#snippet projectChips()}
  {#each gitProjects.slice(0, 3) as p (p.id)}
    <span class="ws-chip"><ProjectPic class="ws-chip-pic" pic={p.pic} color={p.color} name={p.name} />{p.name}</span>
  {/each}
  {#if gitProjects.length > 3}<span class="ws-chip muted">+{gitProjects.length - 3}</span>{/if}
{/snippet}

{#snippet workspaceChip()}
  <span class="ws-chip"><span class="ws-chip-ico"><Icon name="git-branch" size={12} /></span>{liveWorkspaces} {liveWorkspaces === 1 ? 'workspace' : 'workspaces'}</span>
{/snippet}

<section class="view" class:on={$view === 'welcome'} id="v-welcome">
  <div class="scroll">
    <div class="welcome">
      <div class="wmark" style:--wmark={'url("' + wordmark + '")'} role="img" aria-label="Recur"></div>
      <h1>{step === 4 ? 'You’re set up.' : 'Set up Recur'}</h1>

      <ol class="wsteps">
        <li class={['ws', { current: step === 1 }]} data-step="1">
          {@render stepHead(1, 'Connect an agent', step === 4 ? providerChips : undefined)}
          {#if step === 4}
            <button class="lnk ws-head-link" onclick={() => openSettings('agent')}>Providers</button>
          {/if}
          {#if step < 4}
            <div class="ws-body">
              {#if !settingsLoaded}
                <p class="ws-note" role="status">Checking Claude Code and Codex…</p>
              {:else}
                <ul class="wprov">
                  <li class:on={claudeReady} data-provider="claude">
                    <span class="wprov-name">Claude Code</span>
                    {#if claudeReady}
                      <span class="wprov-note on"><Icon name="check" size={13} />{claudeDetail}</span>
                    {:else if !claudeOn}
                      <span class="wprov-note">Turned off.</span>
                      <button class="lnk" onclick={() => openSettings('agent')}>Turn on</button>
                    {:else if !claudeChecked}
                      <span class="wprov-note" role="status">Checking…</span>
                    {:else}
                      <span class="wprov-note">Not signed in. Run <code>claude</code> in a terminal.</span>
                      <button class="lnk" onclick={checkAgents}>Check again</button>
                    {/if}
                  </li>
                  <li class:on={codexReady} data-provider="codex">
                    <span class="wprov-name">Codex</span>
                    {#if codexReady}
                      <span class="wprov-note on"><Icon name="check" size={13} />{codexDetail}</span>
                    {:else if !codexOn}
                      <span class="wprov-note">Turned off.</span>
                      <button class="lnk" onclick={() => openSettings('agent')}>Turn on</button>
                    {:else if !codexChecked}
                      <span class="wprov-note" role="status">Checking…</span>
                    {:else if $settings?.codexChatGPTAuth}
                      <span class="wprov-note">Not signed in. Run <code>codex login</code> in a terminal.</span>
                      <button class="lnk" onclick={checkAgents}>Check again</button>
                    {:else}
                      <span class="wprov-note">Sign in with ChatGPT or an API key.</span>
                      <button class="lnk" onclick={() => openSettings('agent')}>Connect</button>
                    {/if}
                  </li>
                  <li class:on={cursorReady} class:warn={cursorNeedsSdk} data-provider="cursor">
                    <span class="wprov-name">Cursor</span>
                    {#if cursorReady}
                      <span class="wprov-note on"><Icon name="check" size={13} />{cursorStatus?.email || cursorStatus?.account || 'Signed in'}</span>
                    {:else if cursorNeedsSdk}
                      <span class="wprov-note warn">
                        <Icon name="triangle-alert" size={13} />
                        {#if installing}
                          <span>{installMsg || 'Installing…'}{installPct >= 0 ? ` · ${installPct}%` : ''}</span>
                        {:else}
                          <span>Signed in, but the Cursor SDK is not installed. Recur needs it to run Cursor agents (about 50 MB).</span>
                        {/if}
                      </span>
                      <button class="tbtn" disabled={installing} onclick={installCursorSdk}>{installing ? 'Installing…' : 'Install SDK'}</button>
                    {:else if !cursorOn}
                      <span class="wprov-note">Turned off.</span>
                      <button class="lnk" onclick={() => openSettings('agent')}>Turn on</button>
                    {:else if cursorSignedIn && !cursorChecked}
                      <span class="wprov-note" role="status">Checking…</span>
                    {:else}
                      <span class="wprov-note">Sign in with your Cursor account.</span>
                      <button class="lnk" onclick={() => openSettings('agent')}>Connect</button>
                    {/if}
                  </li>
                </ul>
                <div class="ws-links">
                  <button class="lnk" onclick={() => openSettings('agent')}>Provider settings</button>
                  {#if step === 1}
                    <button class="lnk" onclick={() => { skipAgent = true }}>Continue without an agent</button>
                  {/if}
                </div>
              {/if}
            </div>
          {/if}
        </li>

        <li class={['ws', { current: step === 2 }]} data-step="2">
          {@render stepHead(2, 'Add a project', projectChips)}
          {#if step === 2}
            <div class="ws-body">
              <p class="ws-note">Pick a local Git repository. Recur creates a worktree per task, so your checkout stays untouched.</p>
              <button class="tbtn amber" onclick={() => openProject('')}>Choose a repository</button>
            </div>
          {/if}
        </li>

        <li class={['ws', { current: step === 3 }]} data-step="3">
          {@render stepHead(3, 'Start your first task', workspaceChip)}
          {#if step === 3}
            <div class="ws-body">
              <p class="ws-note">Each task gets its own workspace and a chat in one of five modes. Pick the mode to start in.</p>
              <ul class="wmodes">
                {#each CHAT_MODES as m (m.value)}
                  <li>
                    <button class="wmode" style:--mode-accent={m.tint} onclick={() => startWith(m.value)} aria-label={`Start in ${m.label} mode`}>
                      <span class="wmode-ico"><Icon name={m.icon} size={15} /></span>
                      <span class="wmode-text">
                        <span class="wmode-name">{m.label}<span class="wmode-line">{EXAMPLES[m.value].line}</span></span>
                        <span class="wmode-prompt">“{EXAMPLES[m.value].prompt}”</span>
                      </span>
                      <Icon name="chevron-right" size={14} class="wmode-go" />
                    </button>
                  </li>
                {/each}
              </ul>
              <button class="lnk ws-skip" onclick={() => openNewTask(gitProjects[0]?.id ?? '')}>New workspace<Kbd text="⌘N" /></button>
            </div>
          {/if}
        </li>
      </ol>

      {#if step === 4}
        <section class="wexplore" aria-labelledby="wexplore-h">
          <h2 id="wexplore-h">Worth knowing about</h2>
          <ul>
            {#each EXPLORE as e (e.name)}
              <li>
                <button class="wexplore-row" onclick={e.run}>
                  <Icon name={e.icon} size={16} />
                  <span class="wexplore-text"><b>{e.name}</b><span>{e.line}</span></span>
                  <Icon name="chevron-right" size={14} class="wmode-go" />
                </button>
              </li>
            {/each}
          </ul>
          <div class="wexplore-foot">
            <button class="tbtn amber" onclick={() => openNewTask()}>New workspace<Kbd text="⌘N" /></button>
          </div>
        </section>
      {/if}
    </div>
  </div>
</section>
