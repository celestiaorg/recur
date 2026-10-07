/**
 * Bundle [[prompt]] chat actions: list, run (TS Worker / shebang), invoke helpers.
 */
import * as BundleService from '../../bindings/recur/internal/app/bundleservice'
import type { PromptActionInfo, PromptContextDTO } from '../../bindings/recur/internal/app/models'
import {
  normalizePromptDisplay,
  runCompleteModule,
  runPromptModule,
  type PromptCompleteHit,
  type PromptCompleteResult,
  type PromptContext,
  type PromptResult,
} from './macro-worker'
import type { EditorContext } from './commands'
import { skillQueryAt } from './agentchat'
import { parsePromptChipValues, type PromptChipArg } from './composerDom'
import { get, writable } from 'svelte/store'
import { settings } from './stores'
import type { ChatSurface } from './chat-surface'

export type { PromptActionInfo, PromptCompleteHit }

export type PromptActionRequest = {
  workspaceId: string
  slash: string
  args: string
  nonce: number
}

/** Latest pending titlebar prompt invoke; WorkspaceView consumes matching ones. */
export const promptActionRequest = writable<PromptActionRequest | null>(null)

/** Ask the open workspace view to run a slash prompt (commit, update, …). */
export function requestPromptAction(workspaceId: string, slash: string, args = ''): void {
  const s = slash.replace(/^\//, '').trim()
  if (!workspaceId || !s) return
  promptActionRequest.set({ workspaceId, slash: s, args, nonce: Date.now() })
}

/** Claim a pending request for this workspace (once). */
export function takePromptActionRequest(workspaceId: string): { slash: string; args: string } | null {
  const req = get(promptActionRequest)
  if (!req || req.workspaceId !== workspaceId) return null
  promptActionRequest.set(null)
  return { slash: req.slash, args: req.args ?? '' }
}

export type PromptTab = { code: string; title?: string; mode?: string; promptGroup?: string }

export type PromptTabPick =
  | { action: 'current' }
  | { action: 'select'; code: string }
  | { action: 'create' }

/**
 * Where a mode-routed prompt should land.
 * No group: nearest idle tab in mode, else create.
 * Group: nearest idle tab already in that group (no unsent draft); else unused
 * in mode (claimed); else create.
 */
export function pickPromptTab<T extends PromptTab>(opts: {
  tabs: T[]
  activeCode: string
  mode: string
  group?: string
  isBusy: (code: string) => boolean
  tabMode: (tab: T) => string
  started?: (tab: T) => boolean
  drafted?: (tab: T) => boolean
  conductorLocked?: (code: string) => boolean
}): PromptTabPick {
  const modeOk = (tab: T) =>
    tab.code !== 'MA'
    && !opts.isBusy(tab.code)
    && opts.tabMode(tab) === opts.mode
    && !(opts.mode === 'loop' && opts.conductorLocked?.(tab.code))
  const newest = (pred: (tab: T) => boolean): T | undefined => {
    for (let i = opts.tabs.length - 1; i >= 0; i--) {
      const tab = opts.tabs[i]
      if (tab && pred(tab)) return tab
    }
  }
  const pick = (tab: T): PromptTabPick =>
    tab.code === opts.activeCode ? { action: 'current' } : { action: 'select', code: tab.code }

  const current = opts.tabs.find((t) => t.code === opts.activeCode)
  if (opts.group) {
    const inGroup = (tab: T) =>
      modeOk(tab) && tab.promptGroup === opts.group && !opts.drafted?.(tab)
    if (current && inGroup(current)) return { action: 'current' }
    const grouped = newest(inGroup)
    if (grouped) return pick(grouped)
    const unused = (tab: T) => modeOk(tab) && !opts.started?.(tab) && !opts.drafted?.(tab)
    if (current && unused(current)) return { action: 'current' }
    const u = newest(unused)
    if (u) return pick(u)
    return { action: 'create' }
  }
  if (current && modeOk(current)) return { action: 'current' }
  const tab = newest(modeOk)
  if (tab) return pick(tab)
  return { action: 'create' }
}

const DEFAULT_PROMPT_TIMEOUT_MS = 5_000

let cached: PromptActionInfo[] | null = null
let cacheErr: string | null = null

/** Prompt action ids the user disabled in Settings → Bundles. */
export function disabledPromptIds(): Set<string> {
  const list = get(settings)?.disabledBundlePrompts
  if (!list?.length) return new Set()
  return new Set(list.filter(Boolean))
}

export function promptEnabled(id: string): boolean {
  return !disabledPromptIds().has(id)
}

function filterEnabledPrompts(list: PromptActionInfo[]): PromptActionInfo[] {
  const disabled = disabledPromptIds()
  if (disabled.size === 0) return list
  return list.filter((p) => !disabled.has(p.id))
}

export async function listPromptActions(force = false): Promise<PromptActionInfo[]> {
  return filterEnabledPrompts(await listPromptActionsUnfiltered(force))
}

/** Every discovered prompt including those disabled in Settings. */
export async function listPromptActionsUnfiltered(force = false): Promise<PromptActionInfo[]> {
  if (!force && cached) return cached
  try {
    const list = await BundleService.ListPromptActions()
    cached = list ?? []
    cacheErr = null
    return cached
  } catch (err) {
    cacheErr = err instanceof Error ? err.message : String(err)
    // Do not cache [] — a truthy empty array would poison every later call
    // until reload. Leave cached null so the next attempt retries.
    return []
  }
}

export function promptActionsCacheError(): string | null {
  return cacheErr
}

export function findPromptBySlash(slash: string, list: PromptActionInfo[]): PromptActionInfo | undefined {
  const s = slash.replace(/^\//, '').toLowerCase()
  return list.find((p) => p.slash === s)
}

/** True when picking from `/` should insert arg slots instead of invoking. */
export function promptTakesArgs(info: PromptActionInfo | undefined | null): boolean {
  return !!info?.takesArgs || !!(info?.args && info.args.length)
}

/** Named composer slots for a takesArgs prompt (synthesizes `args` when omitted). */
export function promptArgSpecs(info: PromptActionInfo | undefined | null): PromptChipArg[] {
  const listed = (info?.args ?? []).filter((a) => a.name)
  if (listed.length) {
    return listed.map((a) => ({
      name: a.name,
      placeholder: a.placeholder || info?.argPlaceholder || '',
      optional: !!a.optional,
    }))
  }
  if (info?.takesArgs) return [{ name: 'args', placeholder: info.argPlaceholder || '' }]
  return []
}

/** True when every slot is optional — empty Enter invokes instead of staying in the chip. */
export function promptArgsOptional(info: PromptActionInfo | undefined | null): boolean {
  const specs = promptArgSpecs(info)
  return specs.length > 0 && specs.every((s) => !!s.optional)
}

/** True when the focused slot (or the first slot) is optional. */
export function promptArgSlotOptional(info: PromptActionInfo | undefined | null, arg?: string): boolean {
  const specs = promptArgSpecs(info)
  if (!specs.length) return false
  const spec = (arg && specs.find((s) => s.name === arg)) || specs[0]
  return !!spec.optional
}

/** True when every required slot has a non-empty value. Optional slots may be blank.
 *  Missing/zero specs are not invokable (catalog still loading, or not a takesArgs prompt). */
export function promptRequiredArgsFilled(info: PromptActionInfo | undefined | null, values: string[]): boolean {
  const specs = promptArgSpecs(info)
  return specs.length > 0 && specs.every((s, i) => !!s.optional || !!(values[i] ?? '').trim())
}

export function paramsFromArgs(info: PromptActionInfo | undefined | null, args: string): Record<string, string> {
  const specs = promptArgSpecs(info)
  if (!specs.length) return {}
  const values = parsePromptChipValues(args, { slash: info?.slash || '', args: specs })
  const out: Record<string, string> = {}
  specs.forEach((s, i) => { out[s.name] = values[i] ?? '' })
  return out
}

/**
 * Gate for offering prompt actions in the `/` menu (plan §Composer):
 * 1. slash token anchored at offset 0 (permanent)
 * 2. nothing but whitespace after the `/name` token (menu closes once args start)
 * Callers must also require chatAttachments.length === 0.
 */
export function promptActionQueryAt(
  text: string,
  caret: number,
): { start: number; end: number; query: string } | null {
  const q = skillQueryAt(text, caret)
  if (!q || q.start !== 0) return null
  const after = text.slice(q.end)
  if (after.length && !/^\s*$/.test(after)) return null
  return q
}

const COMMITTED_PROMPT = /^\/([a-z][a-z0-9-]*)(?:\s+([\s\S]*))?$/
const PROMPT_ARGS_LINE = /^(\s*)\/([a-z][a-z0-9-]*)(\s+)([\s\S]*)$/

/**
 * Whole-composer `/slash` or `/slash args` (trimmed). Used on Enter/Send when
 * the slash menu is not handling the key. Mid-line slashes return null.
 */
export function parseCommittedPromptLine(text: string): { slash: string; args: string } | null {
  const t = (text ?? '').trim()
  const m = COMMITTED_PROMPT.exec(t)
  if (!m) return null
  return { slash: m[1]!, args: (m[2] ?? '').trim() }
}

export type PromptArgsQuery = {
  slash: string
  args: string
  argsStart: number
  /** Active [[prompt.arg]] name when the caret is in a labeled slot. */
  arg?: string
}

/**
 * Whole-composer `/slash args` while the caret is in the args region.
 * Unlike {@link promptActionQueryAt}, this stays open after trailing args start
 * so the host can offer parameter completion (Linear issue search, …).
 */
export function promptArgsQueryAt(
  text: string,
  caret: number,
  takesArgs: (slash: string) => boolean,
): PromptArgsQuery | null {
  const raw = text ?? ''
  if (caret < 0 || caret > raw.length) return null
  const m = PROMPT_ARGS_LINE.exec(raw)
  if (!m) return null
  const slash = m[2]!
  if (!takesArgs(slash)) return null
  const prefixLen = m[1]!.length + 1 + slash.length
  if (caret < prefixLen) return null
  return {
    slash,
    args: (m[4] ?? '').trimEnd(),
    argsStart: prefixLen + m[3]!.length,
  }
}

export function buildPromptContext(
  ws: { id: string; projectId: string; worktreePath: string; port?: number },
  editor?: Partial<EditorContext> | null,
  surface?: ChatSurface | null,
  args = '',
  params?: Record<string, string>,
): PromptContext {
  const path = editor?.path ?? ''
  return {
    workspaceId: ws.id,
    projectId: ws.projectId,
    worktree: ws.worktreePath,
    port: ws.port ?? 0,
    selectedText: editor?.selectedText ?? '',
    document: editor?.document ?? '',
    currentLine: editor?.currentLine ?? '',
    lineNumber: editor?.lineNumber ?? 0,
    column: editor?.column ?? 0,
    currentWord: editor?.currentWord ?? '',
    filepath: path,
    filename: path.split('/').pop() ?? path,
    scope: editor?.scope ?? '',
    clipboard: editor?.clipboard ?? '',
    tabSize: editor?.tabSize ?? 4,
    softTabs: editor?.softTabs ?? true,
    surface: surface?.surface ?? 'workspace',
    args,
    params,
  }
}

function toDTO(ctx: PromptContext, timeoutMs: number): PromptContextDTO {
  return {
    workspaceId: ctx.workspaceId,
    projectId: ctx.projectId,
    worktree: ctx.worktree,
    port: ctx.port,
    selectedText: ctx.selectedText,
    document: ctx.document,
    currentLine: ctx.currentLine,
    lineNumber: ctx.lineNumber,
    column: ctx.column,
    currentWord: ctx.currentWord,
    filepath: ctx.filepath,
    filename: ctx.filename,
    scope: ctx.scope,
    clipboard: ctx.clipboard,
    tabSize: ctx.tabSize,
    softTabs: ctx.softTabs,
    workDir: ctx.worktree,
    timeoutMs: timeoutMs > 0 ? timeoutMs : 0,
    surface: ctx.surface ?? '',
    chatCode: ctx.chatCode ?? '',
    args: ctx.args ?? '',
    params: ctx.params ?? {},
  }
}

/**
 * Human message from a prompt-action failure. Wails often rejects with a
 * RuntimeError JSON blob (`{"message":"...","kind":"RuntimeError"}`); shebang
 * failures are wrapped as `prompt "id" exited N: <stderr>`.
 */
export function promptActionErrorMessage(err: unknown): string {
  let raw =
    err && typeof err === 'object' && 'message' in err
      ? String((err as { message: unknown }).message)
      : String(err ?? '')
  if (raw.startsWith('{')) {
    try {
      const parsed = JSON.parse(raw) as { message?: unknown }
      if (typeof parsed.message === 'string' && parsed.message) raw = parsed.message
    } catch {
      /* not JSON */
    }
  }
  const stripped = raw.replace(/^prompt "[^"]+" exited \d+:\s*/i, '').trim()
  return stripped || raw.trim() || 'Prompt failed'
}

async function raceAbort<T>(p: Promise<T>, signal?: AbortSignal, timeoutMs = 0): Promise<T> {
  return new Promise((resolve, reject) => {
    if (signal?.aborted) {
      reject(new Error('prompt cancelled'))
      return
    }
    let settled = false
    const finish = (fn: () => void) => {
      if (settled) return
      settled = true
      signal?.removeEventListener('abort', onAbort)
      if (timer) clearTimeout(timer)
      fn()
    }
    const onAbort = () => finish(() => reject(new Error('prompt cancelled')))
    const timer = timeoutMs > 0
      ? setTimeout(() => finish(() => reject(new Error('prompt timed out'))), timeoutMs)
      : null
    signal?.addEventListener('abort', onAbort, { once: true })
    p.then(
      (v) => finish(() => resolve(v)),
      (e) => finish(() => reject(e)),
    )
  })
}

const COMPLETE_TIMEOUT_MS = 4_000

/** Run one prompt-arg complete script; empty when the prompt has none. */
export async function completePromptArgs(
  info: PromptActionInfo,
  args: string,
  opts?: {
    workspaceId?: string
    signal?: AbortSignal
    arg?: string
    params?: Record<string, string>
  },
): Promise<PromptCompleteResult> {
  if (!info?.hasComplete) return { hits: [] }
  const timeoutMs = COMPLETE_TIMEOUT_MS
  const completeCtx = { args: args ?? '', arg: opts?.arg, params: opts?.params }
  if (info.completeInterpreter === 'typescript') {
    const code = await BundleService.CompiledCompleteModule(info.id)
    return runCompleteModule(code, completeCtx, timeoutMs, opts?.signal)
  }
  const res = await raceAbort(
    BundleService.RunPromptComplete(info.id, {
      args: args ?? '',
      arg: opts?.arg ?? '',
      params: opts?.params ?? {},
      workspaceId: opts?.workspaceId ?? '',
    }),
    opts?.signal,
    timeoutMs,
  )
  return {
    hits: (res.hits ?? []).map((h) => ({
      insert: h.insert ?? '',
      label: h.label ?? h.insert ?? '',
      detail: h.detail ?? '',
      badge: h.badge ?? '',
      action: h.action ?? '',
    })),
    message: res.message ?? '',
  }
}

/** Run one prompt action; does not send chat. */
export async function runPromptAction(
  info: PromptActionInfo,
  ctx: PromptContext,
  signal?: AbortSignal,
): Promise<PromptResult> {
  const timeoutMs = (info.timeoutMs ?? 0) > 0 ? info.timeoutMs! : DEFAULT_PROMPT_TIMEOUT_MS
  if (info.interpreter === 'typescript') {
    const code = await BundleService.CompiledPromptModule(info.id)
    return runPromptModule(code, ctx, info.name, timeoutMs, signal)
  }
  const res = await BundleService.RunPromptAction(info.id, toDTO(ctx, timeoutMs))
  return {
    prompt: res.prompt ?? '',
    display: normalizePromptDisplay(res.display ?? '', info.name),
    freshSession: res.freshSession ?? undefined,
  }
}
