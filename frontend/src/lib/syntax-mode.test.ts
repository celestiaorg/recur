/** @vitest-environment happy-dom */
import { beforeEach, describe, expect, it } from 'vitest'
import {
  SYNTAX_PLAIN,
  _resetSyntaxModeForTests,
  MAX_SYNTAX_BY_PATH,
  fileTypeAssociationForPath,
  hydrateSyntaxMode,
  setSyntaxForFileType,
  setSyntaxForPath,
  setSyntaxModePersist,
  syntaxFileTypeOverride,
  syntaxMapsNonEmpty,
  syntaxModeSnapshot,
  syntaxOverrideForPath,
  syntaxPathOverride,
} from './syntax-mode'

beforeEach(() => _resetSyntaxModeForTests())

describe('fileTypeAssociationForPath', () => {
  it('uses *.ext for ordinary files', () => {
    expect(fileTypeAssociationForPath('src/main.go')).toEqual({ key: '*.go', label: '*.go' })
    expect(fileTypeAssociationForPath('foo.JSON')).toEqual({ key: '*.json', label: '*.json' })
  })

  it('uses the basename for dotfiles and extensionless names', () => {
    expect(fileTypeAssociationForPath('.env.local')).toEqual({ key: '.env.local', label: '.env.local' })
    expect(fileTypeAssociationForPath('Makefile')).toEqual({ key: 'makefile', label: 'Makefile' })
  })

  it('skips scratch buffers', () => {
    expect(fileTypeAssociationForPath('scratch://1-Scratch')).toBeNull()
    expect(fileTypeAssociationForPath('scratch://1-notes.txt')).toBeNull()
  })
})

describe('syntax overrides', () => {
  it('path override wins over file-type association', () => {
    setSyntaxForFileType('*.go', 'source.python')
    expect(syntaxOverrideForPath('src/main.go')).toBe('source.python')
    setSyntaxForPath('src/main.go', 'source.yaml')
    expect(syntaxOverrideForPath('src/main.go')).toBe('source.yaml')
    expect(syntaxOverrideForPath('lib/other.go')).toBe('source.python')
  })

  it('treats empty string as explicit Plain Text', () => {
    setSyntaxForPath('src/main.go', SYNTAX_PLAIN)
    expect(syntaxOverrideForPath('src/main.go')).toBe(SYNTAX_PLAIN)
    expect(syntaxPathOverride('src/main.go')).toBe(SYNTAX_PLAIN)
  })

  it('Auto Detect deletes the path override', () => {
    setSyntaxForPath('src/main.go', 'source.yaml')
    setSyntaxForPath('src/main.go', null)
    expect(syntaxOverrideForPath('src/main.go')).toBeUndefined()
    expect(syntaxPathOverride('src/main.go')).toBeUndefined()
  })

  it('hydrate replaces maps without calling persist', () => {
    let n = 0
    setSyntaxModePersist(() => { n++ })
    setSyntaxForPath('notes.txt', 'text.html.markdown')
    setSyntaxForFileType('*.conf', 'source.yaml')
    expect(n).toBe(2)

    hydrateSyntaxMode(
      { 'notes.txt': 'text.html.markdown', 'src/main.go': '' },
      { '*.conf': 'source.yaml' },
    )
    expect(n).toBe(2)
    expect(syntaxOverrideForPath('notes.txt')).toBe('text.html.markdown')
    expect(syntaxOverrideForPath('src/main.go')).toBe(SYNTAX_PLAIN)
    expect(syntaxFileTypeOverride('*.conf')).toBe('source.yaml')
    expect(syntaxMapsNonEmpty()).toBe(true)
  })

  it('snapshot is a copy; hydrate empty clears', () => {
    setSyntaxForPath('a.go', 'source.yaml')
    const snap = syntaxModeSnapshot()
    snap.syntaxByPath['a.go'] = 'nope'
    expect(syntaxOverrideForPath('a.go')).toBe('source.yaml')

    hydrateSyntaxMode({}, {})
    expect(syntaxOverrideForPath('a.go')).toBeUndefined()
    expect(syntaxMapsNonEmpty()).toBe(false)
  })

  it('mergeLive keeps picks made after an empty hydrate', () => {
    hydrateSyntaxMode({}, {})
    setSyntaxForPath('src/main.go', 'source.yaml')
    hydrateSyntaxMode({ 'README.md': 'source.yaml' }, { '*.conf': 'source.yaml' }, true)
    expect(syntaxOverrideForPath('src/main.go')).toBe('source.yaml')
    expect(syntaxOverrideForPath('README.md')).toBe('source.yaml')
    expect(syntaxFileTypeOverride('*.conf')).toBe('source.yaml')
  })

  it('snapshot drops scratch paths', () => {
    setSyntaxForPath('scratch://1-Scratch', 'source.yaml')
    setSyntaxForPath('src/main.go', 'source.yaml')
    expect(syntaxModeSnapshot().syntaxByPath).toEqual({ 'src/main.go': 'source.yaml' })
  })

  it('caps per-path overrides, dropping the oldest', () => {
    for (let i = 0; i < MAX_SYNTAX_BY_PATH + 3; i++) {
      setSyntaxForPath(`f${i}.txt`, 'source.yaml')
    }
    expect(syntaxPathOverride('f0.txt')).toBeUndefined()
    expect(syntaxPathOverride(`f${MAX_SYNTAX_BY_PATH + 2}.txt`)).toBe('source.yaml')
    expect(Object.keys(syntaxModeSnapshot().syntaxByPath).length).toBe(MAX_SYNTAX_BY_PATH)
  })
})
