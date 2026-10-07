/** @vitest-environment happy-dom */
import { describe, expect, it } from 'vitest'
import { snippet } from '@codemirror/autocomplete'
import { EditorState } from '@codemirror/state'
import { EditorView } from '@codemirror/view'
import {
  applySnippet,
  commandSnippetRange,
  isoDate,
  snippetNeedsShield,
  snippetsMatchingPrefix,
  snippetTriggerHint,
  substituteSnippetVars,
  toCodeMirrorSnippet,
  triggerWordBefore,
} from './snippets'

describe('snippetsMatchingPrefix', () => {
  const goIfe = {
    id: 'recur.go.ife',
    label: 'If Err',
    tabTrigger: 'ife',
    body: 'x',
    order: 10,
  }
  const goIf = {
    id: 'recur.go.if',
    label: 'If',
    tabTrigger: 'if',
    body: 'x',
    order: 11,
  }
  const wrap = {
    id: 'recur.html.wrap',
    label: 'Wrap',
    tabTrigger: '',
    body: 'x',
    order: 1,
  }

  it('prefix-matches tab triggers and skips keyEquivalent-only snippets', () => {
    expect(snippetsMatchingPrefix([goIfe, goIf, wrap], 'if', false).map((s) => s.id)).toEqual([
      'recur.go.ife',
      'recur.go.if',
    ])
    expect(snippetsMatchingPrefix([goIfe, goIf, wrap], 'ife', false).map((s) => s.id)).toEqual([
      'recur.go.ife',
    ])
    expect(snippetsMatchingPrefix([goIfe, wrap], 'nope', false)).toEqual([])
  })

  it('lists every tab-trigger snippet on an explicit empty prefix', () => {
    expect(snippetsMatchingPrefix([goIfe, wrap], '', false)).toEqual([])
    expect(snippetsMatchingPrefix([goIfe, wrap], '', true).map((s) => s.id)).toEqual(['recur.go.ife'])
  })
})

describe('snippetTriggerHint', () => {
  it('appends the Tab glyph', () => {
    expect(snippetTriggerHint('ife')).toBe('ife⇥')
    expect(snippetTriggerHint('')).toBe('')
  })
})

describe('triggerWordBefore', () => {
  it('takes the word immediately before the cursor', () => {
    expect(triggerWordBefore('ife', 3)).toEqual({ from: 0, to: 3, word: 'ife' })
    expect(triggerWordBefore('  fori', 6)).toEqual({ from: 2, to: 6, word: 'fori' })
    expect(triggerWordBefore('x ife', 5)).toEqual({ from: 2, to: 5, word: 'ife' })
  })

  it('rejects non-word triggers', () => {
    expect(triggerWordBefore('  ', 2)).toBeNull()
    expect(triggerWordBefore('1ife', 4)).toBeNull()
  })
})

describe('substituteSnippetVars', () => {
  it('wraps selection without escaping (applySnippet shields $ / ${ / braces)', () => {
    const body = '<${1:p}>${2:$TM_SELECTED_TEXT}</$1>'
    expect(substituteSnippetVars(body, 'hi $x')).toBe('<${1:p}>${2:hi $x}</$1>')
    expect(substituteSnippetVars(body, 'C:\\path')).toBe('<${1:p}>${2:C:\\path}</$1>')
  })

  it('fills $ISO_DATE as YYYY-MM-DD', () => {
    const d = new Date(2026, 7, 17)
    expect(isoDate(d)).toBe('2026-08-17')
    expect(substituteSnippetVars('$ISO_DATE', '', d)).toBe('2026-08-17')
  })
})

describe('toCodeMirrorSnippet', () => {
  it('rewrites $0 / $1 to ${n} and copies defaults onto mirrors', () => {
    expect(toCodeMirrorSnippet('console.log(${1})$0')).toBe('console.log(${1})${0}')
    expect(toCodeMirrorSnippet('<${1:div}>x</$1>')).toBe('<${1:div}>x</${1:div}>')
    expect(toCodeMirrorSnippet('for ${1:i} := 0; $1 < ${2:n}; $1++')).toBe(
      'for ${1:i} := 0; ${1:i} < ${2:n}; ${1:i}++',
    )
    expect(toCodeMirrorSnippet('$0t.Fatal')).toBe('${0}t.Fatal')
  })
})

describe('commandSnippetRange', () => {
  it('replaces the current word when fallbackInput is word', () => {
    expect(commandSnippetRange(3, 3, 'word', { from: 0, to: 3 }, null, 10)).toEqual({ from: 0, to: 3 })
  })

  it('keeps a selection and otherwise inserts at the caret', () => {
    expect(commandSnippetRange(1, 4, 'word', { from: 0, to: 8 }, null, 10)).toEqual({ from: 1, to: 4 })
    expect(commandSnippetRange(3, 3, 'word', null, null, 10)).toEqual({ from: 3, to: 3 })
  })
})

describe('snippetNeedsShield', () => {
  it('flags $n and ${…} which CodeMirror would parse as fields', () => {
    expect(snippetNeedsShield('$HOME')).toBe(false)
    expect(snippetNeedsShield('C:\\path')).toBe(false)
    expect(snippetNeedsShield('$1')).toBe(true)
    expect(snippetNeedsShield('${count}')).toBe(true)
  })
})

function expandDoc(body: string, selectedText = ''): { doc: string; sels: string[] } {
  const prepared = toCodeMirrorSnippet(substituteSnippetVars(body, selectedText))
  const view = new EditorView({
    state: EditorState.create({
      doc: 'X',
      extensions: [EditorState.allowMultipleSelections.of(true)],
    }),
  })
  snippet(prepared)(view, null as never, 0, 1)
  const doc = view.state.doc.toString()
  const sels = view.state.selection.ranges.map((r) => view.state.sliceDoc(r.from, r.to))
  view.destroy()
  return { doc, sels }
}

describe('ife-style mirrors', () => {
  it('selects every err so typing replaces the log.Fatal argument too', () => {
    const { doc, sels } = expandDoc('if ${1:err} != nil {\n\t${2:log.Fatal(${1:err})}\n}')
    expect(sels).toEqual(['err', 'err'])
    expect(doc).toMatch(/^if err != nil \{\n\s+log\.Fatal\(err\)\n\}$/)
  })

  it('keeps func placeholder defaults visible', () => {
    const { doc, sels } = expandDoc('func ${1:name}(${2:params}) ${3:type} {\n\t${0:// body}\n}')
    expect(sels[0]).toBe('name')
    expect(doc).toMatch(/func name\(params\) type \{/)
    expect(doc).toMatch(/\/\/ body/)
  })
})

describe('TextMate default expansion', () => {
  it('matches Go ife / fmt / fori / meth / st / imp', () => {
    expect(expandDoc('if ${1:err} != nil {\n\t${2:log.Fatal(${1:err})}\n}').doc).toMatch(
      /if err != nil \{\n\s+log\.Fatal\(err\)\n\}/,
    )
    expect(expandDoc('fmt.Println(${1:"${2:Hello World!}"})').doc).toBe('fmt.Println("Hello World!")')
    expect(expandDoc('for ${2:i} := 0; $2 < ${1:count}; ${3:i++} {\n\t${0:// body}\n}').doc).toMatch(
      /for i := 0; i < count; i\+\+ \{\n\s+\/\/ body\n\}/,
    )
    expect(expandDoc('func (${1:receiver} ${2:type}) ${3:name}(${4:params}) ${5:type} {\n\t${0:// body}\n}').doc).toMatch(
      /func \(receiver type\) name\(params\) type \{/,
    )
    expect(expandDoc('type ${1:Foo} struct {\n\t${2:Bar} ${0:string}\n}').doc).toMatch(
      /type Foo struct \{\n\s+Bar string\n\}/,
    )
    expect(expandDoc('import ${2:package} "${1:package}"').doc).toBe('import package "package"')
  })

  it('matches JS log / ife / try and Python def / ifmain', () => {
    expect(expandDoc('console.log(${1:$TM_SELECTED_TEXT});', 'hi').doc).toBe('console.log(hi);')
    expect(expandDoc('if (${1:true}) {\n\t${2:$TM_SELECTED_TEXT}\n} else {\n\t$3\n}').doc).toMatch(
      /if \(true\) \{\n\s+\n\} else \{\n\s+\n\}/,
    )
    expect(expandDoc('try {\n\t$1\n} catch (${2:e}) {\n\t$3\n} finally {\n\t$4\n}').doc).toMatch(
      /try \{\n\s+\n\} catch \(e\) \{\n\s+\n\} finally \{\n\s+\n\}/,
    )
    expect(expandDoc('def ${1:fname}(${2}):\n\t"""${3:docstring for ${1:fname}}"""\n\t${0:pass}').doc).toMatch(
      /def fname\(\):\n\s+"""docstring for fname"""\n\s+pass/,
    )
    expect(expandDoc('class ${1:ClassName}(${2:object}):\n\t"""${3:docstring for ${1:ClassName}}"""\n\tdef __init__(self, ${4:arg}):\n\t\t${5:super(${1:ClassName}, self).__init__()}\n\t\tself.${4:arg} = ${4:arg}\n\t\t$0').doc).toMatch(
      /class ClassName\(object\):\n\s+"""docstring for ClassName"""\n\s+def __init__\(self, arg\):\n\s+super\(ClassName, self\)\.__init__\(\)\n\s+self\.arg = arg/,
    )
    expect(expandDoc("if __name__ == '__main__':\n\t${1:main()}$0").doc).toMatch(
      /if __name__ == '__main__':\n\s+main\(\)/,
    )
  })

  it('matches Shell if / for / tmpf and HTML wrap / div', () => {
    expect(expandDoc('if ${2:[[ ${1:condition} ]]}; then\n\t${0:#statements}\nfi').doc).toMatch(
      /if \[\[ condition \]\][\s\S]*#statements[\s\S]*fi/,
    )
    expect(expandDoc('for (( i = 0; i < ${1:10}; i++ )); do\n\t${0:#statements}\ndone').doc).toMatch(
      /for \(\( i = 0; i < 10; i\+\+ \)\)/,
    )
    const tmp = expandDoc('${1:TMPFILE}="$(mktemp -t ${2:untitled})"\ntrap \'${3:rm -f "$TMPFILE"}\' EXIT INT HUP TERM')
    expect(tmp.doc).toMatch(/TMPFILE="\$\(mktemp -t untitled\)"/)
    expect(tmp.doc).toMatch(/trap 'rm -f "\$TMPFILE"' EXIT INT HUP TERM/)
    expect(expandDoc('<${1:p}>${2:$TM_SELECTED_TEXT}</$1>', 'hi').doc).toBe('<p>hi</p>')
    expect(expandDoc('<${1:p}>${2:$TM_SELECTED_TEXT}</$1>', '$HOME').doc).toBe('<p>$HOME</p>')
    expect(expandDoc('<${1:p}>${2:$TM_SELECTED_TEXT}</$1>', 'C:\\path').doc).toBe('<p>C:\\path</p>')
    expect(expandDoc('<div${1: id="${2:name}"}>\n\t${0:$TM_SELECTED_TEXT}\n</div>').doc).toMatch(
      /<div id="name">/,
    )
  })
})

describe('applySnippet', () => {
  it('inserts ${…} selection as literal text, not a field', () => {
    const view = new EditorView({
      state: EditorState.create({
        doc: 'X',
        extensions: [EditorState.allowMultipleSelections.of(true)],
      }),
    })
    applySnippet(
      view,
      { id: 't', label: 'Wrap', tabTrigger: '', body: '<${1:p}>${2:$TM_SELECTED_TEXT}</$1>', order: 0 },
      0,
      1,
      '${count}',
    )
    expect(view.state.doc.toString()).toBe('<p>${count}</p>')
    view.destroy()
  })

  it('inserts $1 selection as literal text, not tab stop 1', () => {
    const view = new EditorView({
      state: EditorState.create({
        doc: 'X',
        extensions: [EditorState.allowMultipleSelections.of(true)],
      }),
    })
    applySnippet(
      view,
      { id: 't', label: 'Wrap', tabTrigger: '', body: '"${1:$TM_SELECTED_TEXT}"', order: 0 },
      0,
      1,
      '$1',
    )
    expect(view.state.doc.toString()).toBe('"$1"')
    view.destroy()
  })
})
