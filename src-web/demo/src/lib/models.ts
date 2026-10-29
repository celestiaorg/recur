// The model list is SERVER-owned: service.ListModelCatalog() returns one
// grouped list across every enabled, configured provider, including per-model
// flag dimensions. See internal/app/model_catalog.go.

import { writable } from 'svelte/store'
import { service } from './api'

export type FlagOption = { value: string; label: string; default?: boolean }
export type FlagDimension = { id: string; label: string; options: FlagOption[] }

/** One selectable model from ListModelCatalog, flattened for the UI. */
export type CatalogModel = {
  ref: string
  label: string
  group: string
  provider: string
  flags: FlagDimension[]
}

/** Wails maps land as `{ [k]?: string }` — coerce to a plain string map. */
export type FlagMap = { [k: string]: string | undefined } | Record<string, string> | null | undefined

export function coerceFlags(cur: FlagMap): Record<string, string> | undefined {
  if (!cur) return undefined
  const out: Record<string, string> = {}
  for (const [k, v] of Object.entries(cur)) {
    if (typeof v === 'string') out[k] = v
  }
  return out
}

/** Fill missing dims from catalog defaults; drop dims the model no longer has. */
export function normalizeFlags(
  dims: FlagDimension[] | undefined,
  cur: FlagMap,
): Record<string, string> {
  const src = coerceFlags(cur) ?? {}
  const out: Record<string, string> = {}
  for (const d of dims ?? []) {
    const allowed = new Set(d.options.map((o) => o.value))
    if (src[d.id] && allowed.has(src[d.id])) {
      out[d.id] = src[d.id]
      continue
    }
    const def = d.options.find((o) => o.default) ?? d.options[0]
    if (def) out[d.id] = def.value
  }
  return out
}

/** Flatten ListModelCatalog groups into picker rows. */
type CatalogGroup = {
  provider: string
  label: string
  models?: { ref: string; label: string; flags?: { id: string; label: string; options?: { value: string; label: string; default?: boolean }[] }[] }[]
}

export function flattenModelCatalog(groups: CatalogGroup[] | null | undefined): CatalogModel[] {
  if (!groups?.length) return []
  const models = groups.flatMap((g) =>
    (g.models ?? []).map((m) => ({
      ref: m.ref,
      label: m.label,
      group: g.provider === 'chatgpt' || g.provider === 'codex' ? 'OpenAI' : g.label,
      provider: g.provider,
      flags: (m.flags ?? []).map((f) => ({
        id: f.id,
        label: f.label,
        options: (f.options ?? []).map((o) => ({
          value: o.value,
          label: o.label,
          default: !!o.default,
        })),
      })),
    })),
  )
  // Keep the brand in one picker section while refs still identify the runner.
  const first = models.findIndex((m) => m.group === 'OpenAI')
  if (first < 0) return models
  const openAI = models.filter((m) => m.group === 'OpenAI')
  const multipleRuntimes = new Set(openAI.map((m) => m.provider)).size > 1
  return [
    ...models.slice(0, first),
    ...openAI.map((m) => ({ ...m, label: multipleRuntimes ? `${m.label} (${m.provider === 'chatgpt' ? 'ChatGPT' : m.provider === 'codex' ? 'Codex' : 'API'})` : m.label })),
    ...models.slice(first).filter((m) => m.group !== 'OpenAI'),
  ]
}

let catalogCache: CatalogModel[] | null = null
let catalogInflight: Promise<CatalogModel[]> | null = null

/** Sync snapshot of the last successful catalogue fetch (may be empty). */
export function peekModelCatalog(): CatalogModel[] {
  return catalogCache ?? []
}

/** Bumped by invalidateModelCatalog so open pickers re-fetch. */
export const modelCatalogRev = writable(0)

/** Drop the cache so the next loadModelCatalog() hits the server (e.g. after a
 *  Cursor Connect (minted key) adds/removes the Cursor group). */
export function invalidateModelCatalog() {
  catalogCache = null
  catalogInflight = null
  modelCatalogRev.update((n) => n + 1)
}

/** Fetch (or reuse) the model catalogue. Concurrent callers share one request. */
export async function loadModelCatalog(opts?: { force?: boolean }): Promise<CatalogModel[]> {
  if (opts?.force) {
    catalogCache = null
    catalogInflight = null
  } else {
    if (catalogCache) return catalogCache
    if (catalogInflight) return catalogInflight
  }
  const req: Promise<CatalogModel[]> = Promise.resolve(service.ListModelCatalog()).then((groups) => {
    const models = flattenModelCatalog(groups)
    if (catalogInflight === req) catalogCache = models
    return models
  })
  catalogInflight = req
  try {
    return await req
  } finally {
    if (catalogInflight === req) catalogInflight = null
  }
}

export type ModelSelectOption = {
  value: string
  label: string
  group?: string
  provider?: string
}

export function catalogSelectOptions(models: CatalogModel[]): ModelSelectOption[] {
  return models.map((m) => ({ value: m.ref, label: m.label, group: m.group, provider: m.provider }))
}

/** Short label for a qualified ref when the catalogue row is not available yet. */
export function shortModelLabel(ref: string): string {
  if (!ref) return ''
  const i = ref.indexOf('/')
  const id = i >= 0 ? ref.slice(i + 1) : ref
  const provider = i >= 0 ? ref.slice(0, i) : ''
  if (provider === 'codex' || provider === 'openai' || provider === 'chatgpt') {
    return friendlyCodexSlug(id)
  }
  return id
}

/** Match server-side friendlyCodexModelLabel for slug-only fallbacks. */
function friendlyCodexSlug(id: string): string {
  let s = id.trim()
  if (!s) return ''
  if (s.length >= 4 && s.slice(0, 4).toLowerCase() === 'gpt-') {
    s = 'GPT-' + s.slice(4)
  }
  const dash = s.lastIndexOf('-')
  if (dash > 0) {
    const tail = s.slice(dash + 1)
    if (tail && !/[0-9.]/.test(tail)) {
      return `${s.slice(0, dash)} ${tail.charAt(0).toUpperCase()}${tail.slice(1)}`
    }
  }
  return s
}

/** Keep a saved/current value selectable even when the catalogue has not loaded
 *  yet (or no longer offers that id). */
export function modelOptionsFor(models: CatalogModel[], current: string): ModelSelectOption[] {
  const opts = catalogSelectOptions(models)
  if (!current || opts.some((o) => o.value === current)) return opts
  const provider = current.includes('/') ? current.slice(0, current.indexOf('/')) : ''
  return [{ value: current, label: shortModelLabel(current), group: 'Current', provider }, ...opts]
}

export function flagsForModel(models: CatalogModel[], ref: string): FlagDimension[] {
  return models.find((m) => m.ref === ref)?.flags ?? []
}
