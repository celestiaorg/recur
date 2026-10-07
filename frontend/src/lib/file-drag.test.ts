import { afterEach, describe, expect, test } from 'vitest'
import {
  FILE_DRAG_MIME,
  classifyOsDrop,
  encodeChatDropId,
  isChatImagePath,
  endFileDrag,
  fileDragPaths,
  filesFromWailsDrop,
  isComposerFileDrop,
  isExternalFileDrag,
  isFileDrag,
  isImagePath,
  isAbsFilesystemPath,
  isChipableFilePath,
  normalizeFileDragPaths,
  pathFromFileUrl,
  pathsFromOsDataTransfer,
  relativeToWorkspace,
  quoteForShell,
  startFileDrag,
  terminalDropText,
  resolveExplorerDropPath,
} from './file-drag'

afterEach(() => endFileDrag())

function fakeDt(init: Record<string, string> = {}, files: File[] = []): DataTransfer {
  const data = { ...init }
  const types = Object.keys(data)
  if (files.length && !types.includes('Files')) types.push('Files')
  return {
    types,
    files,
    effectAllowed: 'none',
    dropEffect: 'none',
    setData(type: string, val: string) {
      data[type] = val
      if (!types.includes(type)) types.push(type)
    },
    getData(type: string) {
      return data[type] ?? ''
    },
  } as unknown as DataTransfer
}

describe('normalizeFileDragPaths', () => {
  test('trims, strips leading slashes, and dedupes', () => {
    expect(normalizeFileDragPaths([' /src/a.ts ', '\\src\\b.ts', 'src/a.ts', ''])).toEqual([
      'src/a.ts',
      'src/b.ts',
    ])
  })
})

describe('startFileDrag / fileDragPaths', () => {
  test('module state is the live payload', () => {
    expect(isFileDrag()).toBe(false)
    startFileDrag(['src/main.go'])
    expect(isFileDrag()).toBe(true)
    expect(fileDragPaths()).toEqual(['src/main.go'])
    endFileDrag()
    expect(isFileDrag()).toBe(false)
    expect(fileDragPaths()).toEqual([])
  })

  test('writes MIME onto DataTransfer without text/plain', () => {
    const dt = fakeDt()
    startFileDrag(['src/main.go', 'README.md'], dt)
    expect(dt.effectAllowed).toBe('copyMove')
    expect(dt.getData(FILE_DRAG_MIME)).toBe(JSON.stringify(['src/main.go', 'README.md']))
    expect(dt.getData('text/uri-list')).toBe('src/main.go\r\nREADME.md')
    expect(dt.getData('text/plain')).toBe('')
  })

  test('relative uri-list payload is not an OS file path', () => {
    expect(pathFromFileUrl('src/main.go')).toBe('')
    const dt = fakeDt({ 'text/uri-list': 'src/main.go\r\nREADME.md' })
    expect(pathsFromOsDataTransfer(dt)).toEqual([])
  })

  test('drop can recover paths from MIME after module state is cleared', () => {
    const dt = fakeDt({ [FILE_DRAG_MIME]: JSON.stringify(['frontend/src/a.ts']) })
    expect(isFileDrag(dt)).toBe(true)
    expect(fileDragPaths(dt)).toEqual(['frontend/src/a.ts'])
  })

  test('plain-text drags are not treated as explorer files', () => {
    const dt = fakeDt({ 'text/plain': '@src/main.go' })
    expect(isFileDrag(dt)).toBe(false)
    expect(fileDragPaths(dt)).toEqual([])
  })
})

describe('OS / Finder drops', () => {
  test('Files type is an external drag the composer should accept', () => {
    const dt = fakeDt({ Files: '' })
    expect(isExternalFileDrag(dt)).toBe(true)
    expect(isComposerFileDrop(dt)).toBe(true)
    expect(isFileDrag(dt)).toBe(false)
  })

  test('pathFromFileUrl decodes file URIs and passes through plain paths', () => {
    expect(pathFromFileUrl('file:///Users/josh/src/a.ts')).toBe('/Users/josh/src/a.ts')
    expect(pathFromFileUrl('file:///Users/josh/My%20File.ts')).toBe('/Users/josh/My File.ts')
    expect(pathFromFileUrl('/Users/josh/src/a.ts')).toBe('/Users/josh/src/a.ts')
    expect(pathFromFileUrl('https://example.com/a.ts')).toBe('')
  })

  test('relativeToWorkspace maps abs paths inside the worktree', () => {
    const root = '/mock/worktrees/loop-1'
    expect(relativeToWorkspace('/mock/worktrees/loop-1/src/main.go', root)).toBe('src/main.go')
    expect(relativeToWorkspace('src/main.go', root)).toBe('src/main.go')
    expect(relativeToWorkspace('/elsewhere/main.go', root)).toBeNull()
    expect(relativeToWorkspace(root, root)).toBeNull()
  })

  test('pathsFromOsDataTransfer reads text/uri-list', () => {
    const dt = fakeDt({
      'text/uri-list': 'file:///mock/worktrees/loop-1/src/main.go\n# comment\nfile:///mock/worktrees/loop-1/README.md',
      Files: '',
    })
    expect(pathsFromOsDataTransfer(dt)).toEqual([
      '/mock/worktrees/loop-1/src/main.go',
      '/mock/worktrees/loop-1/README.md',
    ])
  })

  test('classifyOsDrop chips workspace files and attaches outside images', () => {
    const img = new File(['x'], 'shot.png', { type: 'image/png' })
    const got = classifyOsDrop({
      paths: [
        '/mock/wt/src/main.go',
        '/Users/me/Desktop/shot.png',
        '/Users/me/notes.txt',
      ],
      images: [img],
      rootPath: '/mock/wt',
    })
    expect(got.mentions).toEqual(['src/main.go', '/Users/me/notes.txt'])
    expect(got.images.map((f) => f.name)).toEqual(['shot.png'])
    expect(got.imagePaths).toEqual([])
  })

  test('classifyOsDrop does not also attach a workspace image that was chipped', () => {
    const img = new File(['x'], 'icon.png', { type: 'image/png' })
    const got = classifyOsDrop({
      paths: ['/mock/wt/assets/icon.png'],
      images: [img],
      rootPath: '/mock/wt',
    })
    expect(got.mentions).toEqual(['assets/icon.png'])
    expect(got.images).toEqual([])
    expect(got.imagePaths).toEqual([])
  })

  test('path-only outside image (Wails Finder drop) is an attachment path', () => {
    const got = classifyOsDrop({
      paths: ['/Users/me/Desktop/shot.png', '/Users/me/notes.txt'],
      images: [],
      rootPath: '/mock/wt',
    })
    expect(got.mentions).toEqual(['/Users/me/notes.txt'])
    expect(got.images).toEqual([])
    expect(got.imagePaths).toEqual(['/Users/me/Desktop/shot.png'])
  })

  test('classifyOsDrop skips mention paths with spaces', () => {
    const got = classifyOsDrop({
      paths: ['/Users/me/Documents/My Report.csv', '/Users/me/notes.txt'],
      images: [],
      rootPath: '/mock/wt',
    })
    expect(got.mentions).toEqual(['/Users/me/notes.txt'])
    expect(got.skipped).toEqual(['/Users/me/Documents/My Report.csv'])
  })

  test('path-only workspace image stays an @file chip', () => {
    const got = classifyOsDrop({
      paths: ['/mock/wt/assets/icon.png'],
      images: [],
      rootPath: '/mock/wt',
    })
    expect(got.mentions).toEqual(['assets/icon.png'])
    expect(got.imagePaths).toEqual([])
  })

  test('encodeChatDropId replaces the tab in chat-tab keys', () => {
    expect(encodeChatDropId('ws-1\tPA')).toBe('ws-1:PA')
    expect(encodeChatDropId('ws-1:PA')).toBe('ws-1:PA')
  })

  test('filesFromWailsDrop reads Go emit payload and drop-target id', () => {
    expect(filesFromWailsDrop({
      files: ['/Users/me/shot.png', '/tmp/a.ts'],
      attributes: { 'data-chat-drop': 'ws-1:PA' },
    })).toEqual({ files: ['/Users/me/shot.png', '/tmp/a.ts'], dropId: 'ws-1:PA' })
    expect(filesFromWailsDrop({ files: [] }).files).toEqual([])
    expect(filesFromWailsDrop(null).files).toEqual([])
  })

  test('isImagePath', () => {
    expect(isImagePath('a/b/c.PNG')).toBe(true)
    expect(isImagePath('src/main.go')).toBe(false)
  })

  test('isChatImagePath is the paste MIME set', () => {
    expect(isChatImagePath('/Users/me/Desktop/shot.png')).toBe(true)
    expect(isChatImagePath('photo.JPEG')).toBe(true)
    expect(isChatImagePath('icon.ico')).toBe(false)
    expect(isChatImagePath('logo.svg')).toBe(false)
  })

  test('isAbsFilesystemPath', () => {
    expect(isAbsFilesystemPath('/Users/josh/Downloads/export-111.csv')).toBe(true)
    expect(isAbsFilesystemPath('src/main.go')).toBe(false)
    expect(isAbsFilesystemPath('scratch://1-notes.txt')).toBe(false)
    expect(isAbsFilesystemPath('C:/Users/me/a.csv')).toBe(true)
  })

  test('isChipableFilePath rejects spaces', () => {
    expect(isChipableFilePath('src/main.go')).toBe(true)
    expect(isChipableFilePath('/Users/me/notes.txt')).toBe(true)
    expect(isChipableFilePath('/Users/me/My Report.csv')).toBe(false)
    expect(isChipableFilePath('foo@bar.ts')).toBe(false)
  })
})

describe('terminalDropText', () => {
  test('leaves safe relative paths unquoted', () => {
    expect(quoteForShell('src/main.go')).toBe('src/main.go')
    expect(terminalDropText(['src/main.go', 'README.md'])).toBe('src/main.go README.md')
  })

  test('resolves explorer paths against the workspace root', () => {
    expect(resolveExplorerDropPath('src/main.go', '/tmp/wt')).toBe('/tmp/wt/src/main.go')
    expect(resolveExplorerDropPath('src/', '/tmp/wt')).toBe('/tmp/wt/src')
    expect(terminalDropText(['src/main.go', 'src/'], '/tmp/wt')).toBe('/tmp/wt/src/main.go /tmp/wt/src')
  })

  test('single-quotes spaces, glob, and dollar', () => {
    expect(quoteForShell('My File.ts')).toBe("'My File.ts'")
    expect(quoteForShell("it's.ts")).toBe("'it'\\''s.ts'")
    expect(quoteForShell('foo$.sh')).toBe("'foo$.sh'")
    expect(quoteForShell('*.go')).toBe("'*.go'")
  })
})
