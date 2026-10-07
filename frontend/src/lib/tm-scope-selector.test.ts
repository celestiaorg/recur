import { describe, expect, it } from 'vitest'
import {
  compileSelector,
  selectorMatches,
  selectorScore,
  scopePrefixMatch,
} from './tm-scope-selector'

describe('scopePrefixMatch', () => {
  it('matches exact and dotted prefixes', () => {
    expect(scopePrefixMatch('entity.name.function', 'entity.name.function')).toBe(true)
    expect(scopePrefixMatch('entity.name.function', 'entity.name.function.go')).toBe(true)
    expect(scopePrefixMatch('entity.name.function', 'entity.name.type.go')).toBe(false)
    expect(scopePrefixMatch('string', 'string.quoted.double.html')).toBe(true)
  })
})

describe('selectorMatches', () => {
  const goFn = ['source.go', 'meta.function.go', 'entity.name.function.go']
  const goSupport = ['source.go', 'entity.name.function.support.go']
  const htmlId = [
    'text.html.basic',
    'meta.tag.inline.any.html',
    'meta.attribute.id.html',
    'string.quoted.double.html',
  ]

  it('matches a single atom against the stack', () => {
    expect(selectorMatches('entity.name.function', goFn)).toBe(true)
    expect(selectorMatches('entity.name.type', goFn)).toBe(false)
  })

  it('requires descendant atoms in order', () => {
    expect(selectorMatches('source.go entity.name.function', goFn)).toBe(true)
    expect(selectorMatches('source.python entity.name.function', goFn)).toBe(false)
  })

  it('ORs comma alternatives', () => {
    const sel = compileSelector('source.css meta.selector, source.css meta.at-rule.media')
    expect(selectorMatches(sel, ['source.css', 'meta.selector.css'])).toBe(true)
    expect(selectorMatches(sel, ['source.css', 'meta.at-rule.media.css'])).toBe(true)
    expect(selectorMatches(sel, ['source.go', 'meta.selector.css'])).toBe(false)
  })

  it('excludes with minus', () => {
    expect(selectorMatches('entity.name.function - entity.name.function.support', goFn)).toBe(true)
    expect(selectorMatches('entity.name.function - entity.name.function.support', goSupport)).toBe(false)
  })

  it('requires a child after >', () => {
    expect(selectorMatches('meta.attribute.id.html > string', htmlId)).toBe(true)
    expect(selectorMatches('meta.attribute.id.html > string', [
      'text.html.basic',
      'string.quoted.double.html',
      'meta.attribute.id.html',
    ])).toBe(false)
  })

  it('scores more-specific selectors higher', () => {
    const generic = compileSelector('entity.name.function')
    const support = compileSelector('entity.name.function.support')
    expect(selectorScore(support, goSupport)).toBeGreaterThan(selectorScore(generic, goSupport))
  })
})
