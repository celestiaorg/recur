import { expect, it, vi } from 'vitest'
import { get } from 'svelte/store'

let groups = [{ provider: 'claude-code', label: 'Claude Code', models: [{ ref: 'claude-code/a', label: 'A' }] }]
const list = vi.fn(() => Promise.resolve(groups))
vi.mock('../../bindings/recur/internal/app', () => ({
  SettingsService: { ListModelCatalog: () => list() },
}))

const { loadModelCatalog, invalidateModelCatalog, modelCatalogRev } = await import('./models')

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
