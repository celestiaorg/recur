/** @vitest-environment happy-dom */
import { beforeEach, describe, expect, it, vi } from 'vitest'

// In-memory stand-in for the Go EditorSessionService: applies patches with the
// same merge semantics (meta wholesale, buffer/scratch upserts, explicit
// drops, delete-when-empty) and records every call for assertions.
const backend = vi.hoisted(() => ({
  sessions: new Map<string, any>(),
  calls: [] as { method: 'get' | 'update' | 'clear'; wsId: string; patch?: any }[],
  failNextUpdate: false,
  failNextGet: false,
  failNextClear: false,
  reset() {
    this.sessions.clear()
    this.calls.length = 0
    this.failNextUpdate = false
    this.failNextGet = false
    this.failNextClear = false
  },
  updates(wsId: string) {
    return this.calls.filter((c) => c.method === 'update' && c.wsId === wsId)
  },
}))

vi.mock('../../bindings/recur/internal/app', () => ({
  EditorSessionService: {
    GetEditorSession: async (wsId: string) => {
      backend.calls.push({ method: 'get', wsId })
      if (backend.failNextGet) {
        backend.failNextGet = false
        throw new Error('mock get failure')
      }
      const sess = backend.sessions.get(wsId)
      return sess ? structuredClone(sess) : null
    },
    UpdateEditorSession: async (wsId: string, patch: any) => {
      backend.calls.push({ method: 'update', wsId, patch: structuredClone(patch) })
      if (backend.failNextUpdate) {
        backend.failNextUpdate = false
        throw new Error('mock backend failure')
      }
      const cur = backend.sessions.get(wsId) ?? { openFiles: [], selectedPath: '' }
      if (patch.meta) {
        cur.openFiles = [...(patch.meta.openFiles ?? [])]
        cur.selectedPath = patch.meta.selectedPath ?? ''
        cur.splitPath = patch.meta.splitPath
        cur.scratchSeq = patch.meta.scratchSeq
        cur.fileViews = patch.meta.fileViews ? structuredClone(patch.meta.fileViews) : undefined
        if ('syntaxByPath' in patch.meta) {
          const m = patch.meta.syntaxByPath
          cur.syntaxByPath = m && Object.keys(m).length ? structuredClone(m) : undefined
        }
        if ('syntaxByFileType' in patch.meta) {
          const m = patch.meta.syntaxByFileType
          cur.syntaxByFileType = m && Object.keys(m).length ? structuredClone(m) : undefined
        }
        cur.mainSurface = patch.meta.mainSurface === 'review' ? 'review' : undefined
        cur.leftKind = patch.meta.leftKind || undefined
        cur.rightKind = patch.meta.rightKind || undefined
        cur.leftOpen = typeof patch.meta.leftOpen === 'boolean' ? patch.meta.leftOpen : undefined
        cur.rightOpen = typeof patch.meta.rightOpen === 'boolean' ? patch.meta.rightOpen : undefined
      }
      for (const [p, b] of Object.entries(patch.buffers ?? {})) (cur.buffers ??= {})[p] = b
      for (const p of patch.dropBuffers ?? []) delete cur.buffers?.[p]
      for (const [p, d] of Object.entries(patch.scratchDocs ?? {})) (cur.scratchDocs ??= {})[p] = d
      for (const p of patch.dropScratchDocs ?? []) delete cur.scratchDocs?.[p]
      const empty = !cur.openFiles?.length
        && !Object.keys(cur.buffers ?? {}).length
        && !Object.keys(cur.scratchDocs ?? {}).length
        && !Object.keys(cur.syntaxByPath ?? {}).length
        && !Object.keys(cur.syntaxByFileType ?? {}).length
        && cur.mainSurface !== 'review'
        && (!cur.leftKind || cur.leftKind === 'maestro')
        && (!cur.rightKind || cur.rightKind === 'files')
        && cur.leftOpen !== false
        && cur.rightOpen !== false
      if (empty) backend.sessions.delete(wsId)
      else backend.sessions.set(wsId, cur)
    },
    ClearEditorSession: async (wsId: string) => {
      backend.calls.push({ method: 'clear', wsId })
      if (backend.failNextClear) {
        backend.failNextClear = false
        throw new Error('mock clear failure')
      }
      backend.sessions.delete(wsId)
    },
  },
}))

type Mod = typeof import('./editor-session')

async function loadMod(): Promise<Mod> {
  return await import('./editor-session')
}

beforeEach(() => {
  vi.resetModules() // fresh sent-state / queues / migration flag per test
  backend.reset()
  localStorage.clear()
})

describe('editorContentHash', () => {
  it('is sha256 over LF-normalized utf-8 text', async () => {
    const mod = await loadMod()
    expect(await mod.editorContentHash('x'))
      .toBe('2d711642b726b04401627ca9fbac32f5c8530fb1903cc4db02258717921a4881')
    // CRLF on disk hashes the same as the LF buffer it seeds.
    expect(await mod.editorContentHash('a\r\nb\r\n')).toBe(await mod.editorContentHash('a\nb\n'))
  })
})

describe('pruneStashedBuffers', () => {
  const dirty = { draft: 'edited\n', saved: 'original\n' }

  it('keeps dirty buffers for still-open paths', async () => {
    const mod = await loadMod()
    const out = mod.pruneStashedBuffers(
      { 'src/a.go': dirty, 'src/b.go': dirty },
      ['src/a.go'],
    )
    expect(out).toEqual({ 'src/a.go': dirty })
  })

  it('drops scratch, clean, baseless, and oversized buffers', async () => {
    const mod = await loadMod()
    const huge = 'x'.repeat(mod.MAX_UNSAVED_CHARS + 1)
    const out = mod.pruneStashedBuffers(
      {
        'scratch://1-note.txt': dirty,
        'src/clean.go': { draft: 'same\n', saved: 'same\n' },
        'src/crlf.go': { draft: 'a\nb\n', saved: 'a\r\nb\r\n' },
        'src/huge.go': { draft: huge, saved: 'ok\n' },
        'src/no-base.go': { draft: 'orphan\n' },
        'src/keep.go': dirty,
        'src/restored.go': { draft: 'restored\n', baseHash: 'abc' },
      },
      [
        'scratch://1-note.txt', 'src/clean.go', 'src/crlf.go', 'src/huge.go',
        'src/no-base.go', 'src/keep.go', 'src/restored.go',
      ],
    )
    expect(out).toEqual({
      'src/keep.go': dirty,
      'src/restored.go': { draft: 'restored\n', baseHash: 'abc' },
    })
  })
})

describe('uniqueOpenFiles / remapOpenFiles', () => {
  it('drops duplicate and empty paths, first wins', async () => {
    const mod = await loadMod()
    expect(mod.uniqueOpenFiles(['a.jpg', 'b.jpg', 'a.jpg', '', 'c.jpg'])).toEqual([
      'a.jpg', 'b.jpg', 'c.jpg',
    ])
  })

  it('rename onto an already-open path keeps the remapped tab, not the stale one', async () => {
    const mod = await loadMod()
    const path = 'public/images/collections/wanderer.jpg'
    expect(mod.remapOpenFiles(
      [path, 'src/main.go', 'public/images/collections/wanderer-old.jpg'],
      'public/images/collections/wanderer-old.jpg',
      path,
    )).toEqual(['src/main.go', path])
    expect(mod.remapOpenFiles(
      ['public/images/collections/wanderer-old.jpg', path],
      'public/images/collections/wanderer-old.jpg',
      path,
    )).toEqual([path])
  })

  it('rewrites nested paths under a renamed folder', async () => {
    const mod = await loadMod()
    expect(mod.remapOpenFiles(
      ['dir/a.ts', 'dir/b.ts', 'other.ts'],
      'dir',
      'renamed',
    )).toEqual(['renamed/a.ts', 'renamed/b.ts', 'other.ts'])
  })
})

describe('editor session persistence (RPC-backed)', () => {
  const session = (over: Partial<any> = {}) => ({
    openFiles: ['src/main.go', 'src/other.go'],
    selectedPath: 'src/main.go',
    stashedBuffers: {
      'src/main.go': { draft: 'KEEPME\n', saved: 'package main\n' },
    },
    syntaxMapsKnown: true,
    ...over,
  })

  it('round-trips a session: buffers travel as draft + baseHash', async () => {
    const mod = await loadMod()
    mod.saveEditorSession('proj', 'ws', session())
    mod.flushEditorSessionNow()

    const sess = await mod.loadEditorSession('proj', 'ws')
    expect(sess?.openFiles).toEqual(['src/main.go', 'src/other.go'])
    expect(sess?.stashedBuffers?.['src/main.go']).toEqual({
      draft: 'KEEPME\n',
      baseHash: await mod.editorContentHash('package main\n'),
    })
    // The stored copy holds no full saved text — only its hash.
    const stored = backend.sessions.get('ws')
    expect(stored.buffers['src/main.go'].saved).toBeUndefined()
  })

  it('collapses duplicate openFiles from a stored session', async () => {
    const mod = await loadMod()
    const path = 'public/images/collections/wanderer.jpg'
    backend.sessions.set('ws', {
      openFiles: [path, 'src/main.go', path],
      selectedPath: path,
    })
    const sess = await mod.loadEditorSession('proj', 'ws')
    expect(sess?.openFiles).toEqual([path, 'src/main.go'])
  })

  it('round-trips per-file caret, scroll, and viewMode', async () => {
    const mod = await loadMod()
    const views = {
      'src/main.go': {
        viewMode: 'preview' as const, line: 12, col: 4, topLine: 10, scrollTop: 80, scrollLeft: 0, previewScrollTop: 240,
      },
      'src/other.go': {
        viewMode: 'diff' as const, line: 3, col: 1, scrollTop: 0, scrollLeft: 0,
      },
    }
    mod.saveEditorSession('proj', 'ws', session({ fileViews: views }))
    mod.flushEditorSessionNow()

    const sess = await mod.loadEditorSession('proj', 'ws')
    expect(sess?.fileViews).toEqual(views)
    expect(backend.sessions.get('ws').fileViews['src/main.go'].viewMode).toBe('preview')
  })

  it('round-trips language-mode maps and keeps a syntax-only session', async () => {
    const mod = await loadMod()
    const syntaxByPath = { 'src/main.go': '', 'Dockerfile': 'source.dockerfile' }
    const syntaxByFileType = { '*.conf': 'source.yaml' }
    mod.saveEditorSession('proj', 'ws', session({ syntaxByPath, syntaxByFileType }))
    mod.flushEditorSessionNow()

    const sess = await mod.loadEditorSession('proj', 'ws')
    expect(sess?.syntaxByPath).toEqual(syntaxByPath)
    expect(sess?.syntaxByFileType).toEqual(syntaxByFileType)

    // Closing every tab still keeps the maps (not pruned to open files).
    mod.saveEditorSession('proj', 'ws', {
      openFiles: [],
      selectedPath: '',
      syntaxByPath,
      syntaxByFileType,
    })
    mod.flushEditorSessionNow()
    const onlySyntax = await mod.loadEditorSession('proj', 'ws')
    expect(onlySyntax?.openFiles).toEqual([])
    expect(onlySyntax?.syntaxByPath).toEqual(syntaxByPath)
    expect(onlySyntax?.syntaxByFileType).toEqual(syntaxByFileType)
    expect(backend.sessions.has('ws')).toBe(true)
  })

  it('round-trips observatory Review column and keeps a review-only session', async () => {
    const mod = await loadMod()
    mod.saveEditorSession('proj', 'ws', {
      openFiles: [],
      selectedPath: '',
      mainSurface: 'review',
      syntaxMapsKnown: true,
    })
    mod.flushEditorSessionNow()
    const sess = await mod.loadEditorSession('proj', 'ws')
    expect(sess?.mainSurface).toBe('review')
    expect(sess?.openFiles).toEqual([])
    expect(backend.sessions.get('ws').mainSurface).toBe('review')

    mod.saveEditorSession('proj', 'ws', {
      openFiles: [],
      selectedPath: '',
      syntaxMapsKnown: true,
    })
    mod.flushEditorSessionNow()
    expect(await mod.loadEditorSession('proj', 'ws')).toBeNull()
    expect(backend.sessions.has('ws')).toBe(false)
  })

  it('round-trips pane chrome and keeps a pane-only session', async () => {
    const mod = await loadMod()
    mod.saveEditorSession('proj', 'ws', {
      openFiles: [],
      selectedPath: '',
      leftKind: 'findings',
      rightKind: 'scm',
      leftOpen: false,
      rightOpen: false,
      syntaxMapsKnown: true,
    })
    mod.flushEditorSessionNow()
    const sess = await mod.loadEditorSession('proj', 'ws')
    expect(sess?.leftKind).toBe('findings')
    expect(sess?.rightKind).toBe('scm')
    expect(sess?.leftOpen).toBe(false)
    expect(sess?.rightOpen).toBe(false)
    expect(sess?.openFiles).toEqual([])
    expect(backend.sessions.get('ws').leftKind).toBe('findings')
    expect(backend.sessions.get('ws').rightOpen).toBe(false)

    mod.saveEditorSession('proj', 'ws', {
      openFiles: [],
      selectedPath: '',
      syntaxMapsKnown: true,
    })
    mod.flushEditorSessionNow()
    expect(await mod.loadEditorSession('proj', 'ws')).toBeNull()
    expect(backend.sessions.has('ws')).toBe(false)
  })

  it('omits syntax maps until known so a tab write cannot wipe them', async () => {
    const mod = await loadMod()
    const syntaxByPath = { 'src/main.go': 'source.yaml' }
    mod.saveEditorSession('proj', 'ws', session({ syntaxByPath }))
    mod.flushEditorSessionNow()
    await mod.loadEditorSession('proj', 'ws')

    mod.saveEditorSession('proj', 'ws', session({
      selectedPath: 'src/other.go',
      syntaxMapsKnown: false,
      syntaxByPath: {},
      syntaxByFileType: {},
    }))
    mod.flushEditorSessionNow()
    await mod.loadEditorSession('proj', 'ws')
    const last = backend.updates('ws').at(-1)!.patch
    expect(last.meta.syntaxByPath).toBeUndefined()
    expect(last.meta.syntaxByFileType).toBeUndefined()
    expect(backend.sessions.get('ws').syntaxByPath).toEqual(syntaxByPath)
  })

  it('drops scratch:// language overrides from the persisted session', async () => {
    const mod = await loadMod()
    mod.saveEditorSession('proj', 'ws', session({
      syntaxByPath: {
        'src/main.go': 'source.yaml',
        'scratch://1-Scratch': 'source.yaml',
      },
    }))
    mod.flushEditorSessionNow()
    const sess = await mod.loadEditorSession('proj', 'ws')
    expect(sess?.syntaxByPath).toEqual({ 'src/main.go': 'source.yaml' })
    expect(backend.sessions.get('ws').syntaxByPath['scratch://1-Scratch']).toBeUndefined()
  })

  it('change-gates: an unchanged session sends no patch', async () => {
    const mod = await loadMod()
    mod.saveEditorSession('proj', 'ws', session())
    mod.flushEditorSessionNow()
    await mod.loadEditorSession('proj', 'ws') // drains the queue
    const before = backend.updates('ws').length
    expect(before).toBe(1)

    mod.saveEditorSession('proj', 'ws', session())
    mod.flushEditorSessionNow()
    await mod.loadEditorSession('proj', 'ws')
    expect(backend.updates('ws').length).toBe(before)
  })

  it('ships only the changed buffer, and drops removed ones explicitly', async () => {
    const mod = await loadMod()
    const two = session({
      stashedBuffers: {
        'src/main.go': { draft: 'A1\n', saved: 'a\n' },
        'src/other.go': { draft: 'B1\n', saved: 'b\n' },
      },
    })
    mod.saveEditorSession('proj', 'ws', two)
    mod.flushEditorSessionNow()
    await mod.loadEditorSession('proj', 'ws')

    // One buffer edited: patch carries just that buffer, no meta, no drops.
    mod.saveEditorSession('proj', 'ws', session({
      stashedBuffers: {
        'src/main.go': { draft: 'A2\n', saved: 'a\n' },
        'src/other.go': { draft: 'B1\n', saved: 'b\n' },
      },
    }))
    mod.flushEditorSessionNow()
    await mod.loadEditorSession('proj', 'ws')
    const second = backend.updates('ws')[1].patch
    expect(Object.keys(second.buffers)).toEqual(['src/main.go'])
    expect(second.meta).toBeUndefined()
    expect(second.dropBuffers).toBeUndefined()

    // A save landed on main.go: the buffer leaves the session → explicit drop.
    mod.saveEditorSession('proj', 'ws', session({
      stashedBuffers: {
        'src/other.go': { draft: 'B1\n', saved: 'b\n' },
      },
    }))
    mod.flushEditorSessionNow()
    await mod.loadEditorSession('proj', 'ws')
    const third = backend.updates('ws')[2].patch
    expect(third.dropBuffers).toEqual(['src/main.go'])
    expect(third.buffers).toBeUndefined()
    expect(backend.sessions.get('ws').buffers['src/main.go']).toBeUndefined()
    expect(backend.sessions.get('ws').buffers['src/other.go'].draft).toBe('B1\n')
  })

  it('retries the same diff after a failed patch', async () => {
    const mod = await loadMod()
    backend.failNextUpdate = true
    mod.saveEditorSession('proj', 'ws', session())
    mod.flushEditorSessionNow()
    // Steal the quit journal so this test pins the sent-state retry path,
    // not journal recovery (covered separately below).
    localStorage.removeItem('recur.editorSession.quit:ws')
    await mod.loadEditorSession('proj', 'ws')
    expect(backend.sessions.get('ws')).toBeUndefined()

    mod.saveEditorSession('proj', 'ws', session())
    mod.flushEditorSessionNow()
    const sess = await mod.loadEditorSession('proj', 'ws')
    expect(sess?.stashedBuffers?.['src/main.go']?.draft).toBe('KEEPME\n')
  })

  it('flushEditorSessionNow writes a sync quit journal recovered on the next load', async () => {
    const mod = await loadMod()
    backend.failNextUpdate = true
    mod.saveEditorSession('proj', 'ws', session())
    mod.flushEditorSessionNow()
    // Journal is written in the same turn — before the async IPC.
    expect(localStorage.getItem('recur.editorSession.quit:ws')).toBeTruthy()

    const sess = await mod.loadEditorSession('proj', 'ws')
    expect(sess?.stashedBuffers?.['src/main.go']?.draft).toBe('KEEPME\n')
    expect(localStorage.getItem('recur.editorSession.quit:ws')).toBeNull()
    expect(backend.sessions.get('ws')?.buffers['src/main.go']?.draft).toBe('KEEPME\n')
  })

  it('a successful flush drops the quit journal', async () => {
    const mod = await loadMod()
    mod.saveEditorSession('proj', 'ws', session())
    mod.flushEditorSessionNow()
    expect(localStorage.getItem('recur.editorSession.quit:ws')).toBeTruthy()
    await mod.loadEditorSession('proj', 'ws')
    expect(localStorage.getItem('recur.editorSession.quit:ws')).toBeNull()
  })

  it('an unsettled quit journal fills syntax maps from the stored session', async () => {
    const mod = await loadMod()
    const syntaxByPath = { 'src/main.go': 'source.yaml' }
    const syntaxByFileType = { '*.conf': 'source.yaml' }
    mod.saveEditorSession('proj', 'ws', session({ syntaxByPath, syntaxByFileType }))
    mod.flushEditorSessionNow()
    await mod.loadEditorSession('proj', 'ws')

    localStorage.setItem('recur.editorSession.quit:ws', JSON.stringify(session({
      selectedPath: 'src/other.go',
      syntaxMapsKnown: false,
      syntaxByPath: {},
      syntaxByFileType: {},
    })))
    const sess = await mod.loadEditorSession('proj', 'ws')
    expect(sess?.selectedPath).toBe('src/other.go')
    expect(sess?.syntaxByPath).toEqual(syntaxByPath)
    expect(sess?.syntaxByFileType).toEqual(syntaxByFileType)
    expect(backend.sessions.get('ws').syntaxByPath).toEqual(syntaxByPath)
    expect(localStorage.getItem('recur.editorSession.quit:ws')).toBeNull()
  })

  it('merges unsettled journal picks over stored syntax maps', async () => {
    const mod = await loadMod()
    mod.saveEditorSession('proj', 'ws', session({
      syntaxByPath: { 'src/main.go': 'source.yaml' },
      syntaxByFileType: { '*.conf': 'source.yaml' },
    }))
    mod.flushEditorSessionNow()
    await mod.loadEditorSession('proj', 'ws')

    localStorage.setItem('recur.editorSession.quit:ws', JSON.stringify(session({
      syntaxMapsKnown: false,
      syntaxByPath: { 'README.md': 'text.html.markdown' },
      syntaxByFileType: {},
    })))
    const sess = await mod.loadEditorSession('proj', 'ws')
    expect(sess?.syntaxByPath).toEqual({
      'src/main.go': 'source.yaml',
      'README.md': 'text.html.markdown',
    })
    expect(sess?.syntaxByFileType).toEqual({ '*.conf': 'source.yaml' })
  })

  it('restores an unsettled journal even when GetEditorSession fails', async () => {
    const mod = await loadMod()
    mod.saveEditorSession('proj', 'ws', session({
      syntaxByPath: { 'src/main.go': 'source.yaml' },
    }))
    mod.flushEditorSessionNow()
    await mod.loadEditorSession('proj', 'ws')

    localStorage.setItem('recur.editorSession.quit:ws', JSON.stringify(session({
      selectedPath: 'src/other.go',
      syntaxMapsKnown: false,
      syntaxByPath: { 'README.md': 'text.html.markdown' },
      stashedBuffers: {
        'src/other.go': { draft: 'DRAFT\n', saved: 'orig\n' },
      },
    })))
    backend.failNextGet = true
    const sess = await mod.loadEditorSession('proj', 'ws')
    expect(sess?.selectedPath).toBe('src/other.go')
    expect(sess?.stashedBuffers?.['src/other.go']?.draft).toBe('DRAFT\n')
    expect(sess?.syntaxByPath).toEqual({ 'README.md': 'text.html.markdown' })
    expect(backend.sessions.get('ws').syntaxByPath).toEqual({ 'src/main.go': 'source.yaml' })
    expect(localStorage.getItem('recur.editorSession.quit:ws')).toBeNull()
  })

  it('keeps syntaxMapsKnown false on a syntax-only journal when Get fails', async () => {
    const mod = await loadMod()
    mod.saveEditorSession('proj', 'ws', session({
      syntaxByPath: { 'src/main.go': 'source.yaml' },
    }))
    mod.flushEditorSessionNow()
    await mod.loadEditorSession('proj', 'ws')

    localStorage.setItem('recur.editorSession.quit:ws', JSON.stringify(session({
      openFiles: [],
      selectedPath: '',
      stashedBuffers: {},
      syntaxMapsKnown: false,
      syntaxByPath: { 'README.md': 'text.html.markdown' },
    })))
    backend.failNextGet = true
    const sess = await mod.loadEditorSession('proj', 'ws')
    expect(sess?.openFiles).toEqual([])
    expect(sess?.syntaxByPath).toEqual({ 'README.md': 'text.html.markdown' })
    expect(sess?.syntaxMapsKnown).toBe(false)
    expect(backend.sessions.get('ws').syntaxByPath).toEqual({ 'src/main.go': 'source.yaml' })
  })

  it('clearEditorSession drops the stored session and cancels a pending save', async () => {
    const mod = await loadMod()
    mod.saveEditorSession('proj', 'ws', session())
    mod.flushEditorSessionNow()
    await mod.loadEditorSession('proj', 'ws')
    expect(backend.sessions.has('ws')).toBe(true)

    mod.saveEditorSession('proj', 'ws', session({ selectedPath: 'src/other.go' }))
    mod.clearEditorSession('proj', 'ws')
    mod.flushEditorSessionNow()
    expect(await mod.loadEditorSession('proj', 'ws')).toBeNull()
    expect(backend.sessions.has('ws')).toBe(false)
  })

  it('clearEditorSession does not re-RPC when the session is already empty', async () => {
    const mod = await loadMod()
    await mod.loadEditorSession('proj', 'ws')
    mod.clearEditorSession('proj', 'ws')
    await mod.loadEditorSession('proj', 'ws')
    expect(backend.calls.filter((c) => c.method === 'clear' && c.wsId === 'ws')).toHaveLength(0)

    mod.saveEditorSession('proj', 'ws', session())
    mod.flushEditorSessionNow()
    await mod.loadEditorSession('proj', 'ws')
    mod.clearEditorSession('proj', 'ws')
    await mod.loadEditorSession('proj', 'ws')
    expect(backend.calls.filter((c) => c.method === 'clear' && c.wsId === 'ws')).toHaveLength(1)

    mod.clearEditorSession('proj', 'ws')
    await mod.loadEditorSession('proj', 'ws')
    expect(backend.calls.filter((c) => c.method === 'clear' && c.wsId === 'ws')).toHaveLength(1)
  })

  it('re-saves after an in-flight write is followed by clear', async () => {
    const mod = await loadMod()
    mod.saveEditorSession('proj', 'ws', session())
    mod.flushEditorSessionNow()
    mod.clearEditorSession('proj', 'ws')
    expect(await mod.loadEditorSession('proj', 'ws')).toBeNull()

    mod.saveEditorSession('proj', 'ws', session())
    mod.flushEditorSessionNow()
    const sess = await mod.loadEditorSession('proj', 'ws')
    expect(sess?.openFiles).toEqual(['src/main.go', 'src/other.go'])
    expect(backend.sessions.has('ws')).toBe(true)
  })

  it('retries clear after a failed RPC when nothing was previously sent', async () => {
    const mod = await loadMod()
    backend.failNextClear = true
    mod.clearEditorSession('proj', 'ws')
    await vi.waitFor(() => {
      expect(backend.calls.filter((c) => c.method === 'clear' && c.wsId === 'ws')).toHaveLength(1)
    })

    mod.clearEditorSession('proj', 'ws')
    await vi.waitFor(() => {
      expect(backend.calls.filter((c) => c.method === 'clear' && c.wsId === 'ws')).toHaveLength(2)
    })
  })

  it('dropEditorSession cancels a pending save without touching the backend', async () => {
    const mod = await loadMod()
    mod.saveEditorSession('proj', 'gone', session())
    mod.dropEditorSession('gone')
    mod.flushEditorSessionNow()
    expect(await mod.loadEditorSession('proj', 'gone')).toBeNull()
    expect(backend.updates('gone').length).toBe(0)
  })

  it('scratch docs ride the same upsert/drop protocol', async () => {
    const mod = await loadMod()
    const withScratch = session({
      openFiles: ['src/main.go', 'scratch://1-notes.txt'],
      scratchDocs: {
        'scratch://1-notes.txt': { title: 'Notes', content: 'todo\n', lang: 'txt' },
      },
      scratchSeq: 1,
    })
    mod.saveEditorSession('proj', 'ws', withScratch)
    mod.flushEditorSessionNow()
    const sess = await mod.loadEditorSession('proj', 'ws')
    expect(sess?.scratchDocs?.['scratch://1-notes.txt']?.content).toBe('todo\n')
    expect(sess?.scratchSeq).toBe(1)

    // Scratch tab closed → explicit drop.
    mod.saveEditorSession('proj', 'ws', session())
    mod.flushEditorSessionNow()
    await mod.loadEditorSession('proj', 'ws')
    const last = backend.updates('ws').at(-1)!.patch
    expect(last.dropScratchDocs).toEqual(['scratch://1-notes.txt'])
    expect(backend.sessions.get('ws').scratchDocs?.['scratch://1-notes.txt']).toBeUndefined()
  })
})

describe('legacy localStorage migration', () => {
  it('imports v2 keys into the backend once and removes them', async () => {
    localStorage.setItem('recur.editorSession.v2:proj::ws-a', JSON.stringify({
      openFiles: ['a.go'],
      selectedPath: 'a.go',
      stashedBuffers: { 'a.go': { draft: 'DRAFT\n', saved: 'orig\n' } },
    }))
    localStorage.setItem('recur.editorSession.v2:proj::ws-b', JSON.stringify({
      openFiles: ['b.go'],
      selectedPath: 'b.go',
    }))

    const mod = await loadMod()
    const sess = await mod.loadEditorSession('proj', 'ws-a')
    expect(sess?.openFiles).toEqual(['a.go'])
    expect(sess?.stashedBuffers?.['a.go']?.draft).toBe('DRAFT\n')

    expect(localStorage.getItem('recur.editorSession.v2:proj::ws-a')).toBeNull()
    expect(localStorage.getItem('recur.editorSession.v2:proj::ws-b')).toBeNull()
    // Both sessions landed in the backend, buffers converted to baseHash.
    expect(backend.sessions.get('ws-a').buffers['a.go'].baseHash)
      .toBe(await mod.editorContentHash('orig\n'))
    expect(backend.sessions.get('ws-b').openFiles).toEqual(['b.go'])
    // Restart (fresh module): the backend copy is what restores now.
    vi.resetModules()
    const mod2 = await loadMod()
    const restored = await mod2.loadEditorSession('proj', 'ws-a')
    expect(restored?.stashedBuffers?.['a.go']).toEqual({
      draft: 'DRAFT\n',
      baseHash: await mod.editorContentHash('orig\n'),
    })
  })

  it('migrates the v1 blob and prefers a newer v2 key for the same workspace', async () => {
    localStorage.setItem('recur.editorSession.v1', JSON.stringify({
      'proj::ws-a': { openFiles: ['old.go'], selectedPath: 'old.go' },
      'proj::ws-b': { openFiles: ['b.go'], selectedPath: 'b.go' },
    }))
    localStorage.setItem(
      'recur.editorSession.v2:proj::ws-a',
      JSON.stringify({ openFiles: ['new.go'], selectedPath: 'new.go' }),
    )

    const mod = await loadMod()
    expect((await mod.loadEditorSession('proj', 'ws-a'))?.selectedPath).toBe('new.go')
    expect((await mod.loadEditorSession('proj', 'ws-b'))?.selectedPath).toBe('b.go')
    expect(localStorage.getItem('recur.editorSession.v1')).toBeNull()
  })
})
