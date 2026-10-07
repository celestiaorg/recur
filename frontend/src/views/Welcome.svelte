<script lang="ts">
  import { view, openProject, openNewTask, openSettings, projects, settings } from '../lib/stores'
  import { SettingsService } from '../lib/api'
  import { CHAT_MODES, type ChatMode } from '../lib/modes'
  import { providerEnabled, PROVIDER_CLAUDE, PROVIDER_CURSOR, PROVIDER_CODEX } from '../lib/providers'
  import Kbd from '../components/Kbd.svelte'
  import wordmark from '../assets/recur-wordmark.png'

  // Claude is probed live. Cursor / Codex count as ready from saved Settings
  // (Connect mints the Cursor key) — no bridge cold-start just to paint this page.
  let detected = $state('')
  let agentChecked = $state(false)
  let detectFailed = $state(false)
  function checkAgent() {
    agentChecked = false
    detectFailed = false
    SettingsService.DetectAgent()
      .then((d) => { detected = d || ''; detectFailed = !d })
      .catch(() => { detectFailed = true })
      .finally(() => { agentChecked = true })
  }
  checkAgent()

  const claudeOn = $derived(providerEnabled($settings?.disabledProviders, PROVIDER_CLAUDE))
  const cursorOn = $derived(providerEnabled($settings?.disabledProviders, PROVIDER_CURSOR))
  const codexOn = $derived(providerEnabled($settings?.disabledProviders, PROVIDER_CODEX))
  const claudeReady = $derived(claudeOn && !!detected && !detectFailed)
  const cursorReady = $derived(cursorOn && !!$settings?.cursorApiKey)
  const codexReady = $derived(codexOn && !!( $settings?.codexApiKey || $settings?.codexChatGPTAuth ))
  const hasProvider = $derived(claudeReady || cursorReady || codexReady)
  const step1Ready = $derived(agentChecked && $settings !== null)

  const hasProject = $derived($projects.length > 0)
  const hasGitProject = $derived($projects.some((p) => p.isGit))
  const projectLabel = $derived(
    $projects.length === 1 ? ($projects[0].name || 'One project')
      : $projects.length > 1 ? `${$projects.length} projects`
        : '',
  )

  const MODE_LINE: Record<ChatMode, string> = {
    ask: 'Look around.',
    plan: 'Think first.',
    agent: 'Iterate.',
    review: 'Find issues.',
    loop: 'Hand it off.',
  }
</script>

<section class="view" class:on={$view === 'welcome'} id="v-welcome">
  <div class="scroll">
    <div class="wwash" aria-hidden="true"></div>
    <div class="welcome">
      <header class="whead">
        <div class="wmark" style:--wmark={'url("' + wordmark + '")'} role="img" aria-label="Recur"></div>
        <h1>Workspaces<span class="wmany" aria-hidden="true">[]</span><br />The right <em>mode</em>.</h1>
        <p>Open a project. Pick the right mode for the task. Mix them in one workspace, or many.</p>
      </header>

      <div class="wmodes" aria-label="Modes">
        {#each CHAT_MODES as m (m.value)}
          <div class="wmode" style:--mode-accent={m.tint}>
            <div class="wm-h">{m.hint}</div>
            <div class="wm-n">{m.label}</div>
            <p class="wm-d">{MODE_LINE[m.value]}</p>
          </div>
        {/each}
      </div>

      <div class="wstat">
        <div class="wrow">
          <div class="wrow-k">Agent</div>
          <div class="wrow-v">
            {#if hasProvider}
              <span class="wvals">
                {#if claudeReady}<span class="detected">{detected}</span>{/if}
                {#if cursorReady}<span class="detected">Cursor</span>{/if}
                {#if codexReady}<span class="detected">Codex</span>{/if}
              </span>
              <button class="lnk" onclick={() => openSettings('agent')}>Settings</button>
            {:else if !step1Ready}
              <span class="wmuted">Looking…</span>
            {:else}
              <span class="detected bad">No agent connected</span>
              <button class="lnk" onclick={() => openSettings('agent')}>Settings</button>
              <button class="lnk" onclick={checkAgent}>Check again</button>
            {/if}
          </div>
        </div>
        <div class="wrow">
          <div class="wrow-k">Project</div>
          <div class="wrow-v">
            {#if hasProject}
              <span class="wvals">{projectLabel}</span>
              <button class="lnk" onclick={() => openProject('')}>Add another</button>
            {:else}
              <span class="wmuted">None yet</span>
              <button class="lnk" onclick={() => openProject('')}>Add</button>
            {/if}
          </div>
        </div>
        <div class="wcta">
          {#if hasGitProject}
            <button class="tbtn amber" onclick={() => openNewTask()}>New workspace<Kbd text="⌘N" /></button>
          {:else}
            <button class="tbtn amber" onclick={() => openProject('')}>Add a project</button>
          {/if}
        </div>
      </div>
    </div>
  </div>
</section>
