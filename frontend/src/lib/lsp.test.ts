import { afterEach, describe, expect, test, vi } from 'vitest'
import { Text } from '@codemirror/state'
import type { EditorView } from '@codemirror/view'
import type { LSPClient } from '@codemirror/lsp-client'

vi.mock('@wailsio/runtime', () => ({
  Browser: { OpenURL: vi.fn() },
  Events: { On: vi.fn() },
}))

vi.mock('../../bindings/recur/internal/app/lspservice', () => ({
  CloseLSP: vi.fn(() => Promise.resolve()),
  EnsureLSP: vi.fn(() => Promise.resolve(null)),
  WriteLSP: vi.fn(() => Promise.resolve()),
}))

vi.mock('./stores', () => ({ pushToast: vi.fn() }))

import {
  RecurWorkspace,
  RecurWorkspaceFile,
  applyWorkspaceEdit,
  attachLsp,
  clearAllLsp,
  installLspSessionForTest,
  notifyDidSave,
  parseVueTsserverRequest,
  setLspFileIO,
  snapshotLspDocs,
  vueTsserverResponse,
  withVueTsPlugin,
} from './lsp'
import { CloseLSP, EnsureLSP } from '../../bindings/recur/internal/app/lspservice'

function fakeView(text: string): EditorView {
  const view = {
    state: { doc: Text.of(text.split('\n')) },
    plugin() { return undefined },
    dispatch(spec: { changes: { from: number; to: number; insert: string } }) {
      view.state = { doc: Text.of(String(spec.changes.insert).split('\n')) }
    },
  }
  return view as unknown as EditorView
}

function stubWorkspace(workspaceId = 'ws-a', rootUri = 'file:///tmp/ws') {
  const client = {
    didOpen: vi.fn(),
    didClose: vi.fn(),
    notification: vi.fn(),
    sync() {
      for (const u of ws.syncFiles()) {
        client.notification('textDocument/didChange', {
          textDocument: { uri: u.file.uri, version: u.file.version },
          contentChanges: [{ text: u.file.doc.toString() }],
        })
      }
    },
  } as unknown as LSPClient
  const ws = new RecurWorkspace(client, rootUri, workspaceId)
  ;(client as unknown as { workspace: RecurWorkspace }).workspace = ws
  return { client, ws, rootUri }
}

afterEach(() => {
  clearAllLsp()
  setLspFileIO('ws-a', null)
  vi.mocked(CloseLSP).mockClear()
  vi.mocked(EnsureLSP).mockReset()
  vi.mocked(EnsureLSP).mockResolvedValue(null as never)
})

describe('RecurWorkspace', () => {
  test('reattach keeps the last synced snapshot so pending edits still flush', () => {
    const { ws } = stubWorkspace()
    const uri = 'file:///tmp/ws/a.go'
    const view1 = fakeView('old')
    ws.openFile(uri, 'go', view1)
    const file = ws.getFile(uri) as RecurWorkspaceFile
    ;(view1 as unknown as { state: { doc: Text } }).state.doc = Text.of(['pending'])
    file.detachView()
    const view2 = fakeView('pending')
    file.attachView(view2)
    expect(file.doc.toString()).toBe('old')
    const updates = ws.syncFiles()
    expect(updates).toHaveLength(1)
    expect(file.doc.toString()).toBe('pending')
  })

  test('closeFile flushes the live buffer before detaching', () => {
    const { ws } = stubWorkspace()
    const uri = 'file:///tmp/ws/a.go'
    const view = fakeView('old')
    ws.openFile(uri, 'go', view)
    const file = ws.getFile(uri) as RecurWorkspaceFile
    ;(view as unknown as { state: { doc: Text } }).state.doc = Text.of(['new'])
    ws.closeFile(uri, view)
    expect(file.getView()).toBeNull()
    expect(file.doc.toString()).toBe('new')
  })
})

describe('applyWorkspaceEdit', () => {
  test('aborts if an already-planned editor changes while another file is loading', async () => {
    const { client, ws } = stubWorkspace()
    const viewA = fakeView('aaa')
    ws.openFile('file:///tmp/ws/a.go', 'go', viewA)
    let release!: () => void
    let waiting = false
    const gate = new Promise<void>((resolve) => {
      release = resolve
    })
    setLspFileIO('ws-a', {
      unsaved: () => null,
      read: async (rel) => {
        if (rel === 'b.go') {
          waiting = true
          await gate
        }
        return 'bbb'
      },
      write: async () => {},
      applyIfOpen: () => false,
    })
    const pending = applyWorkspaceEdit(client, {
      changes: {
        'file:///tmp/ws/a.go': [{
          range: { start: { line: 0, character: 0 }, end: { line: 0, character: 3 } },
          newText: 'AAA',
        }],
        'file:///tmp/ws/b.go': [{
          range: { start: { line: 0, character: 0 }, end: { line: 0, character: 3 } },
          newText: 'BBB',
        }],
      },
    })
    const deadline = Date.now() + 1000
    while (!waiting && Date.now() < deadline) await Promise.resolve()
    expect(waiting).toBe(true)
    ;(viewA as unknown as { state: { doc: Text } }).state.doc = Text.of(['typed'])
    release()
    expect(await pending).toBe(false)
    expect(viewA.state.doc.toString()).toBe('typed')
  })

  test('F2 snapshot rejects a rename if the editor changed during the request', async () => {
    const { client, ws } = stubWorkspace()
    const viewA = fakeView('aaa')
    ws.openFile('file:///tmp/ws/a.go', 'go', viewA)
    const bases = snapshotLspDocs(client)
    ;(viewA as unknown as { state: { doc: Text } }).state.doc = Text.of(['typed'])
    const ok = await applyWorkspaceEdit(client, {
      changes: {
        'file:///tmp/ws/a.go': [{
          range: { start: { line: 0, character: 0 }, end: { line: 0, character: 3 } },
          newText: 'AAA',
        }],
      },
    }, bases)
    expect(ok).toBe(false)
    expect(viewA.state.doc.toString()).toBe('typed')
  })

  test('consecutive edits without a view keep the server snapshot', async () => {
    const { client, ws } = stubWorkspace()
    const uri = 'file:///tmp/ws/a.go'
    const disk = new Map([['a.go', 'foo baz']])
    setLspFileIO('ws-a', {
      unsaved: () => null,
      read: async (rel) => disk.get(rel) ?? null,
      write: async (rel, text) => { disk.set(rel, text) },
      applyIfOpen: () => false,
    })
    const range = (start: number, end: number, newText: string) => ({
      range: { start: { line: 0, character: start }, end: { line: 0, character: end } },
      newText,
    })
    expect(await applyWorkspaceEdit(client, { changes: { [uri]: [range(0, 3, 'new')] } })).toBe(true)
    const file = ws.getFile(uri) as RecurWorkspaceFile
    expect(file.doc.toString()).toBe('new baz')
    expect(disk.get('a.go')).toBe('new baz')
    expect(client.didOpen).toHaveBeenCalledTimes(1)
    expect(client.notification).toHaveBeenCalledWith(
      'textDocument/didChange',
      expect.objectContaining({
        textDocument: expect.objectContaining({ uri, version: file.version }),
        contentChanges: [{ text: 'new baz' }],
      }),
    )
    expect(await applyWorkspaceEdit(client, { changes: { [uri]: [range(4, 7, 'qux')] } })).toBe(true)
    expect(file.doc.toString()).toBe('new qux')
    expect(disk.get('a.go')).toBe('new qux')
    expect(client.didOpen).toHaveBeenCalledTimes(1)
    expect(client.notification).toHaveBeenLastCalledWith(
      'textDocument/didChange',
      expect.objectContaining({
        contentChanges: [{ text: 'new qux' }],
      }),
    )
  })
})

function deferred<T>() {
  let resolve!: (value: T) => void
  const promise = new Promise<T>((r) => { resolve = r })
  return { promise, resolve }
}

function ensureRes(id: string, extra: {
  serverKey?: string
  rootUri?: string
  rootPath?: string
  fileUri?: string
  languageId?: string
} = {}) {
  return {
    available: true,
    id,
    serverKey: extra.serverKey ?? 'gopls',
    rootUri: extra.rootUri ?? 'file:///tmp/ws',
    rootPath: extra.rootPath ?? '/tmp/ws',
    fileUri: extra.fileUri ?? 'file:///tmp/ws/a.go',
    languageId: extra.languageId ?? 'go',
  }
}

describe('attachLsp', () => {
  test('stale Ensure closes the session when nothing else is pending', async () => {
    vi.mocked(EnsureLSP).mockResolvedValue(ensureRes('gopls-1') as never)
    expect(await attachLsp('p', 'ws-a', 'a.go', () => false)).toBeNull()
    expect(CloseLSP).toHaveBeenCalledWith('gopls-1')
  })

  test('stale Ensure does not CloseLSP while another Ensure for the same language is pending', async () => {
    const a = deferred<ReturnType<typeof ensureRes>>()
    const b = deferred<ReturnType<typeof ensureRes>>()
    vi.mocked(EnsureLSP)
      .mockImplementationOnce(() => a.promise as never)
      .mockImplementationOnce(() => b.promise as never)

    let wantA = true
    const pA = attachLsp('p', 'ws-a', 'a.go', () => wantA)
    const pB = attachLsp('p', 'ws-a', 'b.go', () => false)

    wantA = false
    a.resolve(ensureRes('shared-gopls'))
    expect(await pA).toBeNull()
    expect(CloseLSP).not.toHaveBeenCalled()

    b.resolve(ensureRes('shared-gopls'))
    expect(await pB).toBeNull()
    expect(CloseLSP).toHaveBeenCalledWith('shared-gopls')
  })

  test('stale Ensure does not CloseLSP while another language sharing the server is pending', async () => {
    const a = deferred<ReturnType<typeof ensureRes>>()
    const b = deferred<ReturnType<typeof ensureRes>>()
    vi.mocked(EnsureLSP)
      .mockImplementationOnce(() => a.promise as never)
      .mockImplementationOnce(() => b.promise as never)

    let wantA = true
    const pA = attachLsp('p', 'ws-a', 'a.ts', () => wantA)
    const pB = attachLsp('p', 'ws-a', 'b.tsx', () => false)

    wantA = false
    a.resolve(ensureRes('shared-ts-server', {
      serverKey: 'typescript-language-server',
      fileUri: 'file:///tmp/ws/a.ts',
      languageId: 'typescript',
    }))
    expect(await pA).toBeNull()
    expect(CloseLSP).not.toHaveBeenCalled()

    b.resolve(ensureRes('shared-ts-server', {
      serverKey: 'typescript-language-server',
      fileUri: 'file:///tmp/ws/b.tsx',
      languageId: 'typescriptreact',
    }))
    expect(await pB).toBeNull()
    expect(CloseLSP).toHaveBeenCalledWith('shared-ts-server')
  })

  test('stale Ensure closes once an unrelated pending Ensure in the same workspace finishes', async () => {
    const a = deferred<ReturnType<typeof ensureRes>>()
    const b = deferred<ReturnType<typeof ensureRes>>()
    vi.mocked(EnsureLSP)
      .mockImplementationOnce(() => a.promise as never)
      .mockImplementationOnce(() => b.promise as never)

    let wantA = true
    const pA = attachLsp('p', 'ws-a', 'a.go', () => wantA)
    const pB = attachLsp('p', 'ws-a', 'a.py', () => false)

    wantA = false
    a.resolve(ensureRes('gopls-1'))
    expect(await pA).toBeNull()
    expect(CloseLSP).not.toHaveBeenCalled()

    b.resolve(ensureRes('pyright-1', {
      serverKey: 'basedpyright',
      fileUri: 'file:///tmp/ws/a.py',
      languageId: 'python',
    }))
    expect(await pB).toBeNull()
    expect(CloseLSP).toHaveBeenCalledWith('gopls-1')
    expect(CloseLSP).toHaveBeenCalledWith('pyright-1')
  })
})

describe('notifyDidSave', () => {
  test('only notifies the workspace that saved the file', () => {
    const a = stubWorkspace('ws-a', 'file:///tmp/a')
    const b = stubWorkspace('ws-b', 'file:///tmp/b')
    a.ws.openFile('file:///tmp/a/src/foo.ts', 'typescript', fakeView('from-a'))
    b.ws.openFile('file:///tmp/b/src/foo.ts', 'typescript', fakeView('from-b'))
    ;(a.client as { serverCapabilities: unknown }).serverCapabilities = {
      textDocumentSync: { save: { includeText: true } },
    }
    ;(b.client as { serverCapabilities: unknown }).serverCapabilities = {
      textDocumentSync: { save: { includeText: true } },
    }
    installLspSessionForTest({ id: 'sess-a', workspaceId: 'ws-a', rootUri: a.rootUri, client: a.client })
    installLspSessionForTest({ id: 'sess-b', workspaceId: 'ws-b', rootUri: b.rootUri, client: b.client })

    notifyDidSave('ws-a', 'src/foo.ts', 'saved-in-a')

    expect(a.client.notification).toHaveBeenCalledWith(
      'textDocument/didSave',
      { textDocument: { uri: 'file:///tmp/a/src/foo.ts' }, text: 'saved-in-a' },
    )
    expect(b.client.notification).not.toHaveBeenCalled()
  })
})

describe('vue tsserver pairing', () => {
  test('parses nested tsserver/request params and builds the notify payload', () => {
    const parsed = parseVueTsserverRequest([[42, 'completionInfo', { file: 'App.vue' }]])
    expect(parsed).toEqual({ id: 42, command: 'completionInfo', payload: { file: 'App.vue' } })
    expect(parseVueTsserverRequest([7, 'quickinfo', null])).toEqual({
      id: 7,
      command: 'quickinfo',
      payload: null,
    })
    expect(parseVueTsserverRequest(null)).toBeNull()
    expect(JSON.parse(vueTsserverResponse(42, { entries: [] }))).toEqual({
      jsonrpc: '2.0',
      method: 'tsserver/response',
      params: [[42, { entries: [] }]],
    })
    expect(JSON.parse(vueTsserverResponse(1, undefined)).params).toEqual([[1, null]])
  })

  test('injects @vue/typescript-plugin into typescript-language-server initialize', () => {
    const raw = JSON.stringify({
      jsonrpc: '2.0',
      id: 1,
      method: 'initialize',
      params: { rootUri: 'file:///tmp/ws' },
    })
    const next = JSON.parse(withVueTsPlugin(raw, '/data/bin/lsp/vue-language-server/node_modules/@vue/language-server'))
    expect(next.params.initializationOptions.plugins).toEqual([{
      name: '@vue/typescript-plugin',
      location: '/data/bin/lsp/vue-language-server/node_modules/@vue/language-server',
      languages: ['vue'],
      configNamespace: 'typescript',
    }])
    expect(withVueTsPlugin('{"method":"initialized"}', '/p')).toBe('{"method":"initialized"}')
  })
})
