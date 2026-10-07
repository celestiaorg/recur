// Lightweight per-line syntax highlighting + line diff for the chat's mini code
// viewers (Edit/Write tool cards). Mounting CodeMirror per chat card is far too
// heavy; this is a single-pass, escape-safe tokenizer tuned for "looks right at
// a glance" — strings, comments, numbers, keywords — not a grammar. Per-line
// tokenizing means multi-line strings/comments can mis-tint a line; acceptable
// for card-sized snippets.

function esc(s: string): string {
  return s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;')
}

const KW: Record<string, string[]> = {
  js: ['const','let','var','function','return','if','else','for','while','switch','case','break','continue','new','class','extends','import','export','from','default','async','await','try','catch','finally','throw','typeof','instanceof','in','of','delete','void','yield','this','super','null','undefined','true','false','interface','type','enum','implements','readonly','public','private','protected','static','as','satisfies','keyof','namespace','declare','abstract','unknown'],
  go: ['func','return','if','else','for','range','switch','case','default','break','continue','go','defer','select','chan','map','struct','interface','type','var','const','package','import','goto','fallthrough','nil','true','false','make','new','len','cap','append','copy','delete','panic','recover','error','string','int','int64','int32','uint','byte','rune','bool','float64','float32','any'],
  py: ['def','return','if','elif','else','for','while','break','continue','import','from','as','class','try','except','finally','raise','with','lambda','pass','yield','global','nonlocal','assert','del','in','is','not','and','or','None','True','False','async','await','match','case','self'],
  rs: ['fn','let','mut','return','if','else','for','while','loop','match','break','continue','impl','trait','struct','enum','use','mod','pub','crate','self','super','const','static','ref','move','async','await','dyn','where','type','unsafe','extern','true','false','Some','None','Ok','Err'],
  c: ['int','char','float','double','void','long','short','unsigned','signed','struct','union','enum','typedef','const','static','extern','return','if','else','for','while','do','switch','case','break','continue','goto','sizeof','include','define','class','public','private','protected','virtual','template','typename','namespace','using','new','delete','nullptr','true','false','auto'],
  java: ['public','private','protected','class','interface','extends','implements','static','final','void','int','long','double','float','boolean','char','byte','short','new','return','if','else','for','while','switch','case','break','continue','try','catch','finally','throw','throws','import','package','this','super','null','true','false','abstract','synchronized','instanceof','enum','record','var'],
  sql: ['select','from','where','insert','into','values','update','set','delete','create','table','index','drop','alter','join','left','right','inner','outer','on','group','by','order','having','limit','offset','as','and','or','not','null','primary','key','foreign','references','unique','default','cascade','if','exists','distinct','union','all','desc','asc','count','sum','avg','min','max'],
  rb: ['begin','end','def','class','module','require','require_relative','include','extend','if','elsif','else','unless','case','when','while','until','for','do','return','yield','break','next','redo','retry','rescue','ensure','raise','super','self','nil','true','false','and','or','not','alias','undef','attr_reader','attr_writer','attr_accessor','private','protected','public','lambda','proc','puts','print'],
  php: ['echo','print','function','return','if','else','elseif','for','foreach','while','do','switch','case','break','continue','class','interface','trait','extends','implements','public','private','protected','static','new','try','catch','finally','throw','namespace','use','as','const','var','array','true','false','null','instanceof','clone'],
}

type Lang = { kw: Set<string>; lineComment: string[]; hash?: boolean }

const LANGS: Record<string, Lang> = {
  js: { kw: new Set(KW.js), lineComment: ['//'] },
  go: { kw: new Set(KW.go), lineComment: ['//'] },
  py: { kw: new Set(KW.py), lineComment: ['#'] },
  rs: { kw: new Set(KW.rs), lineComment: ['//'] },
  c: { kw: new Set(KW.c), lineComment: ['//'] },
  java: { kw: new Set(KW.java), lineComment: ['//'] },
  sql: { kw: new Set(KW.sql), lineComment: ['--'] },
  rb: { kw: new Set(KW.rb), lineComment: ['#'] },
  php: { kw: new Set(KW.php), lineComment: ['//', '#'] },
  css: { kw: new Set<string>(), lineComment: [] },
  sh: { kw: new Set<string>(), lineComment: ['#'] },
  json: { kw: new Set(['true', 'false', 'null']), lineComment: [] },
  yaml: { kw: new Set(['true', 'false', 'null']), lineComment: ['#'] },
  text: { kw: new Set<string>(), lineComment: [] },
}

// File-extension → highlighter id (tool cards that know a path).
const EXT: Record<string, string> = {
  js: 'js', jsx: 'js', ts: 'js', tsx: 'js', mjs: 'js', cjs: 'js', svelte: 'js', vue: 'js',
  go: 'go',
  py: 'py',
  rs: 'rs',
  c: 'c', h: 'c', cc: 'c', cpp: 'c', hpp: 'c', m: 'c',
  java: 'java', kt: 'java', swift: 'java', scala: 'java',
  sql: 'sql',
  rb: 'rb',
  php: 'php',
  css: 'css', scss: 'css', less: 'css',
  sh: 'sh', bash: 'sh', zsh: 'sh',
  json: 'json',
  yaml: 'yaml', yml: 'yaml', toml: 'yaml',
}

// Fence info-string aliases (```ruby, ```typescript, …) → highlighter id.
const INFO: Record<string, string> = {
  js: 'js', javascript: 'js', jsx: 'js', mjs: 'js', cjs: 'js',
  ts: 'js', typescript: 'js', tsx: 'js',
  svelte: 'js', vue: 'js',
  go: 'go', golang: 'go',
  py: 'py', python: 'py',
  rs: 'rs', rust: 'rs',
  c: 'c', h: 'c', cpp: 'c', 'c++': 'c', cc: 'c', cxx: 'c', hpp: 'c',
  java: 'java', kotlin: 'java', kt: 'java', swift: 'java', scala: 'java',
  sql: 'sql',
  rb: 'rb', ruby: 'rb',
  php: 'php',
  css: 'css', scss: 'css', less: 'css',
  sh: 'sh', bash: 'sh', zsh: 'sh', shell: 'sh',
  json: 'json',
  yaml: 'yaml', yml: 'yaml', toml: 'yaml',
  text: 'text', plaintext: 'text', txt: 'text',
  html: 'text', xml: 'text', md: 'text', markdown: 'text',
}

// langFromPath maps a file path to a highlighter language id ('text' = plain).
export function langFromPath(path: string): string {
  const base = path.slice(path.lastIndexOf('/') + 1)
  const ext = base.slice(base.lastIndexOf('.') + 1).toLowerCase()
  return EXT[ext] ?? 'text'
}

// langFromInfo maps a markdown fence info string (e.g. "ruby", "ts title=x") to
// a highlighter id. Unknown tags still get string/number tinting via 'text'.
export function langFromInfo(info: string): string {
  const tag = info.trim().split(/\s+/)[0]?.toLowerCase() ?? ''
  if (!tag) return 'text'
  return INFO[tag] ?? 'text'
}

function decodeHtmlText(s: string): string {
  return s
    .replace(/<br\s*\/?>/gi, '\n')
    .replace(/&lt;/g, '<')
    .replace(/&gt;/g, '>')
    .replace(/&quot;/g, '"')
    .replace(/&#39;/g, "'")
    .replace(/&amp;/g, '&')
}

function langFromCodeClass(cls: string): string {
  for (const c of cls.split(/\s+/)) {
    if (!c) continue
    const tag = c.replace(/^language-|^lang-/, '')
    const id = langFromInfo(tag)
    if (id !== 'text') return id
  }
  return 'text'
}

/** Tint ```language fences in LSP hover/signature HTML (same sh-* spans as chat). */
export function highlightHtmlCodeBlocks(html: string): string {
  if (!html.includes('<pre')) return html
  return html.replace(/<pre([^>]*)><code([^>]*)>([\s\S]*?)<\/code><\/pre>/gi, (full, preAttrs, codeAttrs, inner) => {
    if (/<[a-z]/i.test(String(inner).replace(/<br\s*\/?>/gi, ''))) return full
    const cls = /class="([^"]*)"/.exec(String(codeAttrs))?.[1]
      ?? /class="([^"]*)"/.exec(String(preAttrs))?.[1]
      ?? ''
    const raw = decodeHtmlText(String(inner))
    return `<pre${preAttrs}><code${codeAttrs}>${highlightCode(raw, langFromCodeClass(cls))}</code></pre>`
  })
}

// highlightCode renders a multi-line snippet (markdown fences, etc.).
export function highlightCode(src: string, langID: string): string {
  const body = src.endsWith('\n') ? src.slice(0, -1) : src
  if (!body) return ''
  return body.split('\n').map((line) => highlightLine(line, langID)).join('\n')
}

// highlightLine renders ONE source line as safe HTML with sh-* token spans
// (same palette as the bash highlighter).
export function highlightLine(line: string, langID: string): string {
  const lang = LANGS[langID] ?? LANGS.text
  let out = ''
  let i = 0
  const n = line.length
  while (i < n) {
    const c = line[i]

    // line comment (js //, py/sh #, sql --) — the rest of the line
    const lc = lang.lineComment.find((p) => line.startsWith(p, i))
    if (lc) {
      out += `<span class="sh-com">${esc(line.slice(i))}</span>`
      break
    }

    // strings ' " `
    if (c === "'" || c === '"' || c === '`') {
      let j = i + 1
      while (j < n && line[j] !== c) {
        if (line[j] === '\\') j++
        j++
      }
      j = Math.min(j + 1, n)
      out += `<span class="sh-str">${esc(line.slice(i, j))}</span>`
      i = j
      continue
    }

    // numbers
    if (/[0-9]/.test(c) && !/[\w$]/.test(line[i - 1] ?? '')) {
      const m = /^[0-9][\w.]*/.exec(line.slice(i))!
      out += `<span class="sh-num">${esc(m[0])}</span>`
      i += m[0].length
      continue
    }

    // words: keyword or plain
    if (/[A-Za-z_$]/.test(c)) {
      const m = /^[\w$]+/.exec(line.slice(i))!
      const word = m[0]
      const kw = langID === 'sql' ? word.toLowerCase() : word
      out += lang.kw.has(kw) ? `<span class="sh-kw">${esc(word)}</span>` : esc(word)
      i += word.length
      continue
    }

    out += esc(c)
    i++
  }
  return out
}

// ---- line diff (for Edit cards): old/new snippet -> context/del/add rows ----

export type DiffRow = { kind: 'ctx' | 'del' | 'add'; text: string }

// diffLines computes a minimal line diff via LCS. Edit tool snippets are small
// (old_string carries its own context lines); beyond the cap fall back to
// all-del + all-add rather than an O(n·m) blowup.
export function diffLines(oldS: string, newS: string): DiffRow[] {
  const a = oldS.replace(/\n$/, '').split('\n')
  const b = newS.replace(/\n$/, '').split('\n')
  if (a.length * b.length > 250_000) {
    return [...a.map((t) => ({ kind: 'del' as const, text: t })), ...b.map((t) => ({ kind: 'add' as const, text: t }))]
  }
  // LCS table
  const m = a.length
  const n = b.length
  const dp: Uint32Array = new Uint32Array((m + 1) * (n + 1))
  for (let i = m - 1; i >= 0; i--) {
    for (let j = n - 1; j >= 0; j--) {
      dp[i * (n + 1) + j] = a[i] === b[j]
        ? dp[(i + 1) * (n + 1) + j + 1] + 1
        : Math.max(dp[(i + 1) * (n + 1) + j], dp[i * (n + 1) + j + 1])
    }
  }
  const rows: DiffRow[] = []
  let i = 0
  let j = 0
  while (i < m && j < n) {
    if (a[i] === b[j]) {
      rows.push({ kind: 'ctx', text: a[i] })
      i++
      j++
    } else if (dp[(i + 1) * (n + 1) + j] >= dp[i * (n + 1) + j + 1]) {
      rows.push({ kind: 'del', text: a[i] })
      i++
    } else {
      rows.push({ kind: 'add', text: b[j] })
      j++
    }
  }
  for (; i < m; i++) rows.push({ kind: 'del', text: a[i] })
  for (; j < n; j++) rows.push({ kind: 'add', text: b[j] })
  return rows
}

// parseUnifiedDiff turns a unified diff into the same DiffRow[] the Edit card
// renders from old_string/new_string.
//
// Claude Code's Edit tool reports its change as before/after strings, so the card
// can diff them itself. Cursor's `edit` reports only the file path in its args
// and puts a ready-made unified diff in the tool RESULT — so this is the only
// place that change is visible. Parsing it into DiffRow keeps ONE diff viewer
// instead of a second, provider-specific one.
//
// Returns [] for anything that is not a diff, which the card treats as "no diff
// to show" and falls back to plain result text.
export function parseUnifiedDiff(src: string): DiffRow[] {
  if (!src) return []
  const rows: DiffRow[] = []
  let inHunk = false
  for (const line of src.split('\n')) {
    // File headers carry no content; hunk headers start a body.
    if (line.startsWith('--- ') || line.startsWith('+++ ')) continue
    if (line.startsWith('@@')) {
      inHunk = true
      continue
    }
    if (!inHunk) continue
    // "\ No newline at end of file" is a marker, not a line of the file.
    if (line.startsWith('\\')) continue
    if (line.startsWith('+')) rows.push({ kind: 'add', text: line.slice(1) })
    else if (line.startsWith('-')) rows.push({ kind: 'del', text: line.slice(1) })
    else if (line.startsWith(' ')) rows.push({ kind: 'ctx', text: line.slice(1) })
    else if (line === '') rows.push({ kind: 'ctx', text: '' })
  }
  return rows
}

// looksLikeUnifiedDiff is a cheap guard so a tool result that merely happens to
// start with a dash is not rendered as a diff.
//
// Cursor emits full unified diffs (`--- a` / `+++ b` + @@ hunks). Codex
// fileChange updates often send hunk-only bodies that start at `@@`.
export function looksLikeUnifiedDiff(src: string): boolean {
  const s = src ?? ''
  if (!s.includes('@@')) return false
  if (/^--- .*\n\+\+\+ /m.test(s)) return true
  const first = s.split('\n').find((l) => l.trim() !== '')
  return !!first && first.startsWith('@@')
}
