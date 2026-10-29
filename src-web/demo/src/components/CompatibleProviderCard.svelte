<script lang="ts">
  import Icon from './Icon.svelte'
  import Select from './Select.svelte'
  import { SettingsService } from '../../bindings/recur/internal/app'
  import type { CompatibleFlagDim, CompatibleModel, CompatibleProvider } from '../../bindings/recur/internal/domain/models'
  import { persistableFlags, persistableHeaders, persistableModels, unusedKey, COMPATIBLE_WIRES } from '../lib/compatible-providers'

  let {
    provider,
    testing = false,
    testMsg = null as { ok: boolean; text: string } | null,
    onChange,
    onTest,
    onRemove,
  }: {
    provider: CompatibleProvider
    testing?: boolean
    testMsg?: { ok: boolean; text: string } | null
    onChange: (next: CompatibleProvider) => void
    onTest: () => void
    onRemove: () => void
  } = $props()

  let keyVisible = $state(false)

  function set(patch: Partial<CompatibleProvider>) {
    onChange({ ...provider, ...patch })
  }

  type HeaderRow = { key: string; value: string }

  let headerDrafts = $state<HeaderRow[]>([])
  const headerSaved = $derived(
    Object.entries(provider.extraHeaders ?? {})
      .filter((e): e is [string, string] => typeof e[1] === 'string')
      .map(([key, value]) => ({ key, value })),
  )
  const headerRows = $derived([...headerSaved, ...headerDrafts])

  function setHeader(i: number, patch: Partial<HeaderRow>) {
    const n = headerSaved.length
    if (i < n) {
      set({ extraHeaders: persistableHeaders(headerSaved.map((r, j) => (j === i ? { ...r, ...patch } : r))) })
      return
    }
    const next = headerDrafts.map((r, j) => (j === i - n ? { ...r, ...patch } : r))
    const ready = next.filter((r) => r.key.trim())
    headerDrafts = next.filter((r) => !r.key.trim())
    if (ready.length) set({ extraHeaders: persistableHeaders([...headerSaved, ...ready]) })
  }

  function removeHeader(i: number) {
    const n = headerSaved.length
    if (i < n) {
      set({ extraHeaders: persistableHeaders(headerSaved.filter((_, j) => j !== i)) })
      return
    }
    headerDrafts = headerDrafts.filter((_, j) => j !== i - n)
  }

  let drafts = $state<CompatibleModel[]>([])
  const saved = $derived(provider.models ?? [])
  const rows = $derived(saved.length || drafts.length ? [...saved, ...drafts] : [{ id: '', label: '' }])

  function setModel(i: number, patch: Partial<CompatibleModel>) {
    const n = saved.length
    if (i < n) {
      const next = persistableModels(saved.map((m, j) => (j === i ? { ...m, ...patch } : m)))
      if (next.length) set({ models: next })
      return
    }
    const next = drafts.map((m, j) => (j === i - n ? { ...m, ...patch } : m))
    const ready = persistableModels(next)
    drafts = next.filter((m) => !m.id?.trim())
    if (ready.length) set({ models: [...saved, ...ready] })
  }

  function flagKind(m: CompatibleModel): string {
    const flags = m.flags ?? []
    if (flags.length === 0) return ''
    const ids = flags.map((f) => f.id)
    if (ids.includes('effort') && ids.includes('verbosity') && flags.length === 2) return 'openai'
    if (ids.length === 1 && ids[0] === 'effort') return 'effort'
    return 'custom'
  }

  const CONTROL_PRESETS = [
    { value: '', label: 'None' },
    { value: 'effort', label: 'Effort' },
    { value: 'openai', label: 'Effort, verbosity' },
    { value: 'custom', label: 'Custom…' },
  ]
  const WIRE_OPTIONS = [{ value: '', label: '(not sent)' }, ...COMPATIBLE_WIRES]

  let customizing = $state<Set<number>>(new Set())

  function controlsKind(m: CompatibleModel, i: number): string {
    const kind = flagKind(m)
    return kind && customizing.has(i) ? 'custom' : kind
  }

  async function applyFlags(i: number, kind: string) {
    customizing = new Set([...customizing].filter((j) => j !== i))
    if (kind === 'custom') {
      customizing = new Set([...customizing, i])
      const cur = rows[i]?.flags ?? []
      if (cur.length) return
      setModel(i, { flags: [{ id: 'effort', label: 'Effort', wire: 'reasoning.effort', options: [] }] })
      return
    }
    let flags: CompatibleFlagDim[] = []
    if (kind) {
      try {
        flags = (await SettingsService.CompatibleFlagPreset(kind)) ?? []
      } catch {
        flags = []
      }
    }
    setModel(i, { flags: flags.length ? flags : undefined })
  }

  function setFlag(modelIdx: number, dimIdx: number, patch: Partial<CompatibleFlagDim>) {
    const m = rows[modelIdx]
    const flags = [...(m.flags ?? [])]
    flags[dimIdx] = { ...flags[dimIdx], ...patch }
    setModel(modelIdx, { flags: persistableFlags(flags) })
  }

  function addFlag(modelIdx: number) {
    const m = rows[modelIdx]
    const flags = m.flags ?? []
    const id = unusedKey(flags.map((f) => f.id ?? ''), 'flag')
    setModel(modelIdx, { flags: [...flags, { id, label: 'New control', wire: '', options: [] }] })
  }

  function removeFlag(modelIdx: number, dimIdx: number) {
    const m = rows[modelIdx]
    setModel(modelIdx, { flags: persistableFlags((m.flags ?? []).filter((_, j) => j !== dimIdx)) })
  }

  function setOption(modelIdx: number, dimIdx: number, optIdx: number, patch: { value?: string; label?: string; default?: boolean }) {
    const dim = rows[modelIdx]?.flags?.[dimIdx]
    if (!dim) return
    const options = [...(dim.options ?? [])]
    options[optIdx] = { ...options[optIdx], ...patch }
    if (patch.default) {
      for (let i = 0; i < options.length; i++) {
        if (i !== optIdx) options[i] = { ...options[i], default: false }
      }
    }
    setFlag(modelIdx, dimIdx, { options })
  }

  function addOption(modelIdx: number, dimIdx: number) {
    const dim = rows[modelIdx]?.flags?.[dimIdx]
    if (!dim) return
    const options = dim.options ?? []
    const value = unusedKey(options.map((o) => o.value ?? ''), 'value')
    setFlag(modelIdx, dimIdx, { options: [...options, { value, label: value }] })
  }

  function removeOption(modelIdx: number, dimIdx: number, optIdx: number) {
    const dim = rows[modelIdx]?.flags?.[dimIdx]
    if (!dim) return
    setFlag(modelIdx, dimIdx, { options: (dim.options ?? []).filter((_, j) => j !== optIdx) })
  }
</script>

<div class="set-rows code-panel-sides">
  <div class="set-row set-row-stack">
    <div class="set-copy">
      <div class="set-title">Display name</div>
      <div class="set-desc">Shown in model pickers. Slug <code>{provider.id}</code> prefixes model refs.</div>
    </div>
    <div class="path-pick">
      <input
        class="inp"
        type="text"
        autocomplete="off"
        spellcheck="false"
        aria-label="Provider display name"
        value={provider.name}
        onchange={(e) => set({ name: e.currentTarget.value })}
      />
    </div>
  </div>

  <div class="set-row set-row-stack">
    <div class="set-copy">
      <div class="set-title">Base URL</div>
      <div class="set-desc">Recur POSTs <code>{'{base}'}/v1/responses</code>. No trailing slash.</div>
    </div>
    <div class="path-pick">
      <input
        class="inp"
        type="url"
        placeholder="https://api.openai.com"
        autocomplete="off"
        spellcheck="false"
        aria-label="Base URL"
        value={provider.baseUrl}
        onchange={(e) => set({ baseUrl: e.currentTarget.value })}
      />
    </div>
  </div>

  <div class="set-row set-row-stack">
    <div class="set-copy">
      <div class="set-title">API key</div>
      <div class="set-desc">Sent as <code>Authorization: Bearer</code>. Optional for local servers.</div>
    </div>
    <div class="path-pick">
      <input
        class="inp"
        type={keyVisible ? 'text' : 'password'}
        placeholder="sk-…"
        autocomplete="off"
        spellcheck="false"
        aria-label="API key"
        value={provider.apiKey ?? ''}
        onchange={(e) => set({ apiKey: e.currentTarget.value })}
      />
      <button
        class="tbtn"
        type="button"
        aria-label={keyVisible ? 'Hide API key' : 'Show API key'}
        aria-pressed={keyVisible}
        disabled={!provider.apiKey}
        onclick={() => { keyVisible = !keyVisible }}
      >
        <Icon name={keyVisible ? 'eye-off' : 'eye'} />
      </button>
      <button class="tbtn" type="button" disabled={testing || !provider.baseUrl} onclick={onTest}>Test</button>
    </div>
    {#if testing}
      <div class="set-field-msg muted">Testing…</div>
    {:else if testMsg}
      <div class="set-field-msg" class:ok={testMsg.ok} class:bad={!testMsg.ok}>{testMsg.text}</div>
    {/if}
  </div>

  <div class="set-row set-row-stack">
    <div class="set-copy">
      <div class="set-title">Extra headers</div>
      <div class="set-desc">Optional. Sent on every request to this endpoint.</div>
    </div>
    <div class="compat-models">
      {#each headerRows as h, i (i)}
        <div class="compat-header">
          <input
            class="inp"
            type="text"
            placeholder="Key"
            autocomplete="off"
            spellcheck="false"
            aria-label="Header key"
            value={h.key}
            onchange={(e) => setHeader(i, { key: e.currentTarget.value })}
          />
          <input
            class="inp"
            type="text"
            placeholder="Value"
            autocomplete="off"
            spellcheck="false"
            aria-label="Header value"
            value={h.value}
            onchange={(e) => setHeader(i, { value: e.currentTarget.value })}
          />
          <button type="button" class="exclude-rm" aria-label="Remove header" onclick={() => removeHeader(i)}>
            <Icon name="trash" />
          </button>
        </div>
      {/each}
      <div class="exclude-actions">
        <button type="button" class="tbtn" onclick={() => { headerDrafts = [...headerDrafts, { key: '', value: '' }] }}>
          <Icon name="plus" />Add header
        </button>
      </div>
    </div>
  </div>

  <div class="set-row set-row-stack">
    <div class="set-copy">
      <div class="set-title">Models</div>
      <div class="set-desc">Each id is stored as <code>{provider.id}/id</code>. Slash in the id is allowed (OpenRouter).</div>
    </div>
    <div class="compat-models">
      <div class="compat-model compat-cols" aria-hidden="true">
        <span>Model id</span><span>Label</span><span title="Shown in the chat context meter. Not sent to the server.">Context window</span><span>Picker controls</span><span></span>
      </div>
      {#each rows as m, i (i)}
        {@const kind = controlsKind(m, i)}
        <div class="compat-model-block">
          <div class="compat-model">
            <input
              class="inp"
              type="text"
              placeholder="model-id"
              autocomplete="off"
              spellcheck="false"
              aria-label="Model id"
              value={m.id}
              onchange={(e) => setModel(i, { id: e.currentTarget.value, label: m.label || e.currentTarget.value })}
            />
            <input
              class="inp"
              type="text"
              placeholder="Label"
              autocomplete="off"
              spellcheck="false"
              aria-label="Model label"
              value={m.label}
              onchange={(e) => setModel(i, { label: e.currentTarget.value })}
            />
            <input
              class="inp"
              type="number"
              min="0"
              step="1"
              placeholder="128000"
              autocomplete="off"
              aria-label="Context window tokens"
              value={m.contextWindow || ''}
              onchange={(e) => {
                const n = Number(e.currentTarget.value)
                setModel(i, { contextWindow: Number.isFinite(n) && n > 0 ? Math.round(n) : 0 })
              }}
            />
            <Select
              label="Picker controls"
              value={kind}
              options={CONTROL_PRESETS}
              onchange={(v) => void applyFlags(i, v)}
            />
            <button
              type="button"
              class="exclude-rm"
              aria-label="Remove model"
              disabled={rows.length <= 1 || (i < saved.length && saved.length <= 1)}
              onclick={() => {
                customizing = new Set([...customizing].filter((j) => j !== i).map((j) => (j > i ? j - 1 : j)))
                const n = saved.length
                if (i < n) {
                  const next = saved.filter((_, j) => j !== i)
                  if (next.length) set({ models: next })
                  return
                }
                drafts = drafts.filter((_, j) => j !== i - n)
              }}
            >
              <Icon name="trash" />
            </button>
          </div>
          {#if kind && kind !== 'custom'}
            <div class="compat-flag-summary">
              <div class="compat-flag-summary-lead">
                Adds {(m.flags ?? []).length === 1 ? 'this dropdown' : 'these dropdowns'} to the composer's model picker. Choose Custom… to edit.
              </div>
              {#each m.flags ?? [] as dim (dim.id)}
                <span class="compat-flag-name">{dim.label || dim.id}</span>
                <span>
                  {#each dim.options ?? [] as opt, oi (opt.value)}{oi ? ', ' : ''}<span class:compat-flag-default={opt.default}>{opt.label || opt.value}{opt.default ? ' (default)' : ''}</span>{/each}
                </span>
                <span>{#if dim.wire}sends <code>{dim.wire}</code>{:else}not sent{/if}</span>
              {/each}
            </div>
          {:else if kind === 'custom'}
            <div class="compat-flags">
              <div class="set-desc">
                Each control is a dropdown in the composer's model picker. The chosen value is sent as the request field.
              </div>
              {#each m.flags ?? [] as dim, di (dim.id || di)}
                <div class="compat-flag-card">
                  <div class="compat-flag">
                    <input
                      class="inp compat-flag-label"
                      type="text"
                      placeholder="Control name"
                      autocomplete="off"
                      spellcheck="false"
                      aria-label="Control name"
                      value={dim.label}
                      onchange={(e) => setFlag(i, di, { label: e.currentTarget.value })}
                    />
                    <span class="compat-cols">sent as</span>
                    <Select
                      size="sm"
                      label="Request field"
                      value={dim.wire ?? ''}
                      options={WIRE_OPTIONS}
                      onchange={(v) => setFlag(i, di, { wire: v || undefined })}
                    />
                    <button type="button" class="exclude-rm" aria-label="Remove control" onclick={() => removeFlag(i, di)}>
                      <Icon name="trash" />
                    </button>
                  </div>
                  <div class="compat-flag-opts">
                    <span class="compat-cols" aria-hidden="true">Default</span>
                    <span class="compat-cols" aria-hidden="true">Value sent</span>
                    <span class="compat-cols" aria-hidden="true">Shown as</span>
                    <span></span>
                    {#each dim.options ?? [] as opt, oi (opt.value || oi)}
                      <button
                        type="button"
                        class="compat-default"
                        class:on={opt.default}
                        aria-label="Default {opt.label || opt.value}"
                        aria-pressed={!!opt.default}
                        onclick={() => setOption(i, di, oi, { default: !opt.default })}
                      >
                        <Icon name="check" strokeWidth={3} />
                      </button>
                      <input
                        class="inp"
                        type="text"
                        placeholder="value"
                        autocomplete="off"
                        spellcheck="false"
                        aria-label="Option value"
                        value={opt.value}
                        onchange={(e) => setOption(i, di, oi, { value: e.currentTarget.value, label: opt.label || e.currentTarget.value })}
                      />
                      <input
                        class="inp"
                        type="text"
                        placeholder="Label"
                        autocomplete="off"
                        spellcheck="false"
                        aria-label="Option label"
                        value={opt.label}
                        onchange={(e) => setOption(i, di, oi, { label: e.currentTarget.value })}
                      />
                      <button type="button" class="exclude-rm" aria-label="Remove option" onclick={() => removeOption(i, di, oi)}>
                        <Icon name="trash" />
                      </button>
                    {/each}
                  </div>
                  <button type="button" class="tbtn compat-add" onclick={() => addOption(i, di)}>
                    <Icon name="plus" />Add option
                  </button>
                </div>
              {/each}
              <button type="button" class="tbtn compat-add" onclick={() => addFlag(i)}>
                <Icon name="plus" />Add control
              </button>
            </div>
          {/if}
        </div>
      {/each}
      <div class="exclude-actions">
        <button type="button" class="tbtn" onclick={() => { drafts = [...drafts, { id: '', label: '' }] }}>
          <Icon name="plus" />Add model
        </button>
      </div>
    </div>
  </div>

  <div class="set-row">
    <div class="set-copy">
      <div class="set-title">Remove provider</div>
      <div class="set-desc">Drops this endpoint and its models from pickers.</div>
    </div>
    <div class="set-ctrl">
      <button class="tbtn set-danger-btn" type="button" onclick={onRemove}>Remove</button>
    </div>
  </div>
</div>
