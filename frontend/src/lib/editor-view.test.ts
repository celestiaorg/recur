import { describe, expect, test } from 'vitest'
import {
  editorViewModes, isBinaryImagePath, isPdfPath, mergeViewCurrent, mergeViewReady, nextEditorViewMode,
  scmGutterBase, shouldHydrateRestoredCurrent,
} from './editor-view'

describe('scmGutterBase', () => {
  test('stays empty until FileAtBase has been cached', () => {
    expect(scmGutterBase(null, 'u')).toBeNull()
    expect(scmGutterBase(null, '')).toBeNull()
  })

  test('does not paint a gitignored file as a full-file add', () => {
    expect(scmGutterBase('', '')).toBeNull()
    expect(scmGutterBase('', 'm')).toBeNull()
    expect(scmGutterBase('', 'd')).toBeNull()
  })

  test('empty base is a full-file add for untracked or added paths', () => {
    expect(scmGutterBase('', 'u')).toBe('')
    expect(scmGutterBase('', 'a')).toBe('')
  })

  test('passes through a real base revision', () => {
    expect(scmGutterBase('package main\n', '')).toBe('package main\n')
    expect(scmGutterBase('package main\n', 'm')).toBe('package main\n')
  })
})

describe('editorViewModes', () => {
  test('code + diff for a changed file', () => {
    expect(editorViewModes({
      hasFile: true, deleted: false, canShowDiff: true, hasPreview: false,
    })).toEqual(['code', 'diff'])
  })

  test('code + preview for a dual-nature file with no git status', () => {
    expect(editorViewModes({
      hasFile: true, deleted: false, canShowDiff: false, hasPreview: true,
    })).toEqual(['code', 'preview'])
  })

  test('code + diff + preview when both extras apply', () => {
    expect(editorViewModes({
      hasFile: true, deleted: false, canShowDiff: true, hasPreview: true,
    })).toEqual(['code', 'diff', 'preview'])
  })

  test('deleted file only offers Diff', () => {
    expect(editorViewModes({
      hasFile: false, deleted: true, canShowDiff: true, hasPreview: true,
    })).toEqual(['diff'])
  })

  test('plain unchanged file is code-only', () => {
    expect(editorViewModes({
      hasFile: true, deleted: false, canShowDiff: false, hasPreview: false,
    })).toEqual(['code'])
  })

  test('binary images offer Preview + Diff, never Code', () => {
    expect(editorViewModes({
      hasFile: true, deleted: false, canShowDiff: false, hasPreview: true, isImage: true,
    })).toEqual(['preview'])
    expect(editorViewModes({
      hasFile: true, deleted: false, canShowDiff: true, hasPreview: true, isImage: true,
    })).toEqual(['preview', 'diff'])
    expect(editorViewModes({
      hasFile: false, deleted: true, canShowDiff: true, hasPreview: true, isImage: true,
    })).toEqual(['diff'])
  })

  test('pdfs are preview-only; no code, no diff', () => {
    expect(editorViewModes({
      hasFile: true, deleted: false, canShowDiff: false, hasPreview: true, isImage: true,
    })).toEqual(['preview'])
    expect(editorViewModes({
      hasFile: false, deleted: true, canShowDiff: false, hasPreview: false, isImage: true,
    })).toEqual([])
  })
})

describe('isBinaryImagePath', () => {
  test('matches observatory raster types, not SVG', () => {
    expect(isBinaryImagePath('assets/icon.png')).toBe(true)
    expect(isBinaryImagePath('photo.JPEG')).toBe(true)
    expect(isBinaryImagePath('scratch://artifact-1.webp')).toBe(true)
    expect(isBinaryImagePath('logo.svg')).toBe(false)
    expect(isBinaryImagePath('src/main.go')).toBe(false)
    expect(isBinaryImagePath('docs/guide.pdf')).toBe(false)
  })
})

describe('isPdfPath', () => {
  test('matches .pdf basename, case-insensitive', () => {
    expect(isPdfPath('docs/guide.pdf')).toBe(true)
    expect(isPdfPath('Report.PDF')).toBe(true)
    expect(isPdfPath('scratch://artifact-1.pdf')).toBe(true)
    expect(isPdfPath('src/main.go')).toBe(false)
    expect(isPdfPath('assets/icon.png')).toBe(false)
    expect(isPdfPath('notes.pdf.bak')).toBe(false)
  })
})

describe('nextEditorViewMode', () => {
  test('cycles code ↔ diff', () => {
    expect(nextEditorViewMode('diff', ['code', 'diff'])).toBe('code')
    expect(nextEditorViewMode('code', ['code', 'diff'])).toBe('diff')
  })

  test('cycles code → diff → preview → code', () => {
    const modes = ['code', 'diff', 'preview'] as const
    expect(nextEditorViewMode('code', modes)).toBe('diff')
    expect(nextEditorViewMode('diff', modes)).toBe('preview')
    expect(nextEditorViewMode('preview', modes)).toBe('code')
  })

  test('returns null when there is nothing to toggle', () => {
    expect(nextEditorViewMode('code', ['code'])).toBeNull()
    expect(nextEditorViewMode('diff', ['diff'])).toBeNull()
    expect(nextEditorViewMode('code', [])).toBeNull()
  })

  test('falls back to the first available mode when current is missing', () => {
    expect(nextEditorViewMode('preview', ['code', 'diff'])).toBe('code')
  })
})

describe('mergeViewReady', () => {
  const path = 'src/main.go'

  test('false until the base document is this path', () => {
    expect(mergeViewReady({
      selectedPath: path, baseDocPath: '', canShowDiff: true, editorPath: path,
    })).toBe(false)
    expect(mergeViewReady({
      selectedPath: path, baseDocPath: 'other.go', canShowDiff: true, editorPath: path,
    })).toBe(false)
  })

  test('false while the current buffer is still a previous file', () => {
    expect(mergeViewReady({
      selectedPath: path, baseDocPath: path, canShowDiff: true, editorPath: '',
    })).toBe(false)
    expect(mergeViewReady({
      selectedPath: path, baseDocPath: path, canShowDiff: true, editorPath: 'prev.go',
    })).toBe(false)
  })

  test('true once base and current both belong to the selected path', () => {
    expect(mergeViewReady({
      selectedPath: path, baseDocPath: path, canShowDiff: true, editorPath: path,
    })).toBe(true)
  })

  test('deleted files are ready without a current buffer', () => {
    expect(mergeViewReady({
      selectedPath: path, baseDocPath: path, canShowDiff: true, missingCurrent: true, editorPath: '',
    })).toBe(true)
  })

  test('submodule directories are ready without a current buffer', () => {
    expect(mergeViewReady({
      selectedPath: path, baseDocPath: path, canShowDiff: true, missingCurrent: true, editorPath: 'prev.go',
    })).toBe(true)
  })

  test('false when the path is not diffable', () => {
    expect(mergeViewReady({
      selectedPath: path, baseDocPath: path, canShowDiff: false, editorPath: path,
    })).toBe(false)
  })
})

describe('mergeViewCurrent', () => {
  test('uses the draft only after this path is hydrated', () => {
    expect(mergeViewCurrent({
      editorPath: '', selectedPath: 'a.go', draft: 'stale',
    })).toBe('')
    expect(mergeViewCurrent({
      editorPath: 'b.go', selectedPath: 'a.go', draft: 'stale',
    })).toBe('')
    expect(mergeViewCurrent({
      editorPath: 'a.go', selectedPath: 'a.go', draft: 'current',
    })).toBe('current')
  })

  test('deleted files contribute an empty current side', () => {
    expect(mergeViewCurrent({
      missingCurrent: true, editorPath: '', selectedPath: 'gone.go', draft: 'stale',
    })).toBe('')
  })
})

describe('shouldHydrateRestoredCurrent', () => {
  const path = 'src/gone.go'
  const base = {
    selectedPath: path, editorPath: '',
    prevStatus: 'd', nextStatus: 'm',
  }

  test('true when a deleted tab becomes a real file', () => {
    expect(shouldHydrateRestoredCurrent(base)).toBe(true)
  })

  test('false while still deleted, or when the current buffer is already this path', () => {
    expect(shouldHydrateRestoredCurrent({ ...base, nextStatus: 'd' })).toBe(false)
    expect(shouldHydrateRestoredCurrent({ ...base, nextStatus: '' })).toBe(false)
    expect(shouldHydrateRestoredCurrent({ ...base, editorPath: path })).toBe(false)
  })

  test('false when hydrate already failed or a load is in flight', () => {
    expect(shouldHydrateRestoredCurrent({ ...base, hydrateFailedPath: path })).toBe(false)
    expect(shouldHydrateRestoredCurrent({ ...base, fileLoading: true })).toBe(false)
    expect(shouldHydrateRestoredCurrent({ ...base, selectedIsDir: true })).toBe(false)
  })
})
