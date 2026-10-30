/** VS Code–style glob match for repo-relative paths (`*` and `**` only). */

function globToRegExp(glob: string): RegExp {
  let out = '^'
  for (let i = 0; i < glob.length; i++) {
    const c = glob[i]
    if (c === '*' && glob[i + 1] === '*') {
      if (glob[i + 2] === '/') {
        out += '(?:.*/)?'
        i += 2
      } else {
        out += '.*'
        i += 1
      }
    } else if (c === '*') {
      out += '[^/]*'
    } else if (c === '?') {
      out += '[^/]'
    } else if (/[.+^${}()|[\]\\]/.test(c)) {
      out += '\\' + c
    } else {
      out += c
    }
  }
  out += '$'
  return new RegExp(out)
}

const cache = new Map<string, RegExp>()

function compiled(pattern: string): RegExp {
  let re = cache.get(pattern)
  if (!re) {
    re = globToRegExp(pattern)
    cache.set(pattern, re)
  }
  return re
}

/**
 * Pre-resolve an exclude list to its regexes. The file tree tests every node
 * against every pattern on each render, and doing the trim/replace/Map lookup
 * per (node × pattern) dominated tree rendering on large repos: hoist it to
 * once per pattern-list change instead.
 */
export function compileExcludes(patterns: readonly string[]): RegExp[] {
  const out: RegExp[] = []
  for (const pat of patterns) {
    const p = pat.trim()
    if (!p) continue
    out.push(compiled(p.replace(/^\/+/, '')))
  }
  return out
}

/** Exclude check against a list already run through compileExcludes. */
export function isExcludedBy(path: string, compiledPatterns: readonly RegExp[]): boolean {
  if (compiledPatterns.length === 0) return false
  const rel = path.charCodeAt(0) === 47 ? path.replace(/^\/+/, '') : path
  for (let i = 0; i < compiledPatterns.length; i++) {
    if (compiledPatterns[i].test(rel)) return true
  }
  return false
}
