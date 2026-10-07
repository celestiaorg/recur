// Off-thread marked + highlightCode for streaming chat prefixes.
// Settled MarkdownHtml renders synchronously (scroll restore needs real
// heights on the first frame). DOMPurify stays on the main thread.
// Falls back to sync renderMarkdown when Workers are unavailable.
import MarkdownWorker from './markdown-worker.ts?worker'
import { renderMarkdown, sanitizeMarkdown } from './markdown'
import type { ParseMarkdownOpts } from './markdown-parse'
import type { MarkdownWorkerReq, MarkdownWorkerRes } from './markdown-worker'

type Job = {
  id: number
  src: string
  key?: string
  resolve: (html: string) => void
  reject: (err: Error) => void
}

type Cached = { html: string; bytes: number }

/**
 * Settled markdown HTML cache. Byte-budgeted like editor undo history
 * (`EDITOR_HISTORY_MAX_BYTES` = 20 MiB) — generous for chat + findings +
 * a few file previews. Per-entry cap so one giant preview cannot own it.
 */
export const CACHE_MAX_BYTES = 16 * 1024 * 1024
export const CACHE_ENTRY_MAX_BYTES = 2 * 1024 * 1024
/** Drop a wedged worker slot so later blocks can still render. */
const JOB_MS = 2000

const cache = new Map<string, Cached>()
let totalBytes = 0
let maxBytes = CACHE_MAX_BYTES

let seq = 0
let worker: Worker | null = null
let workerFailed = false
let timeoutStrikes = 0
let inflight: Job | null = null
let watchdog: ReturnType<typeof setTimeout> | undefined
const waiting: Job[] = []
const waitingByKey = new Map<string, Job>()

function entryBytes(src: string, html: string): number {
  // JS strings are UTF-16 — this tracks heap, not UTF-8 on the wire.
  return (src.length + html.length) * 2
}

function drop(src: string): void {
  const cur = cache.get(src)
  if (!cur) return
  cache.delete(src)
  totalBytes -= cur.bytes
  if (totalBytes < 0) totalBytes = 0
}

function evict(): void {
  while (totalBytes > maxBytes && cache.size > 1) {
    const oldest = cache.keys().next().value
    if (oldest === undefined) break
    drop(oldest)
  }
}

function cacheGet(src: string): string | undefined {
  const hit = cache.get(src)
  if (!hit) return undefined
  cache.delete(src)
  cache.set(src, hit)
  return hit.html
}

function cacheSet(src: string, html: string) {
  const bytes = entryBytes(src, html)
  if (bytes > CACHE_ENTRY_MAX_BYTES) return
  drop(src)
  cache.set(src, { html, bytes })
  totalBytes += bytes
  evict()
}

/** LRU-promoting read — settled blocks hit this on every mount. */
export function peekMarkdownCache(src: string): string | undefined {
  return cacheGet(src)
}

/** Remember a completed document (not a streaming prefix). */
export function rememberMarkdown(src: string, html: string) {
  if (!src) return
  cacheSet(src, html)
}

/** Sync settled render: cache hit or parse+store. Used by MarkdownHtml. */
export function renderMarkdownCached(src: string, opts?: ParseMarkdownOpts): string {
  if (!src) return ''
  const key = markdownCacheKey(src, opts)
  const hit = cacheGet(key)
  if (hit !== undefined) return hit
  const html = renderMarkdown(src, opts)
  cacheSet(key, html)
  return html
}

function markdownCacheKey(src: string, opts?: ParseMarkdownOpts): string {
  return opts?.demoteImages ? `\x1edemoteimg\n${src}` : src
}

export function clearMarkdownCache() {
  cache.clear()
  totalBytes = 0
  maxBytes = CACHE_MAX_BYTES
}

/** Test helper — shrink the byte budget so eviction is cheap to assert. */
export function setMarkdownCacheMaxBytesForTests(n: number) {
  maxBytes = n
  evict()
}

export function markdownCacheStatsForTests(): { entries: number; bytes: number } {
  return { entries: cache.size, bytes: totalBytes }
}

function finish(job: Job, html: string) {
  // Keyed jobs are streaming prefixes — caching them evicts settled documents.
  if (!job.key) cacheSet(job.src, html)
  job.resolve(html)
}

function runSync(job: Job) {
  try {
    finish(job, renderMarkdown(job.src))
  } catch (err) {
    job.reject(err instanceof Error ? err : new Error(String(err)))
  }
}

function abortJob(job: Job) {
  const err = new Error('markdown render superseded')
  err.name = 'AbortError'
  job.reject(err)
}

function clearWatchdog() {
  if (watchdog !== undefined) clearTimeout(watchdog)
  watchdog = undefined
}

function retireWorker() {
  clearWatchdog()
  try { worker?.terminate() } catch { /* already dead */ }
  worker = null
  timeoutStrikes = 0
}

function armWatchdog(job: Job) {
  clearWatchdog()
  watchdog = setTimeout(() => {
    if (inflight !== job) return
    inflight = null
    watchdog = undefined
    // First timeout: reject this job, keep the worker. Main-thread jank
    // and cold-start eval can burn JOB_MS while the worker is healthy.
    // A second consecutive timeout retires it.
    timeoutStrikes++
    if (timeoutStrikes >= 2) retireWorker()
    const err = new Error('markdown worker timed out')
    err.name = 'TimeoutError'
    job.reject(err)
    pump()
  }, JOB_MS)
}

function getWorker(): Worker | null {
  if (workerFailed) return null
  if (worker) return worker
  if (typeof Worker === 'undefined') {
    workerFailed = true
    return null
  }
  try {
    const w = new MarkdownWorker()
    w.onmessage = (ev: MessageEvent<MarkdownWorkerRes>) => {
      const job = inflight
      if (!job) {
        pump()
        return
      }
      // Late reply for a timed-out job — keep waiting for the current id.
      if (ev.data?.id !== job.id) return
      inflight = null
      clearWatchdog()
      timeoutStrikes = 0
      if (typeof ev.data.error === 'string') {
        runSync(job)
      } else {
        finish(job, sanitizeMarkdown(ev.data.html ?? ''))
      }
      pump()
    }
    w.onerror = () => {
      workerFailed = true
      const job = inflight
      inflight = null
      retireWorker()
      if (job) runSync(job)
      pump()
    }
    worker = w
    return w
  } catch {
    workerFailed = true
    return null
  }
}

function pump() {
  if (inflight) return
  const job = waiting.shift()
  if (!job) return
  if (job.key) waitingByKey.delete(job.key)
  const hit = cacheGet(job.src)
  if (hit !== undefined) {
    job.resolve(hit)
    pump()
    return
  }
  const w = getWorker()
  if (!w) {
    runSync(job)
    pump()
    return
  }
  inflight = job
  armWatchdog(job)
  const req: MarkdownWorkerReq = { id: job.id, src: job.src }
  w.postMessage(req)
}

/**
 * Sanitize+highlight markdown. `key` coalesces queued streaming prefixes:
 * a superseded keyed call rejects with `name === 'AbortError'`.
 */
export function renderMarkdownAsync(src: string, opts?: { key?: string }): Promise<string> {
  if (!src) return Promise.resolve('')
  const hit = cacheGet(src)
  if (hit !== undefined) return Promise.resolve(hit)

  return new Promise((resolve, reject) => {
    const job: Job = { id: ++seq, src, key: opts?.key, resolve, reject }
    if (job.key) {
      const prev = waitingByKey.get(job.key)
      if (prev) {
        const i = waiting.indexOf(prev)
        if (i >= 0) waiting.splice(i, 1)
        abortJob(prev)
      }
      waitingByKey.set(job.key, job)
    }
    waiting.push(job)
    pump()
  })
}
