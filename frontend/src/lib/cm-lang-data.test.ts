/** @vitest-environment happy-dom */
import { describe, expect, it } from 'vitest'
import { insertNewlineAndIndent, toggleComment } from '@codemirror/commands'
import { getIndentation, indentUnit } from '@codemirror/language'
import { EditorState } from '@codemirror/state'
import { EditorView } from '@codemirror/view'
import { insertBracket } from '@codemirror/autocomplete'
import {
  commentTokensOf,
  compileIndent,
  compileRegex,
  computeIndent,
  closeBracketsOf,
  insidePairedQuote,
  insertNewlineBetweenTags,
  languageDataExtension,
} from './cm-lang-data'

const goFields = {
  lineComment: '//',
  blockCommentStart: '/*',
  blockCommentEnd: '*/',
  increaseIndentPattern: '[{(\\[]\\s*(?://.*)?$|^\\s*(?:case|default)\\b.*$',
  decreaseIndentPattern: '^\\s*(?:[})\\]]|(?:case|default)\\b)',
}

const htmlFields = {
  blockCommentStart: '<!--',
  blockCommentEnd: '-->',
  increaseIndentPattern:
    '<(?!\\/)(?!area\\b|base\\b|br\\b|col\\b|embed\\b|hr\\b|img\\b|input\\b|link\\b|meta\\b|param\\b|source\\b|track\\b|wbr\\b)[A-Za-z][-A-Za-z0-9]*(?:\\s[^>]*)?>\\s*$',
  decreaseIndentPattern: '^\\s*<\\/[A-Za-z][-A-Za-z0-9]*>',
  closeBrackets: ['<'],
}

const yamlFields = {
  lineComment: '#',
  increaseIndentPattern: '^\\s*.*(:|-)\\s?(?:&\\w+)?(?:\\{[^}"\']*|\\([^)"\']*)?$',
  decreaseIndentPattern: '^\\s+\\}$',
  disableIndentCorrections: true,
}

const pythonFields = {
  lineComment: '#',
  increaseIndentPattern:
    '^\\s*(?:class|(?:async\\s+)?(?:def|for|with)|elif|else|except|finally|if|match|case|try|while)\\b.*:\\s*(?:#.*)?$|[{[]\\s*(?:#.*)?$',
  decreaseIndentPattern: '^\\s*(?:elif|else|except|finally)\\b|^\\s*[}\\]]',
  disableIndentCorrections: true,
}

function runToggle(doc: string, fields: Parameters<typeof languageDataExtension>[0]): string {
  const parent = document.createElement('div')
  document.body.appendChild(parent)
  const view = new EditorView({
    parent,
    state: EditorState.create({
      doc,
      extensions: [languageDataExtension(fields)],
    }),
  })
  toggleComment(view)
  const out = view.state.doc.toString()
  view.destroy()
  parent.remove()
  return out
}

describe('commentTokensOf', () => {
  it('maps YAML #, HTML block, and Go line+block', () => {
    expect(commentTokensOf(yamlFields)).toEqual({ line: '#' })
    expect(commentTokensOf(htmlFields)).toEqual({
      block: { open: '<!--', close: '-->' },
    })
    expect(commentTokensOf(goFields)).toEqual({
      line: '//',
      block: { open: '/*', close: '*/' },
    })
    expect(commentTokensOf({ increaseIndentPattern: '{$' })).toBeUndefined()
  })

  it('compiles HTML / YAML / Make indent regexes', () => {
    expect(
      compileRegex(
        '<(?!\\/)(?!area\\b|base\\b|br\\b|col\\b|embed\\b|hr\\b|img\\b|input\\b|link\\b|meta\\b|param\\b|source\\b|track\\b|wbr\\b)[A-Za-z][-A-Za-z0-9]*(?:\\s[^>]*)?>\\s*$',
      ),
    ).not.toBeNull()
    expect(compileIndent(yamlFields)).not.toBeNull()
    expect(compileRegex('^[^\\t ]+:')).not.toBeNull()
  })
})

describe('toggleComment', () => {
  it('comments YAML with #', () => {
    expect(runToggle('key: value', yamlFields)).toBe('# key: value')
  })

  it('comments HTML with <!-- -->', () => {
    expect(runToggle('div', htmlFields)).toBe('<!-- div -->')
  })

  it('comments Go with //', () => {
    expect(runToggle('x := 1', goFields)).toBe('// x := 1')
  })
})

describe('computeIndent', () => {
  const go = compileIndent(goFields)!
  const py = compileIndent(pythonFields)!
  const html = compileIndent(htmlFields)!

  it('indents the next Go line after {', () => {
    expect(
      computeIndent({
        prevText: 'func main() {',
        lineText: '',
        unit: 4,
        tabSize: 4,
        patterns: go,
      }),
    ).toBe(4)
  })

  it('outdents a closing brace', () => {
    expect(
      computeIndent({
        prevText: '\tx := 1',
        lineText: '}',
        unit: 4,
        tabSize: 4,
        patterns: go,
      }),
    ).toBe(0)
  })

  it('keeps a closer aligned with its opener on the previous line', () => {
    expect(
      computeIndent({
        prevText: '\tif a {',
        lineText: '\t\t}',
        unit: 4,
        tabSize: 4,
        patterns: go,
      }),
    ).toBe(4)
  })

  it('indents the next Python line after :', () => {
    expect(
      computeIndent({
        prevText: 'if True:',
        lineText: '',
        unit: 4,
        tabSize: 4,
        patterns: py,
      }),
    ).toBe(4)
  })

  it('does not rewrite a non-empty Python line (disableIndentCorrections)', () => {
    expect(
      computeIndent({
        prevText: 'if True:',
        lineText: '    x = 1',
        unit: 4,
        tabSize: 4,
        patterns: py,
      }),
    ).toBe(4)
  })

  it('indents the next HTML line after an opening tag', () => {
    expect(
      computeIndent({
        prevText: '<div>',
        lineText: '',
        unit: 2,
        tabSize: 2,
        patterns: html,
      }),
    ).toBe(2)
  })

  it('outdents an HTML closing tag', () => {
    expect(
      computeIndent({
        prevText: '  hello',
        lineText: '</div>',
        unit: 2,
        tabSize: 2,
        patterns: html,
      }),
    ).toBe(0)
  })
})

describe('getIndentation', () => {
  it('uses indentService after a Go brace', () => {
    const state = EditorState.create({
      doc: 'func main() {\n',
      extensions: [indentUnit.of('\t'), languageDataExtension(goFields)],
    })
    expect(getIndentation(state, state.doc.line(2).from)).toBe(state.tabSize)
  })

  it('uses indentService after a Python colon', () => {
    const state = EditorState.create({
      doc: 'if True:\n',
      extensions: [indentUnit.of('    '), languageDataExtension(pythonFields)],
    })
    expect(getIndentation(state, state.doc.line(2).from)).toBe(4)
  })

  it('uses indentService after an HTML opening tag', () => {
    const state = EditorState.create({
      doc: '<div>\n',
      extensions: [indentUnit.of('  '), languageDataExtension(htmlFields)],
    })
    expect(getIndentation(state, state.doc.line(2).from)).toBe(2)
  })
})

function runEnter(doc: string, pos: number, fields: Parameters<typeof languageDataExtension>[0]): string {
  const parent = document.createElement('div')
  document.body.appendChild(parent)
  const view = new EditorView({
    parent,
    state: EditorState.create({
      doc,
      selection: { anchor: pos },
      extensions: [indentUnit.of('  '), languageDataExtension(fields)],
    }),
  })
  if (!insertNewlineBetweenTags(view)) insertNewlineAndIndent(view)
  const out = view.state.doc.toString()
  view.destroy()
  parent.remove()
  return out
}

describe('insertNewlineBetweenPairs', () => {
  it('splits an empty HTML tag pair the way TextMate Return-inside-tags does', () => {
    const doc = '<div></div>'
    expect(runEnter(doc, '<div>'.length, htmlFields)).toBe('<div>\n  \n</div>')
  })

  it('keeps the outer indent when the pair is nested', () => {
    const doc = '  <p></p>'
    expect(runEnter(doc, '  <p>'.length, htmlFields)).toBe('  <p>\n    \n  </p>')
  })

  it('does not split a void tag', () => {
    expect(runEnter('<br></br>', '<br>'.length, htmlFields)).toBe('<br>\n</br>')
  })

  it('falls through to indentService after a lone opening tag', () => {
    const doc = '<div>'
    expect(runEnter(doc, doc.length, htmlFields)).toBe('<div>\n  ')
  })

  it('splits empty braces like TextMate Source Return Inside Empty Item', () => {
    expect(runEnter('{}', 1, goFields)).toBe('{\n  \n}')
    expect(runEnter('()', 1, goFields)).toBe('(\n  \n)')
    expect(runEnter('[]', 1, goFields)).toBe('[\n  \n]')
  })

  it('keeps outer indent when splitting nested braces', () => {
    expect(runEnter('  {  }', '  {'.length, goFields)).toBe('  {\n    \n  }')
  })

  it('does not split mismatched brackets', () => {
    expect(runEnter('{]', 1, goFields)).toBe('{\n]')
  })
})

const mdFields = {
  blockCommentStart: '<!--',
  blockCommentEnd: '-->',
  closeBrackets: ['<', '_', '*'],
}

describe('closeBrackets extras', () => {
  it('merges HTML <> onto CodeMirror defaults and backtick', () => {
    expect(closeBracketsOf(htmlFields).brackets).toEqual(['(', '[', '{', "'", '"', '`', '<'])
  })

  it('pairs < in HTML and _/* in Markdown', () => {
    const html = EditorState.create({
      doc: '',
      extensions: [languageDataExtension(htmlFields)],
    })
    expect(insertBracket(html, '<')?.state.doc.toString()).toBe('<>')

    const md = EditorState.create({
      doc: '',
      extensions: [languageDataExtension(mdFields)],
    })
    expect(insertBracket(md, '_')?.state.doc.toString()).toBe('__')
    expect(insertBracket(md, '*')?.state.doc.toString()).toBe('**')
  })

  it('does not pair < in Go', () => {
    const go = EditorState.create({
      doc: '',
      extensions: [languageDataExtension(goFields)],
    })
    expect(insertBracket(go, '<')).toBeNull()
  })
})

describe('insidePairedQuote', () => {
  it('detects an open double quote on the line, ignoring escapes', () => {
    expect(insidePairedQuote('"hello', 6, '"')).toBe(true)
    expect(insidePairedQuote('"hello"', 7, '"')).toBe(false)
    expect(insidePairedQuote('"say \\"hi', 9, '"')).toBe(true)
    expect(insidePairedQuote('foo', 3, '"')).toBe(false)
  })
})

