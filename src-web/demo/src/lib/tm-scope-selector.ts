/**
 * TextMate scope selectors for [[symbol]] (showInSymbolList).
 *
 * Subset: comma OR, space descendant, `>` child, `-` exclusion.
 * An atom matches a stack entry by dotted prefix (`entity.name.function`
 * matches `entity.name.function.go`).
 */
export type ScopeAtom = {
  prefix: string
  /** True when this atom follows `>` (must be the next stack entry). */
  child: boolean
}

export type ScopeConjunction = { atoms: ScopeAtom[] }

export type ScopeAlternative = {
  include: ScopeConjunction
  exclude: ScopeConjunction[]
}

export type CompiledSelector = { alts: ScopeAlternative[] }

export function scopePrefixMatch(want: string, have: string): boolean {
  return have === want || have.startsWith(want + '.')
}

function splitUnescaped(src: string, sep: string): string[] {
  const out: string[] = []
  let start = 0
  let i = 0
  while (i <= src.length - sep.length) {
    if (src.slice(i, i + sep.length) === sep) {
      out.push(src.slice(start, i))
      i += sep.length
      start = i
      continue
    }
    i++
  }
  out.push(src.slice(start))
  return out
}

function parseConjunction(src: string): ScopeConjunction {
  const atoms: ScopeAtom[] = []
  const parts = src.trim().split(/\s+/)
  let pendingChild = false
  for (const part of parts) {
    if (part === '>') {
      pendingChild = true
      continue
    }
    const prefix = part.replace(/^>+/, '')
    if (!prefix) continue
    const child = pendingChild || part.startsWith('>')
    pendingChild = false
    atoms.push({ prefix, child: child && atoms.length > 0 })
  }
  return { atoms }
}

function parseAlternative(src: string): ScopeAlternative {
  const chunks = splitUnescaped(src, ' - ')
  const include = parseConjunction(chunks[0] ?? '')
  const exclude = chunks.slice(1).map(parseConjunction)
  return { include, exclude }
}

export function compileSelector(src: string): CompiledSelector {
  const alts = splitUnescaped(src, ',')
    .map((a) => a.trim())
    .filter(Boolean)
    .map(parseAlternative)
  return { alts }
}

function matchConjunction(conj: ScopeConjunction, scopes: string[]): { ok: boolean; score: number } {
  if (conj.atoms.length === 0) return { ok: false, score: 0 }
  let i = 0
  let score = conj.atoms.length * 1000
  for (const atom of conj.atoms) {
    let found = -1
    if (atom.child) {
      if (i < scopes.length && scopePrefixMatch(atom.prefix, scopes[i])) found = i
    } else {
      for (let j = i; j < scopes.length; j++) {
        if (scopePrefixMatch(atom.prefix, scopes[j])) {
          found = j
          break
        }
      }
    }
    if (found < 0) return { ok: false, score: 0 }
    score += atom.prefix.split('.').filter(Boolean).length * 10 + found
    i = found + 1
  }
  return { ok: true, score }
}

/** Highest matching-alternative score, or 0 when nothing matches. */
export function selectorScore(sel: CompiledSelector, scopes: string[]): number {
  let best = 0
  for (const alt of sel.alts) {
    const inc = matchConjunction(alt.include, scopes)
    if (!inc.ok) continue
    let excluded = false
    for (const ex of alt.exclude) {
      if (matchConjunction(ex, scopes).ok) {
        excluded = true
        break
      }
    }
    if (!excluded && inc.score > best) best = inc.score
  }
  return best
}

export function selectorMatches(sel: CompiledSelector | string, scopes: string[]): boolean {
  const compiled = typeof sel === 'string' ? compileSelector(sel) : sel
  return selectorScore(compiled, scopes) > 0
}
