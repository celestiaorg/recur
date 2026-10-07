import { describe, expect, test } from 'vitest'
import { lspLanguageIdForPath } from './lsp-lang'
import { addGrammar, _resetTmRegistryForTests } from './tm-registry'

describe('lspLanguageIdForPath', () => {
  test('opens JSX/TSX as react language ids so tsserver parses JSX', () => {
    expect(lspLanguageIdForPath('src/App.tsx')).toBe('typescriptreact')
    expect(lspLanguageIdForPath('src/App.jsx')).toBe('javascriptreact')
    expect(lspLanguageIdForPath('src/util.ts')).toBe('typescript')
    expect(lspLanguageIdForPath('src/util.js')).toBe('javascript')
  })

  test('opens Go module files with gopls language ids, not go', () => {
    expect(lspLanguageIdForPath('go.mod')).toBe('go.mod')
    expect(lspLanguageIdForPath('go.work')).toBe('go.work')
    expect(lspLanguageIdForPath('go.sum')).toBe('go.sum')
    expect(lspLanguageIdForPath('main.go')).toBe('go')
  })

  test('opens comment-allowing JSON as jsonc', () => {
    expect(lspLanguageIdForPath('tsconfig.json')).toBe('jsonc')
    expect(lspLanguageIdForPath('tsconfig.app.json')).toBe('jsonc')
    expect(lspLanguageIdForPath('tsconfig-base.json')).toBe('jsonc')
    expect(lspLanguageIdForPath('jsconfig.json')).toBe('jsonc')
    expect(lspLanguageIdForPath('.eslintrc.json')).toBe('jsonc')
    expect(lspLanguageIdForPath('.vscode/settings.json')).toBe('jsonc')
    expect(lspLanguageIdForPath('src/notes.jsonc')).toBe('jsonc')
    expect(lspLanguageIdForPath('package.json')).toBe('json')
  })

  test('does not send non-bash files to bash-language-server', () => {
    expect(lspLanguageIdForPath('Dockerfile')).toBe('dockerfile')
    expect(lspLanguageIdForPath('Makefile')).toBe('makefile')
    expect(lspLanguageIdForPath('script.ps1')).toBe('powershell')
    expect(lspLanguageIdForPath('setup.fish')).toBe('fish')
    expect(lspLanguageIdForPath('bin/run.sh')).toBe('shellscript')
  })

  test('does not send indented Sass or JSON5 as a neighboring dialect', () => {
    expect(lspLanguageIdForPath('theme.scss')).toBe('scss')
    expect(lspLanguageIdForPath('theme.sass')).toBe('sass')
    expect(lspLanguageIdForPath('config.json5')).toBe('json5')
  })

  test('uses [[grammar]].language when the prefs map does not know the file', () => {
    _resetTmRegistryForTests()
    addGrammar(
      {
        id: 'recur.languages.solidity',
        scopeName: 'source.solidity',
        path: 'syntaxes/solidity.tmLanguage.json',
        language: 'solidity',
        fileTypes: ['sol'],
        bundleName: 'Languages',
        sourceRoot: 'default',
      },
      async () => '{}',
    )
    expect(lspLanguageIdForPath('contracts/Token.sol')).toBe('solidity')
    expect(lspLanguageIdForPath('test/Token.t.sol')).toBe('solidity')
    _resetTmRegistryForTests()
    expect(lspLanguageIdForPath('contracts/Token.sol')).toBe('')
  })
})
