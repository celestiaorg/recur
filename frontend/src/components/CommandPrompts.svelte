<script lang="ts">
  import type { CommandPrompt } from '../lib/commands'
  import Icon from './Icon.svelte'

  let {
    open = false,
    title = 'Command',
    prompts = [],
    onResolve,
    onCancel,
  }: {
    open?: boolean
    title?: string
    prompts?: CommandPrompt[]
    onResolve?: (values: Record<string, string>) => void
    onCancel?: () => void
  } = $props()

  let values = $state<Record<string, string>>({})
  let formEl = $state<HTMLFormElement | null>(null)

  $effect(() => {
    if (!open) return
    const next: Record<string, string> = {}
    for (const p of prompts) {
      next[p.name] = p.default ?? ''
    }
    values = next
    setTimeout(() => {
      const el = formEl?.querySelector<HTMLElement>('input, select')
      el?.focus()
    }, 0)
  })

  function submit() {
    onResolve?.({ ...values })
  }

  function onKey(e: KeyboardEvent) {
    if (e.key === 'Escape') {
      e.preventDefault()
      onCancel?.()
    } else if (e.key === 'Enter' && !(e.target instanceof HTMLTextAreaElement)) {
      e.preventDefault()
      submit()
    }
  }
</script>

{#if open}
  <div
    class="palette-bg open"
    role="presentation"
    onclick={(e) => { if (e.target === e.currentTarget) onCancel?.() }}
  >
    <div class="palette cmd-prompts" role="dialog" tabindex="-1" aria-modal="true" aria-label={title} onkeydown={onKey}>
      <div class="pal-in">
        <Icon name="settings" />
        <span class="cmd-prompts-title">{title}</span>
        <span class="esc">esc</span>
      </div>
      <form class="cmd-prompts-body" bind:this={formEl} onsubmit={(e) => { e.preventDefault(); submit() }}>
        {#each prompts as p (p.name)}
          <label class="cmd-prompt-field">
            <span class="cmd-prompt-label">{p.label || p.name}</span>
            {#if p.type === 'choice' && p.choices?.length}
              <select bind:value={values[p.name]}>
                {#each p.choices as c}
                  <option value={c}>{c}</option>
                {/each}
              </select>
            {:else}
              <input
                type={p.type === 'password' ? 'password' : 'text'}
                bind:value={values[p.name]}
                autocomplete="off"
                spellcheck="false"
              />
            {/if}
          </label>
        {/each}
      </form>
      <div class="cmd-prompts-actions">
        <button type="button" class="linkish" onclick={() => onCancel?.()}>Cancel</button>
        <button type="button" class="primary" onclick={submit}>Run</button>
      </div>
    </div>
  </div>
{/if}

<style>
  .cmd-prompts { max-width: 420px; }
  .cmd-prompts-title {
    flex: 1;
    font-size: 13px;
    font-weight: 600;
    color: var(--text);
  }
  .cmd-prompts-body {
    padding: 12px 14px;
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  .cmd-prompt-field {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .cmd-prompt-label {
    font-size: 11px;
    color: var(--muted);
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }
  .cmd-prompt-field input,
  .cmd-prompt-field select {
    background: var(--pane-2, var(--pane));
    border: 1px solid var(--border, #2a2f38);
    border-radius: 6px;
    color: var(--text);
    font: inherit;
    font-size: 13px;
    padding: 7px 9px;
  }
  .cmd-prompts-actions {
    display: flex;
    justify-content: flex-end;
    gap: 10px;
    padding: 10px 14px 12px;
    border-top: 1px solid var(--border, #2a2f38);
  }
  .cmd-prompts-actions .primary {
    background: var(--accent, var(--amber));
    color: #0A0C10;
    border: 0;
    border-radius: 6px;
    padding: 6px 12px;
    font-weight: 600;
    cursor: pointer;
  }
</style>
