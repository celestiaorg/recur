import { beforeEach, describe, expect, it, vi } from 'vitest'
import { get } from 'svelte/store'
import type { Project } from '../../bindings/recur/internal/domain/models'

const update = vi.fn((p: Project) => Promise.resolve(p))
vi.mock('../../bindings/recur/internal/app/projectsservice', async (orig) => ({
  ...(await orig<object>()),
  UpdateProjectRunConfig: (p: Project) => update(p),
}))

const { projects } = await import('./stores')
const { linkLinearProject } = await import('./api')

const linear = (id: string) => get(projects).find((p) => p.id === id)?.linearProjects

beforeEach(() => {
  update.mockClear()
  projects.set([
    { id: 'a', linearProjects: 'Mobile App, eng' },
    { id: 'b', linearProjects: 'OPS' },
    { id: 'c' },
  ] as Project[])
})

describe('linkLinearProject', () => {
  it('adds the key to the target project', async () => {
    await linkLinearProject('c', 'WEB')
    expect(linear('c')).toBe('WEB')
    expect(update).toHaveBeenCalledTimes(1)
  })

  it('moves a key off every other project, case-insensitively', async () => {
    await linkLinearProject('b', 'ENG')
    expect(linear('a')).toBe('Mobile App')
    expect(linear('b')).toBe('OPS, ENG')
    expect(linear('c')).toBeUndefined()
  })

  it('is a no-op when the target already owns the key', async () => {
    await linkLinearProject('b', 'ops')
    expect(update).not.toHaveBeenCalled()
  })
})
