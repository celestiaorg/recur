import type { CompatibleFlagDim, CompatibleModel, CompatibleProvider } from '../../bindings/recur/internal/domain/models'

export const COMPATIBLE_PRESETS = [
  { id: 'openai', name: 'OpenAI', baseUrl: 'https://api.openai.com', flags: 'openai', modelId: 'gpt-5', modelLabel: 'GPT-5' },
  { id: 'openrouter', name: 'OpenRouter', baseUrl: 'https://openrouter.ai/api', flags: '', modelId: 'openai/gpt-4o', modelLabel: 'GPT-4o' },
  { id: 'groq', name: 'Groq', baseUrl: 'https://api.groq.com/openai', flags: '', modelId: '', modelLabel: '' },
  { id: 'custom', name: 'Custom', baseUrl: '', flags: '', modelId: '', modelLabel: '' },
] as const

export const COMPATIBLE_WIRES = [
  { value: 'reasoning.effort', label: 'reasoning.effort' },
  { value: 'reasoning.summary', label: 'reasoning.summary' },
  { value: 'text.verbosity', label: 'text.verbosity' },
  { value: 'temperature', label: 'temperature' },
  { value: 'max_output_tokens', label: 'max_output_tokens' },
] as const

export const RESERVED_PROVIDER_SLUGS = ['claude-code', 'cursor', 'codex', 'chatgpt', 'anthropic', 'gemini', 'ollama']

export const COMPATIBLE_SLUG_RE = /^[a-z][a-z0-9-]{0,47}$/

export function compatibleSlugError(id: string, taken: string[] = []): string {
  const slug = id.trim().toLowerCase()
  if (!COMPATIBLE_SLUG_RE.test(slug)) {
    return 'Slug must be kebab-case (a-z, digits, hyphen), starting with a letter.'
  }
  if (RESERVED_PROVIDER_SLUGS.includes(slug)) {
    return `Slug ${slug} is reserved.`
  }
  if (taken.includes(slug)) {
    return `A provider named ${slug} already exists.`
  }
  return ''
}

export function compatibleConfigured(p: CompatibleProvider | undefined | null): boolean {
  return !!p?.baseUrl?.trim() && (p.models?.some((m) => !!m.id.trim()) ?? false)
}

/** Empty ids are dropped by NormalizeCompatibleProviders — keep those rows local. */
export function persistableModels(models: CompatibleModel[]): CompatibleModel[] {
  return models.filter((m) => !!m.id?.trim())
}

/** Empty keys are dropped by cleanExtraHeaders — keep those rows local. */
export function persistableHeaders(rows: { key: string; value: string }[]): { [k: string]: string } | undefined {
  const out: { [k: string]: string } = {}
  for (const r of rows) {
    const k = r.key?.trim()
    if (!k) continue
    out[k] = r.value ?? ''
  }
  return Object.keys(out).length ? out : undefined
}

/** Next unused key: prefix, then prefix2, prefix3, … (skips ids already taken). */
export function unusedKey(taken: Iterable<string>, prefix: string): string {
  const have = new Set<string>()
  for (const s of taken) {
    const t = s.trim()
    if (t) have.add(t)
  }
  if (!have.has(prefix)) return prefix
  for (let i = 2; ; i++) {
    const k = `${prefix}${i}`
    if (!have.has(k)) return k
  }
}

export function persistableFlags(flags: CompatibleFlagDim[] | undefined): CompatibleFlagDim[] | undefined {
  const out = (flags ?? [])
    .map((d) => ({
      ...d,
      id: d.id?.trim() ?? '',
      label: d.label?.trim() || d.id?.trim() || '',
      wire: d.wire?.trim() || undefined,
      options: (d.options ?? []).filter((o) => o.value?.trim()),
    }))
    .filter((d) => d.id)
  return out.length ? out : undefined
}
