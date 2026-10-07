// Observatory LSP bridge: Wails stdio proxy ↔ @codemirror/lsp-client.
import { Browser, Events } from '@wailsio/runtime'
import {
  LSPClient,
  LSPPlugin,
  Workspace,
  serverCompletion,
  hoverTooltips,
  signatureHelp,
  formatKeymap,
  jumpToDefinition,
  type Transport,
  type WorkspaceFile,
  type LSPClientExtension,
} from '@codemirror/lsp-client'
import { EditorView, keymap, tooltips, ViewPlugin, type KeyBinding, type ViewUpdate } from '@codemirror/view'
import { ChangeSet, Text, type Extension, type TransactionSpec } from '@codemirror/state'
import { setDiagnostics, type Diagnostic } from '@codemirror/lint'
import { CloseLSP, EnsureLSP, WriteLSP } from '../../bindings/recur/internal/app/lspservice'
import type { OutlineSymbol, SymbolRef } from '../../bindings/recur/internal/domain/models'
import { lspLanguageIdForPath } from './lsp-lang'
import { sanitizeLspHTML } from './sanitize-lsp'
import { setLspProblemsForUri, clearLspProblems, peekLspProblems, type LspProblem } from './lsp-problems'
import { pushToast } from './stores'

async function soft<T>(p: Promise<T>): Promise<T | null> {
  try {
    return await p
  } catch {
    return null
  }
}

function hasProvider(client: LSPClient, key: string): boolean {
  const caps = client.serverCapabilities as Record<string, unknown> | null
  return !!caps?.[key]
}

export function lspHasDocumentSymbol(client: LSPClient): boolean {
  return hasProvider(client, 'documentSymbolProvider')
}

function joinRootUri(rootUri: string, relPath: string): string {
  const root = rootUri.endsWith('/') ? rootUri : rootUri + '/'
  return root + relPath.split('/').map(encodeURIComponent).join('/')
}

type MsgHandler = (message: string) => void

const handlers = new Map<string, Set<MsgHandler>>()
let wired = false

/** Open http(s) links from LSP hover/completion docs in the system browser. */
function wireExternalDocLinks() {
  document.addEventListener(
    'click',
    (e) => {
      const el = e.target
      if (!(el instanceof Element)) return
      const a = el.closest('a[href]')
      if (!(a instanceof HTMLAnchorElement)) return
      if (!a.closest('.cm-lsp-documentation, .cm-tooltip.cm-tooltip-hover, .cm-completionInfo, .cm-lsp-signature-tooltip')) {
        return
      }
      const href = a.href || a.getAttribute('href') || ''
      if (!/^https?:\/\//i.test(href)) return
      e.preventDefault()
      e.stopPropagation()
      void Browser.OpenURL(href).catch(() => {
        window.open(href, '_blank', 'noopener,noreferrer')
      })
    },
    true,
  )
}

/** Call once from api.wireEvents. */
export function wireLspEvents() {
  if (wired) return
  wired = true
  wireExternalDocLinks()
  Events.On('lsp.message', (e: any) => {
    // Defer before touching handler set — keep Wails Emit off the stack.
    const data = e?.data
    queueMicrotask(() => {
      const id = data?.id
      const message = data?.message
      if (!id || typeof message !== 'string') return
      const set = handlers.get(id)
      if (!set) return
      for (const h of set) h(message)
    })
  })
  Events.On('lsp.exit', (e: any) => {
    const id = e?.data?.id
    queueMicrotask(() => {
      if (!id) return
      dropSession(id)
    })
  })
}

function addHandler(id: string, h: MsgHandler) {
  let set = handlers.get(id)
  if (!set) {
    set = new Set()
    handlers.set(id, set)
  }
  set.add(h)
}

function removeHandler(id: string, h: MsgHandler) {
  const set = handlers.get(id)
  if (!set) return
  set.delete(h)
  if (set.size === 0) handlers.delete(id)
}

/** Maps subscriber fn → microtask-wrapped fn for correct unsubscribe. */
const subscribeWraps = new WeakMap<MsgHandler, MsgHandler>()

/** Answer server→client requests CM’s LSPClient leaves as MethodNotFound. */
function handleServerRequest(sessionId: string, raw: string): boolean {
  let msg: { jsonrpc?: string; id?: string | number; method?: string; params?: any }
  try {
    msg = JSON.parse(raw)
  } catch {
    return false
  }
  // Requests have id + method; responses have id + result/error; notifications have method only.
  if (msg == null || msg.id == null || typeof msg.method !== 'string') return false

  const reply = (result: unknown) => {
    queueMicrotask(() => {
      void WriteLSP(sessionId, JSON.stringify({ jsonrpc: '2.0', id: msg.id, result }))
    })
  }

  if (msg.method === 'workspace/configuration') {
    const items = Array.isArray(msg.params?.items) ? msg.params.items : []
    // `null` is "unset"; some servers (taplo) treat that as include=[] and
    // then exclude every document. An empty object keeps their defaults.
    reply(items.map(() => ({})))
    return true
  }

  if (msg.method === 'workspace/workspaceFolders') {
    const slot = sessions.get(sessionId)
    reply(slot?.rootUri ? [{ uri: slot.rootUri, name: 'workspace' }] : [])
    return true
  }

  // gopls dynamic capability registration — ack so the server doesn't stall.
  if (msg.method === 'client/registerCapability' || msg.method === 'client/unregisterCapability') {
    reply(null)
    return true
  }

  if (msg.method === 'window/workDoneProgress/create') {
    reply(null)
    return true
  }

  if (msg.method === 'tsserver/request') {
    void forwardVueTsserverRequest(sessionId, msg.params, reply)
    return true
  }

  return false
}

export function parseVueTsserverRequest(params: unknown): { id: unknown; command: string; payload: unknown } | null {
  let tuple: unknown = params
  if (Array.isArray(params) && params.length === 1 && Array.isArray(params[0])) {
    tuple = params[0]
  }
  if (!Array.isArray(tuple) || tuple.length < 2) return null
  const [id, command, payload] = tuple
  if (typeof command !== 'string' || !command) return null
  return { id, command, payload }
}

export function vueTsserverResponse(id: unknown, body: unknown): string {
  return JSON.stringify({
    jsonrpc: '2.0',
    method: 'tsserver/response',
    params: [[id, body ?? null]],
  })
}

export function withVueTsPlugin(message: string, pluginPath: string): string {
  if (!pluginPath) return message
  let msg: { method?: string; params?: Record<string, unknown> }
  try {
    msg = JSON.parse(message)
  } catch {
    return message
  }
  if (msg?.method !== 'initialize' || !msg.params) return message
  const init = (msg.params.initializationOptions && typeof msg.params.initializationOptions === 'object')
    ? { ...(msg.params.initializationOptions as Record<string, unknown>) }
    : {}
  const plugins = Array.isArray(init.plugins) ? [...init.plugins] : []
  if (!plugins.some((p) => p && typeof p === 'object' && (p as { name?: string }).name === '@vue/typescript-plugin')) {
    plugins.push({
      name: '@vue/typescript-plugin',
      location: pluginPath,
      languages: ['vue'],
      configNamespace: 'typescript',
    })
  }
  init.plugins = plugins
  msg.params.initializationOptions = init
  return JSON.stringify(msg)
}

async function tsClientForWorkspace(workspaceId: string): Promise<LSPClient | null> {
  for (const s of sessions.values()) {
    if (s.workspaceId !== workspaceId || s.serverKey !== 'typescript-language-server') continue
    try {
      await s.client.initializing
    } catch {
      return null
    }
    return s.client
  }
  return null
}

async function forwardVueTsserverRequest(
  vueSessionId: string,
  params: unknown,
  reply: (result: unknown) => void,
) {
  const parsed = parseVueTsserverRequest(params)
  if (!parsed) {
    reply(null)
    return
  }
  const vue = sessions.get(vueSessionId)
  let body: unknown = null
  const ts = vue ? await tsClientForWorkspace(vue.workspaceId) : null
  if (ts) {
    try {
      const r = await ts.request<any, any>('workspace/executeCommand', {
        command: 'typescript.tsserverRequest',
        arguments: [parsed.command, parsed.payload],
      })
      body = r && typeof r === 'object' && r.body !== undefined ? r.body : r
    } catch {
      body = null
    }
  }
  reply(null)
  void WriteLSP(vueSessionId, vueTsserverResponse(parsed.id, body))
}

function workspaceFolder(rootUri: string): { uri: string; name: string } {
  let name = 'workspace'
  try {
    const parts = new URL(rootUri).pathname.split('/').filter(Boolean)
    if (parts.length) name = decodeURIComponent(parts[parts.length - 1])
  } catch { /* ignore */ }
  return { uri: rootUri, name }
}

/**
 * @codemirror/lsp-client initialize sends rootUri but not workspaceFolders.
 * Taplo (and some others) only create a workspace from workspaceFolders — without
 * one, every TOML file is reported as "this document has been excluded".
 */
function withWorkspaceFolders(message: string, rootUri: string): string {
  if (!rootUri) return message
  let msg: { method?: string; params?: Record<string, unknown> }
  try {
    msg = JSON.parse(message)
  } catch {
    return message
  }
  if (msg?.method !== 'initialize' || !msg.params) return message
  if (msg.params.workspaceFolders == null) {
    msg.params.workspaceFolders = [workspaceFolder(rootUri)]
  }
  if (msg.params.rootUri == null) msg.params.rootUri = rootUri
  return JSON.stringify(msg)
}

function rewriteOutgoing(message: string, sessionId: string, rootUri: string): string {
  let next = withWorkspaceFolders(message, rootUri)
  const slot = sessions.get(sessionId)
  if (slot?.serverKey === 'typescript-language-server' && slot.vuePluginPath) {
    next = withVueTsPlugin(next, slot.vuePluginPath)
  }
  return next
}

function makeTransport(sessionId: string, rootUri: string): Transport {
  return {
    // Defer bridge writes. Wails Event.Emit for lsp.message can be synchronous;
    // if the handler (LSPClient.receiveMessage) immediately transport.send()s a
    // follow-up (e.g. initialize → initialized), a sync WriteLSP re-enters Go
    // while Emit is still on the stack and wedges the whole bridge — the editor
    // then looks frozen / non-editable.
    send(message: string) {
      queueMicrotask(() => {
        void WriteLSP(sessionId, rewriteOutgoing(message, sessionId, rootUri))
      })
    },
    subscribe(handler) {
      let wrap = subscribeWraps.get(handler)
      if (!wrap) {
        wrap = (message: string) => {
          queueMicrotask(() => {
            if (handleServerRequest(sessionId, message)) return
            handler(message)
          })
        }
        subscribeWraps.set(handler, wrap)
      }
      addHandler(sessionId, wrap)
    },
    unsubscribe(handler) {
      const wrap = subscribeWraps.get(handler)
      if (!wrap) return
      removeHandler(sessionId, wrap)
    },
  }
}

type SessionSlot = {
  id: string
  workspaceId: string
  rootUri: string
  rootPath: string
  serverKey: string
  vuePluginPath: string
  client: LSPClient
  /** Absolute file URIs that have published diagnostics for this session. */
  diagUris: Set<string>
  /** True once attachLsp has successfully returned this session to an editor. */
  kept: boolean
}

const sessions = new Map<string, SessionSlot>()
const ensureInFlight = new Map<string, number>()
/** simplify: workspace-scoped because the server is unknown until Ensure returns; a same-workspace Ensure for a different server delays CloseLSP until it resolves. Upgrade: language→serverKey map, then key pending by workspace+server. */
const pendingEnsure = new Map<string, number>()
/** Abandoned session id → workspace, held until in-flight Ensures in that workspace resolve. */
const deferredClose = new Map<string, string>()

/** Observatory registers so cross-file goto can open a path and return its view. */
const displayFileByWorkspace = new Map<string, (relPath: string) => Promise<EditorView | null>>()

export function setLspDisplayFile(
  workspaceId: string,
  fn: ((relPath: string) => Promise<EditorView | null>) | null,
) {
  if (!workspaceId) return
  if (fn) displayFileByWorkspace.set(workspaceId, fn)
  else displayFileByWorkspace.delete(workspaceId)
}

export type LspFileIO = {
  unsaved(relPath: string): string | null
  read(relPath: string): Promise<string | null>
  write(relPath: string, text: string): Promise<void>
  applyIfOpen(relPath: string, text: string): boolean
}

const fileIOByWorkspace = new Map<string, LspFileIO>()

export function setLspFileIO(workspaceId: string, io: LspFileIO | null) {
  if (!workspaceId) return
  if (io) fileIOByWorkspace.set(workspaceId, io)
  else fileIOByWorkspace.delete(workspaceId)
}

function fileIOFor(workspaceId: string): LspFileIO | null {
  return fileIOByWorkspace.get(workspaceId) ?? null
}

function displayFileFor(workspaceId: string) {
  return displayFileByWorkspace.get(workspaceId) ?? null
}

function uriToRelPath(rootUri: string, fileUri: string): string {
  const root = rootUri.endsWith('/') ? rootUri : rootUri + '/'
  if (fileUri.startsWith(root)) {
    try {
      return decodeURIComponent(fileUri.slice(root.length))
    } catch {
      return fileUri.slice(root.length)
    }
  }
  // Fallback: last path segment chain after worktree — keep full URI path tail.
  try {
    const u = new URL(fileUri)
    const r = new URL(rootUri)
    if (u.pathname.startsWith(r.pathname)) {
      return decodeURIComponent(u.pathname.slice(r.pathname.length).replace(/^\//, ''))
    }
  } catch { /* ignore */ }
  return fileUri
}

function toSeverity(sev: number | undefined): LspProblem['severity'] {
  return sev === 1 ? 'error' : sev === 2 ? 'warning' : sev === 3 ? 'info' : 'hint'
}

const problemsCapturer: LSPClientExtension = {
  notificationHandlers: {
    'textDocument/publishDiagnostics': (client, params: any) => {
      const uri = String(params?.uri ?? '')
      if (!uri) return true
      let slot: SessionSlot | undefined
      for (const s of sessions.values()) {
        if (s.client === client) {
          slot = s
          break
        }
      }
      const file = client.workspace.getFile(uri)
      if (params?.version != null && file && params.version !== file.version) return true
      const rootUri = slot?.rootUri ?? ''
      const path = rootUri ? uriToRelPath(rootUri, uri) : uri
      const diags = Array.isArray(params?.diagnostics) ? params.diagnostics : []
      const problems: LspProblem[] = diags.map((d: any) => ({
        uri,
        path,
        severity: toSeverity(d.severity),
        message: String(d.message ?? ''),
        line: (d.range?.start?.line ?? 0) + 1,
        character: d.range?.start?.character ?? 0,
        endLine: (d.range?.end?.line ?? 0) + 1,
        endCharacter: d.range?.end?.character ?? 0,
        source: d.source,
        code: d.code,
        sessionId: slot?.id,
        workspaceId: slot?.workspaceId,
      }))
      if (slot) {
        if (problems.length) slot.diagUris.add(uri)
        else slot.diagUris.delete(uri)
        setLspProblemsForUri(uri, problems, slot.id)
      } else {
        setLspProblemsForUri(uri, problems)
      }
      paintMergedDiagnostics(uri)
      return true
    },
  },
}

function paintMergedDiagnostics(uri: string) {
  let view: EditorView | null = null
  for (const s of sessions.values()) {
    const v = s.client.workspace.getFile(uri)?.getView() ?? null
    if (v) view = v
  }
  if (!view) return
  const doc = view.state.doc
  const diags: Diagnostic[] = peekLspProblems()
    .filter((p) => p.uri === uri)
    .map((p) => ({
      from: posToOffset(doc, { line: p.line - 1, character: p.character }),
      to: posToOffset(doc, { line: p.endLine - 1, character: p.endCharacter }),
      severity: p.severity === 'hint' ? 'hint' : p.severity,
      message: p.message,
    }))
  view.dispatch(setDiagnostics(view.state, diags))
}

/**
 * LSP window/showMessage → Recur toasts (not CodeMirror’s top-of-editor dialog).
 * MessageType: 1 Error, 2 Warning, 3 Info, 4 Log.
 * Typical traffic: gopls Info (“Finished loading packages.”), analyzer Warnings,
 * server Errors on crash/config failure. Log stays console-only via the client default
 * for window/logMessage; we swallow Log here too.
 */
const recurShowMessage: LSPClientExtension = {
  notificationHandlers: {
    'window/showMessage': (client, params: any) => {
      const type = Number(params?.type ?? 3)
      const message = String(params?.message ?? '').trim()
      // Always claim the notification so cm-lsp-message dialogs never appear.
      if (!message || type > 3) return true
      let server = 'LSP'
      for (const s of sessions.values()) {
        if (s.client === client) {
          server = s.serverKey
          break
        }
      }
      pushToast(type === 1 ? 'error' : 'info', `${server}: ${message}`)
      return true
    },
  },
}

const codeActionCaps: LSPClientExtension = {
  clientCapabilities: {
    textDocument: {
      codeAction: {
        codeActionLiteralSupport: {
          codeActionKind: {
            valueSet: ['quickfix', 'quickfix.export', '', 'source.organizeImports'],
          },
        },
        isPreferredSupport: true,
        dataSupport: true,
        resolveSupport: { properties: ['edit'] },
      },
      documentSymbol: {
        hierarchicalDocumentSymbolSupport: true,
      },
      publishDiagnostics: { versionSupport: true },
    },
  },
}

/** Ack common server→client workspace requests so gopls doesn’t stall. */
const workspaceRequestCaps: LSPClientExtension = {
  clientCapabilities: {
    workspace: {
      configuration: true,
      workspaceFolders: true,
    },
  },
}

/** Keep hover/signature cards off the editor chrome edge. */
const TOOLTIP_EDGE_MARGIN = 12

/**
 * CM mounts tooltips inside `.cm-editor` by default. Observatory panes use
 * overflow:hidden, so a hover near the right edge is clipped into a skinny
 * strip instead of shifting left. Host on document.body and give CM an inset
 * editor rect so placement leaves margin and can grow leftward.
 */
const lspTooltipPlacement: Extension = tooltips({
  parent: typeof document !== 'undefined' ? document.body : undefined,
  tooltipSpace(view) {
    const r = view.scrollDOM.getBoundingClientRect()
    const m = TOOLTIP_EDGE_MARGIN
    return {
      top: r.top + m,
      left: r.left + m,
      bottom: r.bottom - m,
      right: r.right - m,
    }
  },
})

/**
 * CM sizes hover tooltips to content and only clamps vertical space with an
 * inline height — wide gopls docs (long signatures, pre blocks) spill off-screen.
 * Cap both axes and wrap/scroll inside. minWidth forces right-edge hovers to
 * claim horizontal room (CM then shifts left within tooltipSpace).
 */
const lspTooltipChrome: Extension = EditorView.baseTheme({
  '.cm-tooltip.cm-tooltip-hover': {
    minWidth: 'min(22rem, 100%)',
    maxWidth: 'min(40rem, 100%)',
    maxHeight: 'min(22rem, 45vh)',
    overflowX: 'hidden',
    overflowY: 'auto',
    boxSizing: 'border-box',
  },
  '.cm-lsp-hover-tooltip': {
    maxWidth: '100%',
    boxSizing: 'border-box',
    '& pre': {
      margin: '0.25em 0',
      whiteSpace: 'pre-wrap',
      wordBreak: 'break-word',
      overflowWrap: 'anywhere',
    },
    '& code': {
      whiteSpace: 'pre-wrap',
      wordBreak: 'break-word',
    },
    '& table': {
      display: 'block',
      maxWidth: '100%',
      overflowX: 'auto',
    },
  },
  '.cm-lsp-completion-documentation': {
    maxWidth: 'min(36rem, 60vw)',
    maxHeight: '14em',
    overflow: 'auto',
    '& pre, & code': {
      whiteSpace: 'pre-wrap',
      wordBreak: 'break-word',
    },
  },
})

function editorExtensions(): (Extension | LSPClientExtension)[] {
  return [
    problemsCapturer,
    recurShowMessage,
    codeActionCaps,
    lspTooltipPlacement,
    lspTooltipChrome,
    serverCompletion(),
    hoverTooltips(),
    // F12 / Mod-click are owned by CodeEditor (LSP then heuristic fallback).
    keymap.of([...formatKeymap, ...recurRenameKeymap]),
    signatureHelp(),
  ]
}

const recurRenameKeymap: KeyBinding[] = [
  {
    key: 'F2',
    run(view) {
      void renameWithWorkspaceLoad(view)
      return true
    },
  },
]

async function renameWithWorkspaceLoad(view: EditorView) {
  const plugin = LSPPlugin.get(view)
  const word = view.state.wordAt(view.state.selection.main.head)
  if (!plugin || !word || !hasProvider(plugin.client, 'renameProvider')) return
  const newName = window.prompt('New name', view.state.sliceDoc(word.from, word.to))
  if (!newName) return
  plugin.client.sync()
  const bases = snapshotLspDocs(plugin.client)
  try {
    const edit = await plugin.client.request<any, any>('textDocument/rename', {
      newName,
      position: plugin.toPosition(word.from),
      textDocument: { uri: plugin.uri },
    })
    if (!edit) return
    const ok = await applyWorkspaceEdit(plugin.client, edit, bases)
    if (!ok) pushToast('info', 'Rename skipped: could not load every affected file')
  } catch (err) {
    plugin.reportError('Rename request failed', err)
  }
}

export class RecurWorkspaceFile implements WorkspaceFile {
  constructor(
    readonly uri: string,
    readonly languageId: string,
    public version: number,
    public doc: Text,
    private view: EditorView | null,
  ) {}
  getView() {
    return this.view
  }
  attachView(view: EditorView) {
    this.view = view
  }
  detachView() {
    this.view = null
  }
}

export class RecurWorkspace extends Workspace {
  files: WorkspaceFile[] = []
  private fileVersions: Record<string, number> = Object.create(null)
  private detachedChanges: { file: WorkspaceFile; prevDoc: Text; changes: any }[] = []
  rootUri: string
  workspaceId: string

  constructor(client: LSPClient, rootUri: string, workspaceId = '') {
    super(client)
    this.rootUri = rootUri
    this.workspaceId = workspaceId
  }

  private flushing = false

  private nextVersion(uri: string) {
    return (this.fileVersions[uri] = (this.fileVersions[uri] ?? -1) + 1)
  }

  private flush() {
    if (this.flushing) return
    this.flushing = true
    try {
      this.client.sync()
    } finally {
      this.flushing = false
    }
  }

  replaceDetachedDoc(uri: string, next: string) {
    const file = this.getFile(uri) as RecurWorkspaceFile | null
    if (!file || file.getView()) return
    const newDoc = textToDoc(next)
    if (newDoc.eq(file.doc)) return
    const changes = ChangeSet.of({ from: 0, to: file.doc.length, insert: next }, file.doc.length)
    this.detachedChanges.push({ changes, file, prevDoc: file.doc })
    file.doc = newDoc
    file.version = this.nextVersion(uri)
  }

  syncFiles() {
    const result: { file: WorkspaceFile; prevDoc: Text; changes: any }[] = []
    for (const file of this.files) {
      const view = file.getView()
      if (!view) continue
      const newDoc = view.state.doc
      if (newDoc.eq(file.doc)) continue
      const changes = ChangeSet.of({ from: 0, to: file.doc.length, insert: newDoc }, file.doc.length)
      result.push({ changes, file, prevDoc: file.doc })
      file.doc = newDoc
      ;(file as RecurWorkspaceFile).version = this.nextVersion(file.uri)
      LSPPlugin.get(view)?.clear()
    }
    if (this.detachedChanges.length) {
      result.push(...this.detachedChanges)
      this.detachedChanges = []
    }
    return result
  }

  openFile(uri: string, languageId: string, view: EditorView) {
    const existing = this.getFile(uri) as RecurWorkspaceFile | null
    if (existing) {
      existing.attachView(view)
      this.flush()
      return
    }
    const file = new RecurWorkspaceFile(uri, languageId, this.nextVersion(uri), view.state.doc, view)
    this.files.push(file)
    this.client.didOpen(file)
  }

  closeFile(uri: string, _view: EditorView) {
    const file = this.getFile(uri) as RecurWorkspaceFile | null
    if (!file) return
    this.flush()
    file.detachView()
  }

  updateFile(uri: string, update: TransactionSpec) {
    const file = this.getFile(uri) as RecurWorkspaceFile | null
    if (!file) return
    const view = file.getView()
    if (view) {
      view.dispatch(update)
      return
    }
    if (update.changes) {
      const cs = ChangeSet.of(update.changes, file.doc.length)
      file.doc = cs.apply(file.doc)
      file.version = this.nextVersion(uri)
    }
  }

  async requestFile(uri: string): Promise<WorkspaceFile | null> {
    const existing = this.getFile(uri)
    if (existing) return existing
    const io = fileIOFor(this.workspaceId)
    if (!io) return null
    const rel = uriToRelPath(this.rootUri, uri)
    const text = io.unsaved(rel) ?? (await io.read(rel))
    if (text == null) return null
    const languageId = lspLanguageIdForPath(rel) || 'plaintext'
    const file = new RecurWorkspaceFile(
      uri,
      languageId,
      this.nextVersion(uri),
      textToDoc(text),
      null,
    )
    this.files.push(file)
    this.client.didOpen(file)
    return file
  }

  async displayFile(uri: string): Promise<EditorView | null> {
    const existing = this.getFile(uri)?.getView()
    if (existing) return existing
    const show = displayFileFor(this.workspaceId)
    if (!show) return null
    const rel = uriToRelPath(this.rootUri, uri)
    return show(rel)
  }
}

function textToDoc(text: string): Text {
  return Text.of(text.split('\n'))
}

function dropSession(id: string) {
  deferredClose.delete(id)
  const slot = sessions.get(id)
  if (!slot) return
  try {
    slot.client.disconnect()
  } catch { /* ignore */ }
  for (const uri of slot.diagUris) setLspProblemsForUri(uri, [], slot.id)
  sessions.delete(id)
  handlers.delete(id)
}

const creating = new Map<string, Promise<LSPClient | null>>()

async function getOrCreateClient(info: {
  id: string
  workspaceId: string
  rootUri: string
  rootPath: string
  serverKey: string
  vuePluginPath?: string
}): Promise<LSPClient | null> {
  const existing = sessions.get(info.id)
  if (existing?.client.serverCapabilities) return existing.client
  const pending = creating.get(info.id)
  if (pending) return pending
  const p = createClient(info)
  creating.set(info.id, p)
  try {
    return await p
  } finally {
    creating.delete(info.id)
  }
}

async function createClient(info: {
  id: string
  workspaceId: string
  rootUri: string
  rootPath: string
  serverKey: string
  vuePluginPath?: string
}): Promise<LSPClient | null> {
  const existing = sessions.get(info.id)
  if (existing?.client.serverCapabilities) return existing.client

  const client = new LSPClient({
    rootUri: info.rootUri,
    timeout: 12_000,
    sanitizeHTML: sanitizeLspHTML,
    workspace: (c) => new RecurWorkspace(c, info.rootUri, info.workspaceId),
    notificationHandlers: {
      // First-pass hook lives in problemsCapturer extension; keep config empty
      // so ordering stays extension-based after config.
    },
    extensions: [...editorExtensions(), workspaceRequestCaps],
  })
  sessions.set(info.id, {
    id: info.id,
    workspaceId: info.workspaceId,
    rootUri: info.rootUri,
    rootPath: info.rootPath,
    serverKey: info.serverKey,
    vuePluginPath: info.vuePluginPath || '',
    client,
    diagUris: new Set(),
    kept: false,
  })
  client.connect(makeTransport(info.id, info.rootUri))
  try {
    await client.initializing
  } catch {
    dropSession(info.id)
    void CloseLSP(info.id)
    return null
  }
  if (!client.serverCapabilities) {
    dropSession(info.id)
    void CloseLSP(info.id)
    return null
  }
  if (!(ensureInFlight.get(info.id) ?? 0) && !sessions.get(info.id)?.kept) {
    dropSession(info.id)
    void CloseLSP(info.id)
    return null
  }
  return client
}

export type LspAttach = {
  extension: Extension
  client: LSPClient
  fileUri: string
  languageId: string
  sessionId: string
  rootUri: string
  serverKey: string
} | null

/** Ensure a language server and return a CM plugin extension for this file.
 *  Several servers may attach (e.g. basedpyright + ruff). */
export async function attachLsp(
  projectId: string,
  workspaceId: string,
  relPath: string,
  stillWanted?: () => boolean,
): Promise<LspAttach> {
  const languageId = lspLanguageIdForPath(relPath)

  pendingEnsure.set(workspaceId, (pendingEnsure.get(workspaceId) ?? 0) + 1)
  let ids: string[] = []
  let res: Awaited<ReturnType<typeof EnsureLSP>> | null = null
  try {
    res = await soft(EnsureLSP(projectId, workspaceId, languageId, relPath))
    ids = ensureSessionIds(res)
    claimEnsureIds(ids)
  } finally {
    const n = (pendingEnsure.get(workspaceId) ?? 1) - 1
    if (n > 0) pendingEnsure.set(workspaceId, n)
    else pendingEnsure.delete(workspaceId)
    flushDeferredClose()
  }
  if (stillWanted && !stillWanted()) {
    releaseEnsureIds(ids, true, workspaceId)
    return null
  }
  if (!res?.available || !res.id || !res.fileUri || !res.rootUri) {
    releaseEnsureIds(ids, true, workspaceId)
    return null
  }
  const openLang = languageId || res.languageId || 'plaintext'

  const infos = [res, ...(Array.isArray(res.extra) ? res.extra : [])].filter(
    (s) => s?.available && s.id && s.rootUri,
  )
  const clientInfo = (info: (typeof infos)[number]) => ({
    id: info.id,
    workspaceId,
    rootUri: info.rootUri || res.rootUri,
    rootPath: info.rootPath || res.rootPath,
    serverKey: info.serverKey || '',
    vuePluginPath: info.vuePluginPath || '',
  })
  const tsInfo = infos.find((s) => s.serverKey === 'typescript-language-server')
  if (tsInfo) {
    if (stillWanted && !stillWanted()) {
      releaseEnsureIds(ids, true, workspaceId)
      return null
    }
    await getOrCreateClient(clientInfo(tsInfo))
  }
  const plugins: Extension[] = []
  const clients: LSPClient[] = []
  let primary: { client: LSPClient; sessionId: string; serverKey: string } | null = null
  for (const info of infos) {
    if (stillWanted && !stillWanted()) {
      releaseEnsureIds(ids, true, workspaceId)
      return null
    }
    const client = await getOrCreateClient(clientInfo(info))
    if (!client?.connected || !client.serverCapabilities) continue
    clients.push(client)
    if (!primary) {
      primary = { client, sessionId: info.id, serverKey: info.serverKey || '' }
      plugins.push(client.plugin(res.fileUri, openLang))
    } else {
      plugins.push(secondaryDocSync(client, res.fileUri, openLang))
    }
  }
  if (stillWanted && !stillWanted()) {
    releaseEnsureIds(ids, true, workspaceId)
    return null
  }
  if (!primary || plugins.length === 0) {
    releaseEnsureIds(ids, true, workspaceId)
    return null
  }
  keepEnsureIds(ids)
  releaseEnsureIds(ids, false)
  plugins.push(autoSyncClients(clients))

  return {
    extension: plugins,
    client: primary.client,
    fileUri: res.fileUri,
    languageId: openLang,
    sessionId: primary.sessionId,
    rootUri: res.rootUri,
    serverKey: primary.serverKey,
  }
}

function ensureSessionIds(res: { id?: string; extra?: { id?: string }[] } | null | undefined): string[] {
  if (!res) return []
  const ids: string[] = []
  if (res.id) ids.push(res.id)
  if (Array.isArray(res.extra)) {
    for (const s of res.extra) if (s?.id) ids.push(s.id)
  }
  return ids
}

function claimEnsureIds(ids: string[]) {
  for (const id of ids) ensureInFlight.set(id, (ensureInFlight.get(id) ?? 0) + 1)
}

function keepEnsureIds(ids: string[]) {
  for (const id of ids) {
    const slot = sessions.get(id)
    if (slot) slot.kept = true
    deferredClose.delete(id)
  }
}

function pendingUnknown(workspaceId: string): boolean {
  return !!workspaceId && (pendingEnsure.get(workspaceId) ?? 0) > 0
}

function closeEnsureId(id: string) {
  dropSession(id)
  void CloseLSP(id)
}

function flushDeferredClose() {
  for (const [id, workspaceId] of [...deferredClose]) {
    if ((ensureInFlight.get(id) ?? 0) > 0) continue
    const slot = sessions.get(id)
    if (slot?.kept) {
      deferredClose.delete(id)
      continue
    }
    if (creating.has(id) && !slot) continue
    if (pendingUnknown(slot?.workspaceId || workspaceId)) continue
    closeEnsureId(id)
  }
}

function releaseEnsureIds(ids: string[], abandon: boolean, workspaceId = '') {
  for (const id of ids) {
    const n = (ensureInFlight.get(id) ?? 1) - 1
    if (n > 0) ensureInFlight.set(id, n)
    else ensureInFlight.delete(id)
    if (!abandon || n > 0) continue
    const slot = sessions.get(id)
    if (slot?.kept) continue
    if (creating.has(id) && !slot) continue
    const ws = slot?.workspaceId || workspaceId
    if (pendingUnknown(ws)) {
      deferredClose.set(id, ws)
      continue
    }
    closeEnsureId(id)
  }
}

function secondaryDocSync(client: LSPClient, uri: string, languageId: string): Extension {
  return ViewPlugin.fromClass(class {
    constructor(readonly view: EditorView) {
      client.workspace.openFile(uri, languageId, view)
    }
    destroy() {
      client.workspace.closeFile(uri, this.view)
    }
  })
}

function autoSyncClients(clients: LSPClient[]): Extension {
  return ViewPlugin.fromClass(class {
    pending = -1
    update(update: ViewUpdate) {
      if (!update.docChanged) return
      if (this.pending > -1) clearTimeout(this.pending)
      this.pending = window.setTimeout(() => {
        this.pending = -1
        for (const c of clients) c.sync()
      }, 500)
    }
    destroy() {
      if (this.pending > -1) clearTimeout(this.pending)
      for (const c of clients) c.sync()
    }
  })
}

export function runLspGotoDef(view: EditorView): boolean {
  return jumpToDefinition(view)
}

/** LSP documentSymbol → OutlineSymbol[]; null if unsupported / failed. */
export async function lspOutline(client: LSPClient, fileUri: string): Promise<OutlineSymbol[] | null> {
  if (!hasProvider(client, 'documentSymbolProvider')) return null
  client.sync()
  try {
    const result = await client.request<any, any>('textDocument/documentSymbol', {
      textDocument: { uri: fileUri },
    })
    if (!result) return null
    return flattenSymbols(result)
  } catch {
    return null
  }
}

function symbolKindName(kind: number | undefined): string {
  switch (kind) {
    case 5: return 'class'
    case 6: return 'method'
    case 12: return 'function'
    case 11:
    case 13: return 'var'
    case 14: return 'const'
    case 10: return 'type'
    case 23: return 'export'
    default: return 'symbol'
  }
}

function flattenSymbols(nodes: any[], out: OutlineSymbol[] = []): OutlineSymbol[] {
  for (const n of nodes) {
    if (!n) continue
    // SymbolInformation
    if (n.location?.range) {
      out.push({
        name: String(n.name ?? ''),
        kind: symbolKindName(n.kind),
        line: (n.location.range.start?.line ?? 0) + 1,
        end: (n.location.range.end?.line ?? 0) + 1,
      })
      continue
    }
    // DocumentSymbol
    const range = n.selectionRange ?? n.range
    if (range) {
      out.push({
        name: String(n.name ?? ''),
        kind: symbolKindName(n.kind),
        line: (range.start?.line ?? 0) + 1,
        end: (n.range?.end?.line ?? range.end?.line ?? 0) + 1,
      })
    }
    if (Array.isArray(n.children) && n.children.length) flattenSymbols(n.children, out)
  }
  return out
}

/** LSP references at a 0-based position. */
export async function lspReferences(
  client: LSPClient,
  fileUri: string,
  rootUri: string,
  line: number,
  character: number,
): Promise<SymbolRef[] | null> {
  if (!hasProvider(client, 'referencesProvider')) return null
  client.sync()
  try {
    const locs = await client.request<any, any>('textDocument/references', {
      textDocument: { uri: fileUri },
      position: { line, character },
      context: { includeDeclaration: true },
    })
    if (!Array.isArray(locs)) return null
    return locs.map((loc: any) => ({
      path: uriToRelPath(rootUri, String(loc.uri)),
      line: (loc.range?.start?.line ?? 0) + 1,
      text: `${uriToRelPath(rootUri, String(loc.uri))}:${(loc.range?.start?.line ?? 0) + 1}`,
      def: false,
    }))
  } catch {
    return null
  }
}

export type CodeActionItem = {
  title: string
  kind?: string
  edit?: any
  command?: any
  data?: any
}

/** Quick-fix code actions for a problem range. */
export async function lspCodeActions(
  client: LSPClient,
  problem: LspProblem,
): Promise<CodeActionItem[]> {
  if (!hasProvider(client, 'codeActionProvider')) return []
  client.sync()
  const diagnostic = {
    range: {
      start: { line: problem.line - 1, character: problem.character },
      end: { line: problem.endLine - 1, character: problem.endCharacter },
    },
    message: problem.message,
    severity: problem.severity === 'error' ? 1 : problem.severity === 'warning' ? 2 : 3,
    source: problem.source,
    code: problem.code,
  }
  try {
    const result = await client.request<any, any>('textDocument/codeAction', {
      textDocument: { uri: problem.uri },
      range: diagnostic.range,
      context: {
        diagnostics: [diagnostic],
        only: ['quickfix'],
      },
    })
    if (!Array.isArray(result)) return []
    return result
      .map((a: any) => {
        if (typeof a?.command === 'string') {
          return { title: String(a.title ?? a.command), command: a, data: a.data }
        }
        return {
          title: String(a.title ?? 'Fix'),
          kind: a.kind,
          edit: a.edit,
          command: a.command,
          data: a.data,
        }
      })
      .filter((a: CodeActionItem) => a.title)
  } catch {
    return []
  }
}

export async function resolveCodeAction(client: LSPClient, action: CodeActionItem): Promise<CodeActionItem> {
  if (action.edit || action.data == null) return action
  try {
    const resolved = await client.request<any, any>('codeAction/resolve', {
      title: action.title,
      kind: action.kind,
      edit: action.edit,
      command: action.command,
      data: action.data,
    })
    if (!resolved) return action
    return {
      title: String(resolved.title ?? action.title),
      kind: resolved.kind ?? action.kind,
      edit: resolved.edit ?? action.edit,
      command: resolved.command ?? action.command,
      data: resolved.data ?? action.data,
    }
  } catch {
    return action
  }
}

function collectEditChanges(edit: any): Record<string, any[]> {
  const changes: Record<string, any[]> = { ...(edit?.changes ?? {}) }
  if (Array.isArray(edit?.documentChanges)) {
    for (const dc of edit.documentChanges) {
      if (dc?.textDocument?.uri && Array.isArray(dc.edits)) {
        const ver = dc.textDocument.version
        changes[dc.textDocument.uri] = (changes[dc.textDocument.uri] ?? []).concat(
          dc.edits.map((e: any) => ({ ...e, _docVersion: ver })),
        )
      }
    }
  }
  return changes
}

function applyEditsToText(text: string, edits: any[]): string {
  const doc = textToDoc(text)
  const sorted = [...edits].sort((a, b) => {
    const la = a.range.start.line - b.range.start.line
    if (la !== 0) return -la
    return -(a.range.start.character - b.range.start.character)
  })
  let out = text
  const asDoc = doc
  for (const e of sorted) {
    const from = posToOffset(asDoc, e.range.start)
    const to = posToOffset(asDoc, e.range.end)
    out = out.slice(0, from) + String(e.newText ?? '') + out.slice(to)
  }
  return out
}

export function snapshotLspDocs(client: LSPClient): Map<string, string> {
  const m = new Map<string, string>()
  for (const file of client.workspace.files) {
    const view = file.getView()
    m.set(file.uri, (view ? view.state.doc : file.doc).toString())
  }
  return m
}

type PlannedEdit = {
  uri: string
  edits: any[]
  rel: string
  base: string
  view: EditorView | null
  loaded: boolean
}

function liveDocText(
  client: LSPClient,
  p: PlannedEdit,
  io: LspFileIO | null,
): string | null {
  const file = client.workspace.getFile(p.uri)
  const view = file?.getView() ?? p.view
  if (view) return view.state.doc.toString()
  if (io && p.rel) {
    const unsaved = io.unsaved(p.rel)
    if (unsaved != null) return unsaved
  }
  if (file) return file.doc.toString()
  return null
}

/** Apply a WorkspaceEdit to open editors, unsaved buffers, and on-disk files. */
export async function applyWorkspaceEdit(
  client: LSPClient,
  edit: any,
  bases?: Map<string, string>,
): Promise<boolean> {
  const changes = collectEditChanges(edit)
  const rootUri = (client.workspace as RecurWorkspace).rootUri || ''
  const io = fileIOFor((client.workspace as RecurWorkspace).workspaceId)
  const snapshots = bases ?? snapshotLspDocs(client)
  const planned: PlannedEdit[] = []
  for (const [uri, edits] of Object.entries(changes)) {
    if (!edits?.length) continue
    const file = client.workspace.getFile(uri)
    const view = file?.getView() ?? null
    const rel = rootUri ? uriToRelPath(rootUri, uri) : ''
    let text: string | null = snapshots.get(uri) ?? null
    if (text == null && view) text = view.state.doc.toString()
    if (text == null && io && rel) text = io.unsaved(rel)
    if (text == null && file) text = file.doc.toString()
    if (text != null) snapshots.set(uri, text)
    planned.push({ uri, edits, rel, base: text ?? '', view, loaded: text != null })
  }
  for (const p of planned) {
    if (p.loaded) continue
    let file = client.workspace.getFile(p.uri)
    if (!file && client.workspace.requestFile) {
      file = await client.workspace.requestFile(p.uri)
    }
    const view = file?.getView() ?? null
    let text: string | null = view ? view.state.doc.toString() : null
    if (text == null && io && p.rel) text = io.unsaved(p.rel)
    if (text == null && file) text = file.doc.toString()
    if (text == null && io && p.rel) text = await io.read(p.rel)
    if (text == null) return false
    p.base = text
    p.view = view
    p.loaded = true
  }
  for (const p of planned) {
    const file = client.workspace.getFile(p.uri) as RecurWorkspaceFile | null
    const docVersion = file && typeof file.version === 'number' ? file.version : null
    for (const e of p.edits) {
      if (e._docVersion != null && docVersion != null && e._docVersion !== -1 && e._docVersion !== docVersion) {
        return false
      }
    }
    const live = liveDocText(client, p, io)
    if (live != null && live !== p.base) return false
    p.view = file?.getView() ?? p.view
  }
  for (const p of planned) {
    const next = applyEditsToText(p.base, p.edits)
    if (p.view) {
      p.view.dispatch({
        changes: { from: 0, to: p.view.state.doc.length, insert: next },
        userEvent: 'lsp.fix',
      })
    }
  }
  const ws = client.workspace as RecurWorkspace
  for (const p of planned) {
    if (p.view) continue
    const next = applyEditsToText(p.base, p.edits)
    if (io && p.rel) {
      if (!io.applyIfOpen(p.rel, next)) await io.write(p.rel, next)
    }
    if (ws.getFile(p.uri)?.getView()) continue
    ws.replaceDetachedDoc(p.uri, next)
  }
  client.sync()
  return true
}

function posToOffset(doc: Text, pos: { line: number; character: number }): number {
  const line = doc.line(Math.min(doc.lines, Math.max(1, pos.line + 1)))
  return Math.min(line.to, line.from + pos.character)
}

export function clearLspForWorkspace(workspaceId: string) {
  if (!workspaceId) return
  for (const [id, slot] of [...sessions.entries()]) {
    if (slot.workspaceId !== workspaceId) continue
    dropSession(id)
  }
  setLspFileIO(workspaceId, null)
  setLspDisplayFile(workspaceId, null)
}

export function clearAllLsp() {
  for (const id of [...sessions.keys()]) dropSession(id)
  fileIOByWorkspace.clear()
  displayFileByWorkspace.clear()
  pendingEnsure.clear()
  ensureInFlight.clear()
  deferredClose.clear()
  clearLspProblems()
}

export function clientForUri(uri: string): LSPClient | null {
  for (const s of sessions.values()) {
    if (s.client.workspace.getFile(uri)) return s.client
  }
  for (const s of sessions.values()) {
    if (uri.startsWith(s.rootUri)) return s.client
  }
  return null
}

export function clientForProblem(problem: LspProblem): LSPClient | null {
  if (problem.sessionId) {
    const s = sessions.get(problem.sessionId)
    if (s) return s.client
  }
  return clientForUri(problem.uri)
}

function saveIncludesText(client: LSPClient): boolean {
  const sync = client.serverCapabilities?.textDocumentSync as any
  if (sync && typeof sync === 'object' && sync.save && typeof sync.save === 'object') {
    return !!sync.save.includeText
  }
  return false
}

function wantsDidSave(client: LSPClient): boolean {
  const sync = client.serverCapabilities?.textDocumentSync as any
  if (sync == null) return true
  if (typeof sync === 'number') return sync !== 0
  if (sync.save === false) return false
  return true
}

export function notifyDidSave(workspaceId: string, relPath: string, text: string) {
  if (!workspaceId) return
  for (const s of sessions.values()) {
    if (s.workspaceId !== workspaceId) continue
    const uri = joinRootUri(s.rootUri, relPath)
    if (!s.client.workspace.getFile(uri)) continue
    if (!wantsDidSave(s.client)) continue
    s.client.sync()
    const params: { textDocument: { uri: string }; text?: string } = { textDocument: { uri } }
    if (saveIncludesText(s.client)) params.text = text
    s.client.notification('textDocument/didSave', params)
  }
}

export { uriToRelPath }

export function installLspSessionForTest(opts: {
  id: string
  workspaceId: string
  rootUri: string
  client: LSPClient
}) {
  sessions.set(opts.id, {
    id: opts.id,
    workspaceId: opts.workspaceId,
    rootUri: opts.rootUri,
    rootPath: '',
    serverKey: '',
    vuePluginPath: '',
    client: opts.client,
    diagUris: new Set(),
    kept: true,
  })
}
