// Sandboxed runner for typescript Recur Bundle macros.
//
// Go esbuild-bundles the command (resolving `@recur/sdk`) to CommonJS. We run
// that in a classic Worker via AsyncFunction so the code never touches the DOM
// or Wails bridge — only the frozen MacroContext we post in.

export type OutputRouting =
  | 'discard'
  | 'replaceSelection'
  | 'replaceDocument'
  | 'replaceInput'
  | 'insertText'
  | 'insertSnippet'
  | 'newDocument'
  | 'showAsTooltip'
  | 'copyToClipboard'
  | 'showAsToast'

/** Runtime output-routing constants (mirrors `@recur/sdk` `Output`). */
export const Output: { readonly [K in OutputRouting]: K } = Object.freeze({
  discard: 'discard',
  replaceSelection: 'replaceSelection',
  replaceDocument: 'replaceDocument',
  replaceInput: 'replaceInput',
  insertText: 'insertText',
  insertSnippet: 'insertSnippet',
  newDocument: 'newDocument',
  showAsTooltip: 'showAsTooltip',
  copyToClipboard: 'copyToClipboard',
  showAsToast: 'showAsToast',
})

/** Mirror of Go bundles.ValidateOutput for Worker-returned routing strings. */
export function validateOutputRouting(s: string): string | null {
  if ((Object.values(Output) as string[]).includes(s)) return null
  if (s === 'showAsHTML') {
    return 'output showAsHTML was removed — use newDocument with language + openInPreview'
  }
  return `unknown output ${JSON.stringify(s)}`
}

export type MacroContext = {
  input: string
  selectedText: string
  document: string
  currentLine: string
  lineNumber: number
  column: number
  currentWord: string
  filepath: string
  filename: string
  scope: string
  clipboard: string
  tabSize: number
  softTabs: boolean
  prompts: Record<string, string>
}

export type MacroResult = {
  text: string
  output?: OutputRouting
  language?: string
  openInPreview?: boolean
}

const WORKER_BOOT = `
self.onmessage = async (ev) => {
  const { id, code, ctx } = ev.data || {};
  try {
    const AsyncFunction = Object.getPrototypeOf(async function () {}).constructor;
    const runner = new AsyncFunction('ctx',
      'const module = { exports: {} };\\n' +
      'const exports = module.exports;\\n' +
      String(code) + '\\n' +
      'const __fn = module.exports.default || module.exports;\\n' +
      "if (typeof __fn !== 'function') throw new Error('macro must export default function (ctx) { … }');\\n" +
      'return await __fn(ctx);'
    );
    const frozen = Object.freeze(Object.assign({}, ctx, {
      prompts: Object.freeze(Object.assign({}, (ctx && ctx.prompts) || {})),
    }));
    const raw = await runner(frozen);
    let text = '';
    let output = undefined;
    let language = undefined;
    let openInPreview = undefined;
    if (typeof raw === 'string') {
      text = raw;
    } else if (raw && typeof raw === 'object') {
      text = raw.text != null ? String(raw.text) : '';
      if (raw.output != null) output = String(raw.output);
      if (raw.language != null && String(raw.language).trim()) language = String(raw.language).trim();
      if (typeof raw.openInPreview === 'boolean') openInPreview = raw.openInPreview;
    } else if (raw != null) {
      throw new Error('macro must return a string or { text, output?, language?, openInPreview? }');
    }
    self.postMessage({ id, ok: true, text, output, language, openInPreview });
  } catch (err) {
    const message = err && err.message ? String(err.message) : String(err);
    self.postMessage({ id, ok: false, error: message });
  }
};
`

let worker: Worker | null = null
let workerURL: string | null = null
let seq = 0
const pending = new Map<number, {
  resolve: (r: MacroResult) => void
  reject: (e: Error) => void
}>()

function resetWorker(err: Error) {
  for (const [, p] of pending) p.reject(err)
  pending.clear()
  worker?.terminate()
  worker = null
  if (workerURL) {
    URL.revokeObjectURL(workerURL)
    workerURL = null
  }
}

function getWorker(): Worker {
  if (worker) return worker
  const blob = new Blob([WORKER_BOOT], { type: 'text/javascript' })
  workerURL = URL.createObjectURL(blob)
  worker = new Worker(workerURL) // classic worker
  worker.onmessage = (ev: MessageEvent) => {
    const data = ev.data || {}
    const p = pending.get(data.id)
    if (!p) return
    pending.delete(data.id)
    if (data.ok) {
      p.resolve({
        text: data.text ?? '',
        output: data.output,
        language: data.language,
        openInPreview: typeof data.openInPreview === 'boolean' ? data.openInPreview : undefined,
      })
    } else {
      p.reject(new Error(data.error || 'macro failed'))
    }
  }
  worker.onerror = (ev) => {
    resetWorker(new Error(ev.message || 'macro worker error'))
  }
  return worker
}

/** Run compiled CommonJS in the sandbox Worker. */
export function runMacroModule(code: string, ctx: MacroContext, timeoutMs = 10_000): Promise<MacroResult> {
  const w = getWorker()
  const id = ++seq
  const ms = timeoutMs && timeoutMs > 0 ? timeoutMs : 10_000
  return new Promise<MacroResult>((resolve, reject) => {
    const timer = setTimeout(() => {
      // Kill the shared worker so a runaway macro cannot starve later runs.
      resetWorker(new Error('macro timed out'))
      reject(new Error('macro timed out'))
    }, ms)
    pending.set(id, {
      resolve: (r) => { clearTimeout(timer); resolve(r) },
      reject: (e) => { clearTimeout(timer); reject(e) },
    })
    w.postMessage({ id, code, ctx })
  })
}

// ---- Prompt actions (separate Worker contract; returns { prompt, display }) ----

export type PromptContext = {
  workspaceId: string
  projectId: string
  worktree: string
  port: number
  selectedText: string
  document: string
  currentLine: string
  lineNumber: number
  column: number
  currentWord: string
  filepath: string
  filename: string
  scope: string
  clipboard: string
  tabSize: number
  softTabs: boolean
  /** Operator looking-at view: workspace, local_pr, github_pr. */
  surface?: string
  /** Trailing composer remainder after `/slash` (empty when none). */
  args?: string
  /** Named [[prompt.arg]] values. */
  params?: Record<string, string>
  /** Chat tab the prompt is sent to (PA, PA2, …). Host-only; not seen by TS prompts. */
  chatCode?: string
}

export type PromptResult = {
  prompt: string
  display: string
  /** Host override of the manifest fresh_session flag (shebang prompts only). */
  freshSession?: boolean
}

export type PromptCompleteContext = {
  args: string
  arg?: string
  params?: Record<string, string>
}

export type PromptCompleteHit = {
  insert: string
  label: string
  psub?: string
  detail?: string
  badge?: string
  action?: string
}

export type PromptCompleteResult = {
  hits?: PromptCompleteHit[]
  message?: string
}

const PROMPT_DISPLAY_MAX = 120

/** Coerce display to a single line ≤ 120 chars (mirrors Go NormalizePromptDisplay). */
export function normalizePromptDisplay(display: string, fallbackName: string): string {
  let s = (display || '').replace(/[\r\n]+/g, ' ').replace(/\s+/g, ' ').trim()
  if (!s) s = (fallbackName || '').replace(/[\r\n]+/g, ' ').replace(/\s+/g, ' ').trim()
  if (s.length > PROMPT_DISPLAY_MAX) s = s.slice(0, PROMPT_DISPLAY_MAX - 1) + '…'
  return s
}

/** Validate Worker/shebang prompt result; returns error message or null. */
export function validatePromptResult(r: { prompt?: unknown; display?: unknown }, fallbackName: string): string | null {
  if (r == null || typeof r !== 'object') return 'prompt action must return { prompt, display? }'
  if (r.prompt != null && typeof r.prompt !== 'string') return 'prompt action must return a prompt string'
  return null
}

const PROMPT_WORKER_BOOT = `
self.onmessage = async (ev) => {
  const { id, code, ctx, fallbackName } = ev.data || {};
  try {
    const AsyncFunction = Object.getPrototypeOf(async function () {}).constructor;
    const runner = new AsyncFunction('ctx',
      'const module = { exports: {} };\\n' +
      'const exports = module.exports;\\n' +
      String(code) + '\\n' +
      'const __fn = module.exports.default || module.exports;\\n' +
      "if (typeof __fn !== 'function') throw new Error('prompt must export default function (ctx) { … }');\\n" +
      'return await __fn(ctx);'
    );
    const frozen = Object.freeze(Object.assign({}, ctx));
    const raw = await runner(frozen);
    if (!raw || typeof raw !== 'object') {
      throw new Error('prompt must return { prompt, display? }');
    }
    const prompt = raw.prompt != null ? String(raw.prompt) : '';
    let display = raw.display != null ? String(raw.display) : '';
    display = display.replace(/[\\r\\n]+/g, ' ').replace(/\\s+/g, ' ').trim();
    if (!display) display = String(fallbackName || '').replace(/[\\r\\n]+/g, ' ').replace(/\\s+/g, ' ').trim();
    if (display.length > 120) display = display.slice(0, 119) + '\\u2026';
    self.postMessage({ id, ok: true, prompt, display });
  } catch (err) {
    const message = err && err.message ? String(err.message) : String(err);
    self.postMessage({ id, ok: false, error: message });
  }
};
`

let promptWorker: Worker | null = null
let promptWorkerURL: string | null = null
let promptSeq = 0
const promptPending = new Map<number, {
  resolve: (r: PromptResult) => void
  reject: (e: Error) => void
}>()

function resetPromptWorker(err: Error) {
  for (const [, p] of promptPending) p.reject(err)
  promptPending.clear()
  promptWorker?.terminate()
  promptWorker = null
  if (promptWorkerURL) {
    URL.revokeObjectURL(promptWorkerURL)
    promptWorkerURL = null
  }
}

function getPromptWorker(): Worker {
  if (promptWorker) return promptWorker
  const blob = new Blob([PROMPT_WORKER_BOOT], { type: 'text/javascript' })
  promptWorkerURL = URL.createObjectURL(blob)
  promptWorker = new Worker(promptWorkerURL)
  promptWorker.onmessage = (ev: MessageEvent) => {
    const data = ev.data || {}
    const p = promptPending.get(data.id)
    if (!p) return
    promptPending.delete(data.id)
    if (data.ok) {
      p.resolve({ prompt: data.prompt ?? '', display: data.display ?? '' })
    } else {
      p.reject(new Error(data.error || 'prompt failed'))
    }
  }
  promptWorker.onerror = (ev) => {
    resetPromptWorker(new Error(ev.message || 'prompt worker error'))
  }
  return promptWorker
}

/** Run compiled prompt-action CommonJS in a sandboxed Worker. */
export function runPromptModule(
  code: string,
  ctx: PromptContext,
  fallbackName: string,
  timeoutMs = 5_000,
  signal?: AbortSignal,
): Promise<PromptResult> {
  const w = getPromptWorker()
  const id = ++promptSeq
  const ms = timeoutMs && timeoutMs > 0 ? timeoutMs : 5_000
  return new Promise<PromptResult>((resolve, reject) => {
    if (signal?.aborted) {
      reject(new Error('prompt cancelled'))
      return
    }
    const timer = setTimeout(() => {
      resetPromptWorker(new Error('prompt timed out'))
      reject(new Error('prompt timed out'))
    }, ms)
    const onAbort = () => {
      clearTimeout(timer)
      promptPending.delete(id)
      resetPromptWorker(new Error('prompt cancelled'))
      reject(new Error('prompt cancelled'))
    }
    signal?.addEventListener('abort', onAbort, { once: true })
    promptPending.set(id, {
      resolve: (r) => {
        clearTimeout(timer)
        signal?.removeEventListener('abort', onAbort)
        resolve(r)
      },
      reject: (e) => {
        clearTimeout(timer)
        signal?.removeEventListener('abort', onAbort)
        reject(e)
      },
    })
    w.postMessage({ id, code, ctx, fallbackName })
  })
}

const COMPLETE_WORKER_BOOT = `
self.onmessage = async (ev) => {
  const { id, code, ctx } = ev.data || {};
  try {
    const AsyncFunction = Object.getPrototypeOf(async function () {}).constructor;
    const runner = new AsyncFunction('ctx',
      'const module = { exports: {} };\\n' +
      'const exports = module.exports;\\n' +
      String(code) + '\\n' +
      'const __fn = module.exports.default || module.exports;\\n' +
      "if (typeof __fn !== 'function') throw new Error('complete must export default function (ctx) { … }');\\n" +
      'return await __fn(ctx);'
    );
    const frozen = Object.freeze(Object.assign({}, ctx));
    const raw = await runner(frozen);
    const hits = [];
    const list = raw && Array.isArray(raw.hits) ? raw.hits : (Array.isArray(raw) ? raw : []);
    for (const h of list) {
      if (!h || typeof h !== 'object') continue;
      const insert = h.insert != null ? String(h.insert) : '';
      const label = h.label != null ? String(h.label) : insert;
      const detail = h.detail != null ? String(h.detail) : '';
      const badge = h.badge != null ? String(h.badge) : '';
      const action = h.action != null ? String(h.action) : '';
      if (!label && !action) continue;
      hits.push({ insert, label, detail, badge, action });
      if (hits.length >= 25) break;
    }
    const message = raw && raw.message != null ? String(raw.message) : '';
    self.postMessage({ id, ok: true, hits, message });
  } catch (err) {
    const message = err && err.message ? String(err.message) : String(err);
    self.postMessage({ id, ok: false, error: message });
  }
};
`

let completeWorker: Worker | null = null
let completeWorkerURL: string | null = null
let completeSeq = 0
const completePending = new Map<number, {
  resolve: (r: PromptCompleteResult) => void
  reject: (e: Error) => void
}>()

function resetCompleteWorker(err: Error) {
  for (const [, p] of completePending) p.reject(err)
  completePending.clear()
  completeWorker?.terminate()
  completeWorker = null
  if (completeWorkerURL) {
    URL.revokeObjectURL(completeWorkerURL)
    completeWorkerURL = null
  }
}

function getCompleteWorker(): Worker {
  if (completeWorker) return completeWorker
  const blob = new Blob([COMPLETE_WORKER_BOOT], { type: 'text/javascript' })
  completeWorkerURL = URL.createObjectURL(blob)
  completeWorker = new Worker(completeWorkerURL)
  completeWorker.onmessage = (ev: MessageEvent) => {
    const data = ev.data || {}
    const p = completePending.get(data.id)
    if (!p) return
    completePending.delete(data.id)
    if (data.ok) {
      p.resolve({ hits: data.hits ?? [], message: data.message ?? '' })
    } else {
      p.reject(new Error(data.error || 'complete failed'))
    }
  }
  completeWorker.onerror = (ev) => {
    resetCompleteWorker(new Error(ev.message || 'complete worker error'))
  }
  return completeWorker
}

/** Run compiled prompt-complete CommonJS in a sandboxed Worker. */
export function runCompleteModule(
  code: string,
  ctx: PromptCompleteContext,
  timeoutMs = 2_000,
  signal?: AbortSignal,
): Promise<PromptCompleteResult> {
  const w = getCompleteWorker()
  const id = ++completeSeq
  const ms = timeoutMs && timeoutMs > 0 ? timeoutMs : 4_000
  return new Promise<PromptCompleteResult>((resolve, reject) => {
    if (signal?.aborted) {
      reject(new Error('prompt cancelled'))
      return
    }
    const timer = setTimeout(() => {
      resetCompleteWorker(new Error('complete timed out'))
      reject(new Error('complete timed out'))
    }, ms)
    const onAbort = () => {
      clearTimeout(timer)
      completePending.delete(id)
      resetCompleteWorker(new Error('prompt cancelled'))
      reject(new Error('prompt cancelled'))
    }
    signal?.addEventListener('abort', onAbort, { once: true })
    completePending.set(id, {
      resolve: (r) => {
        clearTimeout(timer)
        signal?.removeEventListener('abort', onAbort)
        resolve(r)
      },
      reject: (e) => {
        clearTimeout(timer)
        signal?.removeEventListener('abort', onAbort)
        reject(e)
      },
    })
    w.postMessage({ id, code, ctx })
  })
}
