import { expect, it, vi } from 'vitest'
import { get } from 'svelte/store'

let groups = [{ provider: 'claude-code', label: 'Claude Code', models: [{ ref: 'claude-code/a', label: 'A' }] }]
const list = vi.fn(() => Promise.resolve(groups))
vi.mock('../../bindings/recur/internal/app', () => ({
  SettingsService: { ListModelCatalog: () => list() },
}))

const { loadModelCatalog, invalidateModelCatalog, modelCatalogRev, flattenModelCatalog } = await import('./models')

it('offers one OpenAI section while preserving the selected model runtime', () => {
  const models = flattenModelCatalog([
    { provider: 'claude-code', label: 'Claude Code', models: [{ ref: 'claude-code/a', label: 'A' }] },
    { provider: 'codex', label: 'Codex', models: [{ ref: 'codex/shared', label: 'Shared' }] },
    { provider: 'local', label: 'Local', models: [{ ref: 'local/q', label: 'Q' }] },
    { provider: 'chatgpt', label: 'ChatGPT', models: [{ ref: 'chatgpt/shared', label: 'Shared' }] },
    { provider: 'openai', label: 'OpenAI', models: [{ ref: 'openai/api', label: 'API model' }] },
  ])
  expect(models.map((m) => m.group)).toEqual(['Claude Code', 'OpenAI', 'OpenAI', 'OpenAI', 'Local'])
  expect(models.slice(1, 4).map((m) => [m.ref, m.provider, m.label])).toEqual([
    ['codex/shared', 'codex', 'Shared (Codex)'],
    ['chatgpt/shared', 'chatgpt', 'Shared (ChatGPT)'],
    ['openai/api', 'openai', 'API model (API)'],
  ])
})

it('keeps model labels short when only one OpenAI runtime is available', () => {
  expect(flattenModelCatalog([{ provider: 'chatgpt', label: 'ChatGPT', models: [{ ref: 'chatgpt/gpt', label: 'GPT' }] }])[0])
    .toMatchObject({ ref: 'chatgpt/gpt', group: 'OpenAI', label: 'GPT', provider: 'chatgpt' })
})

it('invalidate bumps the revision and the next load sees a new provider', async () => {
  expect((await loadModelCatalog()).map((m) => m.ref)).toEqual(['claude-code/a'])
  const rev = get(modelCatalogRev)
  groups = [...groups, { provider: 'local-ollama', label: 'Ollama', models: [{ ref: 'local-ollama/q', label: 'Q' }] }]
  invalidateModelCatalog()
  expect(get(modelCatalogRev)).toBe(rev + 1)
  expect((await loadModelCatalog()).map((m) => m.ref)).toEqual(['claude-code/a', 'local-ollama/q'])
  expect(list).toHaveBeenCalledTimes(2)
})

it('a superseded request does not overwrite the fresh cache', async () => {
  const resolvers: ((g: typeof groups) => void)[] = []
  list.mockImplementation(() => new Promise((r) => resolvers.push(r)))
  invalidateModelCatalog()
  const a = loadModelCatalog()
  invalidateModelCatalog()
  const b = loadModelCatalog()
  const fresh = [{ provider: 'claude-code', label: 'Claude Code', models: [{ ref: 'claude-code/b', label: 'B' }] }]
  resolvers[1](fresh)
  await b
  resolvers[0](groups)
  await a
  expect((await loadModelCatalog()).map((m) => m.ref)).toEqual(['claude-code/b'])
})
