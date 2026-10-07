import { describe, expect, it } from 'vitest'
import {
  applySymbolTransform,
  bestSymbolRule,
  collectSymbols,
  filterSymbols,
  inferSymbolKind,
  qualifyGoReceiver,
  symbolAtLine,
  symbolLookupName,
  type SymbolRule,
} from './tm-symbols'
import { compileSelector } from './tm-scope-selector'

const funcRule: SymbolRule = {
  id: 'func',
  scope: 'entity.name.function',
  show: true,
  transform: '',
  kind: 'func',
}
const hideSupport: SymbolRule = {
  id: 'hide-support',
  scope: 'entity.name.function.support',
  show: false,
  transform: '',
  kind: '',
}
const idRule: SymbolRule = {
  id: 'html-id',
  scope: 'meta.attribute.id.html > string',
  show: true,
  transform: 's/^["\']|["\']$//g\ns/^/ID: /',
  kind: 'id',
}

describe('applySymbolTransform', () => {
  it('applies sequential s/// replacements', () => {
    expect(applySymbolTransform('"main"', idRule.transform)).toBe('ID: main')
    expect(applySymbolTransform('func Foo', 's/^\\s*func\\s+/ /')).toBe(' Foo')
    expect(applySymbolTransform('keep', '')).toBe('keep')
    expect(applySymbolTransform('## Layout', 's/^##\\s+/\\u2003/')).toBe('\u2003Layout')
  })
})

describe('inferSymbolKind', () => {
  it('uses explicit kind, else infers from the selector', () => {
    expect(inferSymbolKind(funcRule)).toBe('func')
    expect(inferSymbolKind({ ...funcRule, kind: '', scope: 'entity.name.section' })).toBe('heading')
    expect(inferSymbolKind({ ...funcRule, kind: '', scope: 'entity.name.tag.yaml' })).toBe('key')
  })
})

describe('bestSymbolRule', () => {
  it('prefers a more specific exclude', () => {
    const rules = [funcRule, hideSupport].map((r) => ({ ...r, sel: compileSelector(r.scope) }))
    const support = ['source.go', 'entity.name.function.support.go']
    const def = ['source.go', 'entity.name.function.go']
    expect(bestSymbolRule(def, rules)?.id).toBe('func')
    expect(bestSymbolRule(support, rules)?.id).toBe('hide-support')
    expect(bestSymbolRule(support, rules)?.show).toBe(false)
  })
})

describe('collectSymbols', () => {
  it('emits function names and skips support calls', () => {
    const lines = ['func Foo()', '    len(x)']
    const byLine: Record<number, { start: number; end: number; scopes: string[] }[]> = {
      1: [{ start: 5, end: 8, scopes: ['source.go', 'entity.name.function.go'] }],
      2: [{ start: 4, end: 7, scopes: ['source.go', 'entity.name.function.support.go'] }],
    }
    const got = collectSymbols(lines, (n) => byLine[n], [funcRule, hideSupport])
    expect(got).toEqual([{ name: 'Foo', kind: 'func', line: 1 }])
  })

  it('prefixes Go methods with the receiver type', () => {
    const line = 'func (e *Engine) PendingAskUsers() bool {'
    const start = line.indexOf('PendingAskUsers')
    const got = collectSymbols(
      [line],
      () => [{ start, end: start + 'PendingAskUsers'.length, scopes: ['source.go', 'entity.name.function.go'] }],
      [funcRule],
    )
    expect(got).toEqual([{ name: 'Engine.PendingAskUsers', kind: 'func', line: 1 }])
  })

  it('joins consecutive tokens and applies transform', () => {
    const lines = ['id="main"']
    const tokens = [
      { start: 3, end: 4, scopes: ['text.html.basic', 'meta.attribute.id.html', 'string.quoted.double.html'] },
      { start: 4, end: 8, scopes: ['text.html.basic', 'meta.attribute.id.html', 'string.quoted.double.html'] },
      { start: 8, end: 9, scopes: ['text.html.basic', 'meta.attribute.id.html', 'string.quoted.double.html'] },
    ]
    const got = collectSymbols(lines, (n) => (n === 1 ? tokens : undefined), [idRule])
    expect(got).toEqual([{ name: 'ID: main', kind: 'id', line: 1 }])
  })

  it('indents markdown headings by hash depth', () => {
    const heading: SymbolRule = {
      id: 'md-h',
      scope: 'markup.heading.markdown',
      show: true,
      transform: [
        's/^\\s{0,3}###\\s+/\\u2003\\u2003/',
        's/^\\s{0,3}##\\s+/\\u2003/',
        's/^\\s{0,3}#\\s+//',
      ].join('\n'),
      kind: 'heading',
    }
    const lines = ['# What this is', '## Layout', '### Nested']
    const byLine: Record<number, { start: number; end: number; scopes: string[] }[]> = {
      1: [
        { start: 0, end: 1, scopes: ['text.html.markdown', 'markup.heading.markdown', 'punctuation.definition.heading.markdown'] },
        { start: 1, end: 14, scopes: ['text.html.markdown', 'markup.heading.markdown', 'entity.name.section.markdown'] },
      ],
      2: [
        { start: 0, end: 2, scopes: ['text.html.markdown', 'markup.heading.markdown', 'punctuation.definition.heading.markdown'] },
        { start: 2, end: 9, scopes: ['text.html.markdown', 'markup.heading.markdown', 'entity.name.section.markdown'] },
      ],
      3: [
        { start: 0, end: 3, scopes: ['text.html.markdown', 'markup.heading.markdown', 'punctuation.definition.heading.markdown'] },
        { start: 3, end: 10, scopes: ['text.html.markdown', 'markup.heading.markdown', 'entity.name.section.markdown'] },
      ],
    }
    const got = collectSymbols(lines, (n) => byLine[n], [heading])
    expect(got).toEqual([
      { name: 'What this is', kind: 'heading', line: 1 },
      { name: '\u2003Layout', kind: 'heading', line: 2 },
      { name: '\u2003\u2003Nested', kind: 'heading', line: 3 },
    ])
  })

  it('skips symbols inside fenced code', () => {
    const func: SymbolRule = {
      id: 'func',
      scope: 'entity.name.function',
      show: true,
      transform: '',
      kind: 'func',
    }
    const heading: SymbolRule = {
      id: 'md-h',
      scope: 'markup.heading.markdown',
      show: true,
      transform: 's/^\\s{0,3}#\\s+//',
      kind: 'heading',
    }
    const lines = ['# Intro', 'func Foo()']
    const byLine: Record<number, { start: number; end: number; scopes: string[] }[]> = {
      1: [
        { start: 0, end: 1, scopes: ['text.html.markdown', 'markup.heading.markdown', 'punctuation.definition.heading.markdown'] },
        { start: 1, end: 7, scopes: ['text.html.markdown', 'markup.heading.markdown', 'entity.name.section.markdown'] },
      ],
      2: [
        {
          start: 5,
          end: 8,
          scopes: [
            'text.html.markdown',
            'markup.fenced_code.block.markdown',
            'meta.embedded.block.go',
            'source.go',
            'entity.name.function.go',
          ],
        },
      ],
    }
    const got = collectSymbols(lines, (n) => byLine[n], [func, heading])
    expect(got).toEqual([{ name: 'Intro', kind: 'heading', line: 1 }])
  })
})

describe('symbolAtLine', () => {
  const foo = { name: 'Foo', kind: 'func', line: 3 }
  const bar = { name: 'Bar', kind: 'func', line: 20 }
  const baz = { name: 'Baz', kind: 'func', line: 20 }

  it('returns the last symbol at or before the caret line', () => {
    expect(symbolAtLine([foo, bar], 1)).toBeNull()
    expect(symbolAtLine([foo, bar], 3)).toEqual(foo)
    expect(symbolAtLine([foo, bar], 10)).toEqual(foo)
    expect(symbolAtLine([foo, bar], 20)).toEqual(bar)
    expect(symbolAtLine([foo, bar], 99)).toEqual(bar)
  })

  it('prefers the later symbol when several share a line', () => {
    expect(symbolAtLine([foo, bar, baz], 20)).toEqual(baz)
  })
})

describe('filterSymbols', () => {
  const em = '\u2003'
  const syms = [
    { name: 'AGENTS.md', kind: 'heading', line: 1 },
    { name: 'What this is', kind: 'heading', line: 5 },
    { name: `${em}Layout`, kind: 'heading', line: 15 },
    { name: `${em}Dev environment tips`, kind: 'heading', line: 27 },
    { name: `${em}${em}Cursor SDK pin`, kind: 'heading', line: 53 },
  ]

  it('keeps document order when the query is empty', () => {
    expect(filterSymbols(syms, '').map((h) => h.item.line)).toEqual([1, 5, 15, 27, 53])
    expect(filterSymbols(syms, '', 2).map((h) => h.item.line)).toEqual([1, 5])
  })

  it('tie-breaks equal scores by line, not leading indent', () => {
    const twins = [
      { name: `${em}${em}Foo`, kind: 'heading', line: 53 },
      { name: `${em}Foo`, kind: 'heading', line: 15 },
    ]
    expect(filterSymbols(twins, 'foo').map((h) => h.item.line)).toEqual([15, 53])
  })
})

describe('qualifyGoReceiver', () => {
  it('prefixes pointer and value receivers, leaves plain funcs alone', () => {
    expect(qualifyGoReceiver('func (e *Engine) Foo() {', 'Foo')).toBe('Engine.Foo')
    expect(qualifyGoReceiver('func (b askUserBroker) park() {', 'park')).toBe('askUserBroker.park')
    expect(qualifyGoReceiver('func (s *Set[T]) Add(v T) {', 'Add')).toBe('Set.Add')
    expect(qualifyGoReceiver('func (c *http.Client) Do() {', 'Do')).toBe('http.Client.Do')
    expect(qualifyGoReceiver('func Foo() {', 'Foo')).toBe('Foo')
    expect(qualifyGoReceiver('def Foo(self):', 'Foo')).toBe('Foo')
  })
})

describe('symbolLookupName', () => {
  it('strips a Go receiver prefix only for funcs', () => {
    expect(symbolLookupName('Engine.Foo', 'func')).toBe('Foo')
    expect(symbolLookupName('http.Client.Do', 'func')).toBe('Do')
    expect(symbolLookupName('foo.bar', 'key')).toBe('foo.bar')
    expect(symbolLookupName('Foo', 'func')).toBe('Foo')
  })
})
