// Tiny bash syntax highlighter for the chat's terminal cards. A real grammar is
// overkill (and mounting CodeMirror per card is too heavy) — this is a single-pass
// scanner producing safe HTML: text is built from escaped segments only.
//
// Colors (span classes, styled in styles/_chat.scss): sh-kw command words, sh-str strings,
// sh-com comments, sh-flag -flags, sh-var $vars, sh-op operators.

function esc(s: string): string {
  return s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;')
}

const OPS = ['&&', '||', '|', ';', '>>', '>', '<<', '<', '2>&1']

// highlightBash renders one shell command as highlighted HTML (safe to {@html}).
export function highlightBash(src: string): string {
  let out = ''
  let i = 0
  // atCmdStart: the next bare word is a command name (start, or after an operator).
  let atCmdStart = true

  const n = src.length
  while (i < n) {
    const c = src[i]

    // comment to end of line (only when preceded by start/whitespace)
    if (c === '#' && (i === 0 || /\s/.test(src[i - 1]))) {
      let j = src.indexOf('\n', i)
      if (j < 0) j = n
      out += `<span class="sh-com">${esc(src.slice(i, j))}</span>`
      i = j
      continue
    }

    // quoted strings (keep \-escapes inside double quotes)
    if (c === "'" || c === '"') {
      let j = i + 1
      while (j < n && src[j] !== c) {
        if (c === '"' && src[j] === '\\') j++
        j++
      }
      j = Math.min(j + 1, n)
      out += `<span class="sh-str">${esc(src.slice(i, j))}</span>`
      i = j
      atCmdStart = false
      continue
    }

    // $VAR / ${VAR}
    if (c === '$' && /[A-Za-z_{]/.test(src[i + 1] ?? '')) {
      const m = /^\$(\{[^}]*\}|[A-Za-z_]\w*)/.exec(src.slice(i))
      if (m) {
        out += `<span class="sh-var">${esc(m[0])}</span>`
        i += m[0].length
        atCmdStart = false
        continue
      }
    }

    // operators — a new command follows
    const op = OPS.find((o) => src.startsWith(o, i))
    if (op) {
      out += `<span class="sh-op">${esc(op)}</span>`
      i += op.length
      atCmdStart = true
      continue
    }

    if (c === '\n') {
      out += '\n'
      i++
      atCmdStart = true
      continue
    }
    if (/\s/.test(c)) {
      out += c
      i++
      continue
    }

    // a bare word: flag, command name, or plain text
    const m = /^[^\s'"#$|&;<>]+/.exec(src.slice(i))
    const word = m ? m[0] : c
    if (/^--?[\w][\w-]*=?$/.test(word)) {
      out += `<span class="sh-flag">${esc(word)}</span>`
    } else if (atCmdStart) {
      // VAR=value prefixes keep the command slot open
      if (/^[A-Za-z_]\w*=/.test(word)) {
        out += `<span class="sh-var">${esc(word)}</span>`
      } else {
        out += `<span class="sh-kw">${esc(word)}</span>`
        atCmdStart = false
      }
    } else {
      out += esc(word)
    }
    i += word.length
  }
  return out
}
