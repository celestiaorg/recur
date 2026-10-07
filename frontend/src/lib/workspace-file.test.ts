import { afterEach, describe, expect, test, vi } from 'vitest'
import { externalFileURL, fileNeedsHTTPBody, readExternalEditorFile, workspaceFileURL } from './workspace-file'

describe('workspaceFileURL', () => {
  test('encodes project, path, and optional base revision', () => {
    expect(workspaceFileURL('p1', 'w1', 'assets/icon.png', 12)).toBe(
      '/workspace-file?project=p1&workspace=w1&path=assets%2Ficon.png&t=12',
    )
    expect(workspaceFileURL('p1', 'w1', 'assets/icon.png', 0, 'base')).toBe(
      '/workspace-file?project=p1&workspace=w1&path=assets%2Ficon.png&at=base',
    )
  })
})

describe('externalFileURL', () => {
  test('encodes the absolute path', () => {
    expect(externalFileURL('/Users/josh/Downloads/export-111.csv')).toBe(
      '/external-file?path=%2FUsers%2Fjosh%2FDownloads%2Fexport-111.csv',
    )
  })
})

describe('readExternalEditorFile', () => {
  afterEach(() => {
    vi.unstubAllGlobals()
  })

  test('loads text as a read-only editor file', async () => {
    vi.stubGlobal('fetch', async (input: RequestInfo) => {
      expect(String(input)).toContain('/external-file?')
      return new Response('a,b\n1,2\n', {
        status: 200,
        headers: { 'Content-Type': 'text/plain; charset=utf-8' },
      })
    })
    const got = await readExternalEditorFile('/Users/josh/Downloads/export-111.csv')
    expect(got.content).toBe('a,b\n1,2\n')
    expect(got.writable).toBe(false)
    expect(got.path).toBe('/Users/josh/Downloads/export-111.csv')
  })

  test('maps a failed fetch to not found', async () => {
    vi.stubGlobal('fetch', async () => new Response('not found: /missing.csv', { status: 404 }))
    await expect(readExternalEditorFile('/missing.csv')).rejects.toThrow(/not found/)
  })

  test('maps application/pdf content-type as a binary preview', async () => {
    vi.stubGlobal('fetch', async () => new Response('%PDF-1.4\n', {
      status: 200,
      headers: { 'Content-Type': 'application/pdf', 'Content-Length': '9' },
    }))
    const got = await readExternalEditorFile('/tmp/doc.pdf')
    expect(got.content).toBe('')
    expect(got.mime).toBe('application/pdf')
    expect(got.writable).toBe(false)
  })
})

describe('fileNeedsHTTPBody', () => {
  test('does not fetch when mime is set', () => {
    expect(fileNeedsHTTPBody({ mime: 'application/pdf', size: 1200 })).toBe(false)
    expect(fileNeedsHTTPBody({ mime: 'image/png', size: 80 })).toBe(false)
    expect(fileNeedsHTTPBody({ content: '', size: 80 })).toBe(true)
    expect(fileNeedsHTTPBody({ content: 'hi', size: 2 })).toBe(false)
  })
})
