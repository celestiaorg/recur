// OS clipboard writes for the Wails desktop shell.
// navigator.clipboard.writeText often fails here: WKWebView may lack a secure
// clipboard grant, and any await before writeText drops the user-activation
// gesture. Prefer the native Wails Clipboard bridge for text. Image writes use
// ClipboardItem with a Promise body so the gesture stays valid while we load
// and transcode (Wails Clipboard is text-only).
import { Clipboard } from '@wailsio/runtime'

export async function copyText(text: string): Promise<void> {
  try {
    await Clipboard.SetText(text)
    return
  } catch {
    // Browser / e2e without the Wails bridge.
  }
  await navigator.clipboard.writeText(text)
}

/** Read text from the OS clipboard (Wails bridge, then navigator fallback). */
export async function readText(): Promise<string> {
  try {
    // Wails v3 Clipboard exposes Text(); some builds use GetText().
    const api = Clipboard as { Text?: () => Promise<string>; GetText?: () => Promise<string> }
    const t = api.Text ? await api.Text() : api.GetText ? await api.GetText() : null
    if (typeof t === 'string') return t
  } catch {
    // Browser / e2e without the Wails bridge.
  }
  try {
    return await navigator.clipboard.readText()
  } catch {
    return ''
  }
}

async function loadAsPngBlob(src: string): Promise<Blob> {
  const res = await fetch(src)
  if (!res.ok) throw new Error(`Could not load image (${res.status})`)
  const blob = await res.blob()
  if (blob.type === 'image/png') return blob
  const bmp = await createImageBitmap(blob)
  try {
    const canvas = document.createElement('canvas')
    canvas.width = bmp.width
    canvas.height = bmp.height
    const ctx = canvas.getContext('2d')
    if (!ctx) throw new Error('Could not encode image')
    ctx.drawImage(bmp, 0, 0)
    const png = await new Promise<Blob>((resolve, reject) => {
      canvas.toBlob((b) => (b ? resolve(b) : reject(new Error('Could not encode PNG'))), 'image/png')
    })
    return png
  } finally {
    bmp.close()
  }
}

/** Copy an image (data URL or same-origin URL) to the OS clipboard as PNG. */
export async function copyImage(src: string): Promise<void> {
  if (!src) throw new Error('No image to copy')
  if (!navigator.clipboard?.write || typeof ClipboardItem === 'undefined') {
    throw new Error('Clipboard images are not supported here')
  }
  // Pass a Promise into ClipboardItem so the write stays inside the click/
  // keydown activation window while fetch + PNG encode run.
  await navigator.clipboard.write([
    new ClipboardItem({ 'image/png': loadAsPngBlob(src) }),
  ])
}
