// Dedicated worker: marked + highlightCode only. The host sanitizes.
import { parseMarkdown } from './markdown-parse'

export type MarkdownWorkerReq = { id: number; src: string }
export type MarkdownWorkerRes = { id: number; html?: string; error?: string }

self.onmessage = (ev: MessageEvent<MarkdownWorkerReq>) => {
  const id = ev.data?.id ?? 0
  try {
    const html = parseMarkdown(ev.data?.src ?? '')
    const res: MarkdownWorkerRes = { id, html }
    self.postMessage(res)
  } catch (err) {
    const res: MarkdownWorkerRes = {
      id,
      error: err instanceof Error ? err.message : String(err),
    }
    self.postMessage(res)
  }
}
