/**
 * Contenteditable helpers for the agent chat composer: serialize inline file /
 * image chips ↔ @path / ⟦attachment:id⟧ wire text, hydrate DOM, place caret.
 *
 * Structure: one <p> per line. WebKit’s loose div/br junk is
 * normalized back into clean paragraphs on each sync.
 */
import {
  FILE_AT_RE, ATTACHMENT_MARKER_RE, SKILL_SLASH_RE, TERMINAL_AT_RE, ARTIFACT_AT_RE,
  fileMentionToken, fileMentionLabel, parseFileMentionChip, dirFileMentionPath, isDirFileMention,
  repairFileMentionBoundaries,
  attachmentMarker, skillMentionToken,
  terminalMentionToken, terminalMentionLabel,
  artifactMentionToken, artifactMentionLabel,
  collectLooseAtNames, tokenSpanOverlaps,
} from './agentchat'
import { fileIconClass } from './file-icons'

export type ComposerAtt = { id: string; dataUrl: string; name: string }

function baseName(path: string): string {
  const i = path.lastIndexOf('/')
  return i >= 0 ? path.slice(i + 1) : path
}

export function isDirChipPath(path: string): boolean {
  return isDirFileMention(path)
}

function fileChipWire(path: string, dir?: boolean): string {
  const p = dir || isDirFileMention(path) ? dirFileMentionPath(path) : path
  return fileMentionToken(p)
}

const FOLDER_SVG =
  '<svg xmlns="http://www.w3.org/2000/svg" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">' +
  '<path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"/></svg>'

/** Build an atomic, non-editable file chip for the composer (`path` or `path:lines`). */
export function makeFileChip(chipPath: string): HTMLSpanElement {
  const { path } = parseFileMentionChip(chipPath)
  const dir = isDirFileMention(chipPath)
  const stored = dir ? dirFileMentionPath(path || chipPath) : chipPath
  const label = fileMentionLabel(stored)
  const chip = document.createElement('span')
  chip.className = 'chat-composer-chip'
  chip.contentEditable = 'false'
  chip.dataset.filePath = stored
  if (dir) chip.dataset.dir = '1'
  chip.title = dir ? path : `Open ${chipPath}`
  chip.setAttribute('aria-label', dir ? path : `Open ${chipPath}`)
  if (!dir) chip.setAttribute('role', 'link')

  const icon = document.createElement('span')
  icon.setAttribute('aria-hidden', 'true')
  if (dir) {
    icon.className = 'folder-ico'
    icon.innerHTML = FOLDER_SVG
  } else {
    icon.className = `fico ${fileIconClass(baseName(path || chipPath))}`
  }

  const name = document.createElement('span')
  name.className = 'chat-composer-chip-name'
  name.textContent = label

  const x = document.createElement('button')
  x.type = 'button'
  x.className = 'chat-composer-chip-x'
  x.dataset.chipRemove = '1'
  x.setAttribute('aria-label', `Remove ${label}`)
  x.textContent = '×'

  chip.append(icon, name, x)
  return chip
}

/** Build an atomic terminal chip (`@terminal` or `@terminal:<id>`). */
export function makeTerminalChip(id: string, title?: string): HTMLSpanElement {
  const chip = document.createElement('span')
  chip.className = 'chat-composer-chip chat-composer-terminal'
  chip.contentEditable = 'false'
  chip.dataset.terminalChip = '1'
  chip.dataset.terminalId = id || ''
  const label = terminalMentionLabel(id ? `terminal:${id}` : 'terminal', title)
  chip.title = title ? `${title} terminal` : 'Terminal'
  chip.setAttribute('aria-label', `Terminal ${label}`)

  const icon = document.createElement('span')
  icon.className = 'term-ico'
  icon.setAttribute('aria-hidden', 'true')
  icon.innerHTML =
    '<svg xmlns="http://www.w3.org/2000/svg" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">' +
    '<polyline points="4 17 10 11 4 5"/><line x1="12" x2="20" y1="19" y2="19"/></svg>'

  const name = document.createElement('span')
  name.className = 'chat-composer-chip-name'
  name.textContent = label

  const x = document.createElement('button')
  x.type = 'button'
  x.className = 'chat-composer-chip-x'
  x.dataset.chipRemove = '1'
  x.setAttribute('aria-label', `Remove ${label}`)
  x.textContent = '×'

  chip.append(icon, name, x)
  return chip
}

/** Build an atomic artifact chip (`@artifact` or `@artifact:<id>`). */
export function makeArtifactChip(id: string, title?: string): HTMLSpanElement {
  const chip = document.createElement('span')
  chip.className = 'chat-composer-chip chat-composer-artifact'
  chip.contentEditable = 'false'
  chip.dataset.artifactChip = '1'
  chip.dataset.artifactId = id || ''
  const label = artifactMentionLabel(id ? `artifact:${id}` : 'artifact', title)
  chip.title = title ? `${title} artifact` : 'Artifact'
  chip.setAttribute('aria-label', `Artifact ${label}`)

  const icon = document.createElement('span')
  icon.className = 'art-ico'
  icon.setAttribute('aria-hidden', 'true')
  icon.innerHTML =
    '<svg xmlns="http://www.w3.org/2000/svg" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">' +
    '<path d="M12 3 4 7l8 4 8-4-8-4z"/><path d="M4 12l8 4 8-4"/><path d="M4 17l8 4 8-4"/></svg>'

  const name = document.createElement('span')
  name.className = 'chat-composer-chip-name'
  name.textContent = label

  const x = document.createElement('button')
  x.type = 'button'
  x.className = 'chat-composer-chip-x'
  x.dataset.chipRemove = '1'
  x.setAttribute('aria-label', `Remove ${label}`)
  x.textContent = '×'

  chip.append(icon, name, x)
  return chip
}

/** Build an atomic skill chip (`/name` on the wire). */
export function makeSkillChip(name: string): HTMLSpanElement {
  const chip = document.createElement('span')
  chip.className = 'chat-composer-chip chat-composer-skill'
  chip.contentEditable = 'false'
  chip.dataset.skillName = name
  chip.title = `/${name}`
  chip.setAttribute('aria-label', `Skill ${name}`)

  const icon = document.createElement('span')
  icon.className = 'skill-ico'
  icon.setAttribute('aria-hidden', 'true')
  // Inline SVG — matches Icon "slash" (steeper, `/`-like angle).
  icon.innerHTML =
    '<svg xmlns="http://www.w3.org/2000/svg" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">' +
    '<path d="M15 3.5 9 20.5"/></svg>'

  const label = document.createElement('span')
  label.className = 'chat-composer-chip-name'
  label.textContent = name

  const x = document.createElement('button')
  x.type = 'button'
  x.className = 'chat-composer-chip-x'
  x.dataset.chipRemove = '1'
  x.setAttribute('aria-label', `Remove /${name}`)
  x.textContent = '×'

  chip.append(icon, label, x)
  return chip
}

/** Named [[prompt.arg]] slot rendered inside a Discord-style prompt chip. */
export type PromptChipArg = {
  name: string
  placeholder?: string
  /** Empty + Enter invokes; scripts treat empty as unset. Default required. */
  optional?: boolean
}

export type PromptChipSpec = {
  slash: string
  args: PromptChipArg[]
}

function promptArgValues(spec: PromptChipSpec, values?: string[]): string[] {
  const n = spec.args.length || 1
  const out = Array.from({ length: n }, (_, i) => (values?.[i] ?? '').trim())
  return out
}

/** Wire form: `/slash` plus trailing arg values. Slots with spaces are quoted. */
export function encodePromptWire(slash: string, values: string[]): string {
  const s = slash.replace(/^\//, '').trim()
  const filled = values.map((v) => v.trim())
  if (filled.every((v) => !v)) return `/${s} `
  if (filled.length <= 1) return `/${s} ${filled[0] ?? ''}`.trimEnd()
  return `/${s} ${filled.map(encodePromptArgToken).join(' ')}`.trimEnd()
}

function encodePromptArgToken(v: string): string {
  if (!v) return '""'
  if (/[\s"]/.test(v)) return `"${v.replace(/\\/g, '\\\\').replace(/"/g, '\\"')}"`
  return v
}

/** Split on whitespace; `"quoted"` tokens keep spaces (`\"` / `\\` escapes). */
export function splitPromptArgTokens(raw: string): string[] {
  const s = (raw ?? '').trim()
  const out: string[] = []
  let i = 0
  while (i < s.length) {
    while (i < s.length && /\s/.test(s[i]!)) i++
    if (i >= s.length) break
    if (s[i] === '"') {
      i++
      let tok = ''
      while (i < s.length) {
        if (s[i] === '\\' && i + 1 < s.length) {
          tok += s[i + 1]
          i += 2
          continue
        }
        if (s[i] === '"') {
          i++
          break
        }
        tok += s[i]
        i++
      }
      out.push(tok)
      continue
    }
    let j = i
    while (j < s.length && !/\s/.test(s[j]!)) j++
    out.push(s.slice(i, j))
    i = j
  }
  return out
}

export function parsePromptChipValues(rest: string, spec: PromptChipSpec): string[] {
  const n = spec.args.length || 1
  const raw = (rest ?? '').trim()
  if (n <= 1) return [raw]
  const parts = splitPromptArgTokens(raw)
  return spec.args.map((_, i) => (i === n - 1 ? parts.slice(i).join(' ') : (parts[i] ?? '')))
}

function matchPromptChipLine(line: string, specs: readonly PromptChipSpec[] | undefined): { spec: PromptChipSpec; values: string[] } | null {
  if (!specs?.length) return null
  const t = (line ?? '').trim()
  const m = /^\/([a-z][a-z0-9-]*)(?:\s+([\s\S]*))?$/.exec(t)
  if (!m) return null
  const spec = specs.find((s) => s.slash === m[1])
  if (!spec || !spec.args.length) return null
  return { spec, values: parsePromptChipValues(m[2] ?? '', spec) }
}

function resizePromptArgInput(input: HTMLInputElement) {
  const n = Math.max(8, (input.value || input.placeholder || '').length + 1)
  input.style.width = `${n}ch`
}

/** Discord-style `/slash` + labeled arg slots. Values live in <input>s. */
export function makePromptChip(spec: PromptChipSpec, values?: string[]): HTMLSpanElement {
  const chip = document.createElement('span')
  chip.className = 'chat-composer-chip chat-composer-prompt'
  chip.contentEditable = 'false'
  chip.dataset.promptSlash = spec.slash
  chip.title = `/${spec.slash}`
  chip.setAttribute('aria-label', `Prompt /${spec.slash}`)

  const cmd = document.createElement('span')
  cmd.className = 'chat-composer-prompt-cmd'
  cmd.textContent = `/${spec.slash}`

  chip.append(cmd)
  const vals = promptArgValues(spec, values)
  const args = spec.args.length ? spec.args : [{ name: 'args', placeholder: '' }]
  args.forEach((arg, i) => {
    const slot = document.createElement('span')
    slot.className = 'chat-composer-prompt-arg'
    slot.dataset.argName = arg.name
    slot.dataset.argIndex = String(i)

    const lab = document.createElement('span')
    lab.className = 'chat-composer-prompt-arg-label'
    lab.textContent = arg.name

    const input = document.createElement('input')
    input.type = 'text'
    input.className = 'chat-composer-prompt-arg-input'
    input.spellcheck = false
    input.autocomplete = 'off'
    input.placeholder = arg.placeholder || ''
    input.value = vals[i] ?? ''
    input.setAttribute('aria-label', arg.placeholder || arg.name)
    resizePromptArgInput(input)
    input.addEventListener('input', () => resizePromptArgInput(input))
    input.addEventListener('focus', () => slot.classList.add('on'))
    input.addEventListener('blur', () => slot.classList.remove('on'))

    slot.append(lab, input)
    chip.append(slot)
  })

  const x = document.createElement('button')
  x.type = 'button'
  x.className = 'chat-composer-chip-x'
  x.dataset.chipRemove = '1'
  x.setAttribute('aria-label', `Remove /${spec.slash}`)
  x.textContent = '×'
  chip.append(x)

  chip.addEventListener('mousedown', (e) => {
    const t = e.target as HTMLElement | null
    if (!t || t.closest('[data-chip-remove]') || t.classList.contains('chat-composer-prompt-arg-input')) return
    const slot = t.closest('.chat-composer-prompt-arg')
    const input = (slot?.querySelector('.chat-composer-prompt-arg-input') as HTMLInputElement | null)
      ?? promptChipInputs(chip)[0]
    if (!input) return
    e.preventDefault()
    input.focus()
  })
  return chip
}

export function promptChipInputs(chip: HTMLElement): HTMLInputElement[] {
  return [...chip.querySelectorAll<HTMLInputElement>('.chat-composer-prompt-arg-input')]
}

export function readPromptChipValues(chip: HTMLElement): string[] {
  return promptChipInputs(chip).map((el) => el.value)
}

export function serializePromptChip(chip: HTMLElement): string {
  const slash = chip.dataset.promptSlash || ''
  return encodePromptWire(slash, readPromptChipValues(chip))
}

export function focusPromptChipArg(chip: HTMLElement, index = 0): HTMLInputElement | null {
  const inputs = promptChipInputs(chip)
  const el = inputs[Math.max(0, Math.min(index, inputs.length - 1))]
  el?.focus()
  el?.select()
  return el ?? null
}

export function findPromptChip(root: HTMLElement): HTMLElement | null {
  return root.querySelector<HTMLElement>('[data-prompt-slash]')
}

/** Build an inline image attachment chip (thumbnail + remove). */
export function makeAttachmentChip(att: ComposerAtt): HTMLSpanElement {
  const chip = document.createElement('span')
  chip.className = 'chat-composer-chip chat-composer-att'
  chip.contentEditable = 'false'
  chip.dataset.attachmentId = att.id
  const label = att.name || 'image'
  chip.title = `View ${label}`
  chip.setAttribute('aria-label', `View ${label}`)
  chip.setAttribute('role', 'button')

  const img = document.createElement('img')
  img.className = 'chat-composer-att-img'
  img.src = att.dataUrl
  img.alt = att.name || ''
  img.draggable = false

  const x = document.createElement('button')
  x.type = 'button'
  x.className = 'chat-composer-chip-x'
  x.dataset.chipRemove = '1'
  x.title = `Remove ${label}`
  x.setAttribute('aria-label', `Remove ${label}`)
  x.textContent = '×'

  chip.append(img, x)
  return chip
}

function attById(atts: readonly ComposerAtt[] | undefined, id: string): ComposerAtt | undefined {
  return atts?.find((a) => a.id === id)
}

function isComposerChip(node: Node | null | undefined): node is HTMLElement {
  if (!node || node.nodeType !== Node.ELEMENT_NODE) return false
  const el = node as HTMLElement
  return !!(el.dataset.filePath || el.dataset.attachmentId || el.dataset.skillName || el.dataset.terminalChip || el.dataset.artifactChip || el.dataset.promptSlash)
}

/** Paragraph/div block — boolean (not a type predicate) so HTMLElement isn't narrowed to never. */
function isBlockEl(node: Node | null | undefined): boolean {
  return !!node && node.nodeType === Node.ELEMENT_NODE &&
    ((node as HTMLElement).tagName === 'P' || (node as HTMLElement).tagName === 'DIV')
}

function chipWireLen(el: HTMLElement): number {
  if (el.dataset.filePath) return fileChipWire(el.dataset.filePath, el.dataset.dir === '1').length
  if (el.dataset.attachmentId) return attachmentMarker(el.dataset.attachmentId).length
  if (el.dataset.skillName) return skillMentionToken(el.dataset.skillName).length
  if (el.dataset.terminalChip) return terminalMentionToken(el.dataset.terminalId).length
  if (el.dataset.artifactChip) return artifactMentionToken(el.dataset.artifactId).length
  if (el.dataset.promptSlash) return serializePromptChip(el).length
  return 0
}

function isZwspOnly(t: string): boolean {
  return /^[\u200B\uFEFF]+$/.test(t)
}

/** Inline filler only — never structural <p>/<div> blocks. */
function isInlineFiller(node: Node | null | undefined): boolean {
  if (!node) return false
  if (node.nodeType === Node.TEXT_NODE) {
    const t = node.textContent ?? ''
    return t.length === 0 || isZwspOnly(t)
  }
  if (node.nodeType !== Node.ELEMENT_NODE) return false
  const el = node as HTMLElement
  if (el.dataset.filePath || el.dataset.attachmentId || el.dataset.skillName || el.dataset.terminalChip || el.dataset.artifactChip || el.dataset.promptSlash) return false
  if (el.tagName === 'P' || el.tagName === 'DIV') return false
  return el.tagName === 'BR'
}

function ensureParagraphs(root: HTMLElement): HTMLElement[] {
  return [...root.childNodes].filter((n): n is HTMLElement => isBlockEl(n))
}

/** Serialize inline children of a block (chips / text; placeholder <br> → ""). */
function serializeInline(container: HTMLElement): string {
  let out = ''
  for (const node of container.childNodes) {
    if (node.nodeType === Node.TEXT_NODE) {
      out += node.textContent ?? ''
      continue
    }
    if (node.nodeType !== Node.ELEMENT_NODE) continue
    const el = node as HTMLElement
    if (el.dataset.promptSlash) {
      out += serializePromptChip(el)
      continue
    }
    if (el.dataset.filePath) {
      out += fileChipWire(el.dataset.filePath, el.dataset.dir === '1')
      continue
    }
    if (el.dataset.skillName) {
      out += skillMentionToken(el.dataset.skillName)
      continue
    }
    if (el.dataset.attachmentId) {
      out += attachmentMarker(el.dataset.attachmentId)
      continue
    }
    if (el.dataset.terminalChip) {
      out += terminalMentionToken(el.dataset.terminalId)
      continue
    }
    if (el.dataset.artifactChip) {
      out += artifactMentionToken(el.dataset.artifactId)
      continue
    }
    if (el.tagName === 'BR') continue
    // Nested span/etc. — recurse.
    out += serializeInline(el)
  }
  return out
}

/** Serialize composer DOM to wire text (@path /skill / ⟦attachment:id⟧ / plain). */
export function serializeComposer(root: HTMLElement): string {
  const blocks = ensureParagraphs(root)
  if (!blocks.length) {
    // Flat / messy — treat root as a single line (normalize will wrap soon).
    return serializeInline(root)
  }
  return blocks.map((p) => serializeInline(p)).join('\n')
}

type WireTok =
  | { kind: 'file'; index: number; len: number; path: string }
  | { kind: 'skill'; index: number; len: number; name: string }
  | { kind: 'att'; index: number; len: number; id: string }
  | { kind: 'terminal'; index: number; len: number; id: string }
  | { kind: 'artifact'; index: number; len: number; id: string }

function wireTokens(
  text: string,
  artifactIdForTitle?: (title: string) => string,
): WireTok[] {
  const toks: WireTok[] = []
  const artRe = new RegExp(ARTIFACT_AT_RE.source, 'g')
  for (const m of text.matchAll(artRe)) {
    if (m.index == null) continue
    toks.push({ kind: 'artifact', index: m.index, len: m[0].length, id: m[1] || '' })
  }
  const termRe = new RegExp(TERMINAL_AT_RE.source, 'g')
  for (const m of text.matchAll(termRe)) {
    if (m.index == null) continue
    toks.push({ kind: 'terminal', index: m.index, len: m[0].length, id: m[1] || '' })
  }
  const fileRe = new RegExp(FILE_AT_RE.source, 'g')
  for (const m of text.matchAll(fileRe)) {
    if (m[1] == null || m.index == null) continue
    toks.push({ kind: 'file', index: m.index, len: m[0].length, path: m[1] })
  }
  const skillRe = new RegExp(SKILL_SLASH_RE.source, 'g')
  for (const m of text.matchAll(skillRe)) {
    if (m[1] == null || m.index == null) continue
    // match[0] may include a leading whitespace lookbehind char via (?<=\s) —
    // the `/name` starts at the `/` inside the match. Find `/` offset.
    const slash = m[0].indexOf('/')
    const index = m.index + (slash >= 0 ? slash : 0)
    const len = 1 + m[1].length
    toks.push({ kind: 'skill', index, len, name: m[1] })
  }
  const attRe = new RegExp(ATTACHMENT_MARKER_RE.source, 'g')
  for (const m of text.matchAll(attRe)) {
    if (m[1] == null || m.index == null) continue
    toks.push({ kind: 'att', index: m.index, len: m[0].length, id: m[1] })
  }
  if (artifactIdForTitle) {
    for (const hit of collectLooseAtNames(text)) {
      const id = artifactIdForTitle(hit.token)
      if (!id) continue
      if (tokenSpanOverlaps(hit.index, hit.len, toks)) continue
      toks.push({ kind: 'artifact', index: hit.index, len: hit.len, id })
    }
  }
  toks.sort((a, b) => a.index - b.index)
  return toks
}

function fillParagraph(
  p: HTMLElement,
  line: string,
  attachments: readonly ComposerAtt[],
  chipFiles: boolean,
  chipSkills: boolean,
  seenAtt: Set<string>,
  terminalTitle?: (id: string) => string,
  artifactTitle?: (id: string) => string,
  artifactIdForTitle?: (title: string) => string,
  promptChips?: readonly PromptChipSpec[],
) {
  if (!line) {
    p.appendChild(document.createElement('br'))
    return
  }
  const promptHit = matchPromptChipLine(line, promptChips)
  if (promptHit) {
    p.appendChild(makePromptChip(promptHit.spec, promptHit.values))
    return
  }
  const toks = wireTokens(line, artifactIdForTitle)
  let last = 0
  for (const tok of toks) {
    if (tok.index < last) continue
    if (tok.index > last) p.appendChild(document.createTextNode(line.slice(last, tok.index)))
    if (tok.kind === 'file') {
      if (chipFiles) p.appendChild(makeFileChip(tok.path))
      else p.appendChild(document.createTextNode(line.slice(tok.index, tok.index + tok.len)))
    } else if (tok.kind === 'skill') {
      if (chipSkills) p.appendChild(makeSkillChip(tok.name))
      else p.appendChild(document.createTextNode(line.slice(tok.index, tok.index + tok.len)))
    } else if (tok.kind === 'terminal') {
      if (chipFiles) p.appendChild(makeTerminalChip(tok.id, terminalTitle?.(tok.id)))
      else p.appendChild(document.createTextNode(line.slice(tok.index, tok.index + tok.len)))
    } else if (tok.kind === 'artifact') {
      if (chipFiles) p.appendChild(makeArtifactChip(tok.id, artifactTitle?.(tok.id)))
      else p.appendChild(document.createTextNode(line.slice(tok.index, tok.index + tok.len)))
    } else {
      const att = attById(attachments, tok.id) ?? {
        id: tok.id, dataUrl: '', name: tok.id,
      }
      if (att.dataUrl) p.appendChild(makeAttachmentChip(att))
      else p.appendChild(document.createTextNode(attachmentMarker(tok.id)))
      seenAtt.add(tok.id)
    }
    last = tok.index + tok.len
  }
  if (last < line.length) p.appendChild(document.createTextNode(line.slice(last)))
  if (!p.childNodes.length) p.appendChild(document.createElement('br'))
}

/**
 * Replace root contents from wire text — one <p> per line.
 * `chipFiles` / `chipSkills`: recreate chips from wire tokens (tab restore).
 * Leave false while typing — chips are only inserted by an explicit picker commit.
 */
export function hydrateComposer(
  root: HTMLElement,
  text: string,
  attachments: readonly ComposerAtt[] = [],
  opts: { chipFiles?: boolean; chipSkills?: boolean; terminalTitle?: (id: string) => string; artifactTitle?: (id: string) => string; artifactIdForTitle?: (title: string) => string; promptChips?: readonly PromptChipSpec[] } = {},
) {
  const chipFiles = !!opts.chipFiles
  const chipSkills = !!opts.chipSkills
  // Chips that lost their leading space serialize as `this@path` — repair so
  // FILE_AT_RE can chipify them again (slashy / dotfile paths only).
  if (chipFiles) text = repairFileMentionBoundaries(text)
  root.replaceChildren()
  const seenAtt = new Set<string>()
  const lines = text.split('\n')
  for (const line of lines) {
    const p = document.createElement('p')
    fillParagraph(p, line, attachments, chipFiles, chipSkills, seenAtt, opts.terminalTitle, opts.artifactTitle, opts.artifactIdForTitle, opts.promptChips)
    root.appendChild(p)
  }
  // Orphan image drafts not present in wire text — append to last paragraph.
  const orphans = attachments.filter((a) => !seenAtt.has(a.id) && a.dataUrl)
  if (orphans.length) {
    let p = root.lastElementChild as HTMLElement | null
    if (!p || !isBlockEl(p)) {
      p = document.createElement('p')
      root.appendChild(p)
    }
    // Drop placeholder <br> before appending chips.
    if (p.childNodes.length === 1 && (p.firstChild as HTMLElement)?.tagName === 'BR') {
      p.replaceChildren()
    }
    for (const a of orphans) p.appendChild(makeAttachmentChip(a))
  }
  if (!root.childNodes.length) {
    const p = document.createElement('p')
    p.appendChild(document.createElement('br'))
    root.appendChild(p)
  }
}

/**
 * Promote ⟦attachment:id⟧ runs in text nodes into image chips.
 * File @paths are never auto-chipified — only the picker inserts those.
 */
export function chipifyComposerText(
  root: HTMLElement,
  attachments: readonly ComposerAtt[] = [],
) {
  const texts: Text[] = []
  const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT)
  while (walker.nextNode()) texts.push(walker.currentNode as Text)
  for (const textNode of texts) {
    const raw = textNode.textContent ?? ''
    if (!raw.includes('⟦attachment:')) continue
    const attRe = new RegExp(ATTACHMENT_MARKER_RE.source, 'g')
    const frag = document.createDocumentFragment()
    let last = 0
    let touched = false
    for (const match of raw.matchAll(attRe)) {
      const idx = match.index ?? 0
      const id = match[1]
      if (idx > last) frag.appendChild(document.createTextNode(raw.slice(last, idx)))
      const att = id ? attById(attachments, id) : undefined
      if (att?.dataUrl) {
        frag.appendChild(makeAttachmentChip(att))
        touched = true
      } else {
        frag.appendChild(document.createTextNode(match[0]))
      }
      last = idx + match[0].length
    }
    if (!touched) continue
    if (last < raw.length) frag.appendChild(document.createTextNode(raw.slice(last)))
    textNode.parentNode?.replaceChild(frag, textNode)
  }
}

function inlineOffsetIn(container: HTMLElement, anchor: Node, anchorOffset: number): number | null {
  let offset = 0
  const walk = (node: Node): boolean => {
    if (node === anchor) {
      if (node.nodeType === Node.TEXT_NODE) {
        offset += Math.min(anchorOffset, (node.textContent ?? '').length)
        return true
      }
      const el = node as HTMLElement
      const limit = Math.min(anchorOffset, el.childNodes.length)
      for (let i = 0; i < limit; i++) {
        offset += serializeComposerNode(el.childNodes[i]!)
      }
      return true
    }
    if (node.nodeType === Node.TEXT_NODE) {
      offset += (node.textContent ?? '').length
      return false
    }
    if (node.nodeType !== Node.ELEMENT_NODE) return false
    const el = node as HTMLElement
    if (el.dataset.filePath || el.dataset.attachmentId || el.dataset.skillName || el.dataset.terminalChip || el.dataset.artifactChip || el.dataset.promptSlash) {
      offset += chipWireLen(el)
      return false
    }
    if (el.tagName === 'BR') return false
    for (const child of el.childNodes) {
      if (walk(child)) return true
    }
    return false
  }
  for (const child of container.childNodes) {
    if (walk(child)) return offset
  }
  return null
}

function serializeComposerNode(node: Node): number {
  if (node.nodeType === Node.TEXT_NODE) return (node.textContent ?? '').length
  if (node.nodeType !== Node.ELEMENT_NODE) return 0
  const el = node as HTMLElement
  if (el.dataset.filePath || el.dataset.attachmentId || el.dataset.skillName || el.dataset.terminalChip || el.dataset.artifactChip || el.dataset.promptSlash) return chipWireLen(el)
  if (el.tagName === 'BR') return 0
  let n = 0
  for (const child of el.childNodes) n += serializeComposerNode(child)
  return n
}

function blockContaining(node: Node | null, root: HTMLElement): HTMLElement | null {
  for (let n: Node | null = node; n && n !== root; n = n.parentNode) {
    if (isBlockEl(n) && n.parentNode === root) return n as HTMLElement
  }
  return null
}

/** Serialized-string caret offset for the current selection inside root. */
export function composerCaretOffset(root: HTMLElement): number {
  const sel = window.getSelection()
  if (!sel || sel.rangeCount === 0) return serializeComposer(root).length
  const anchor = sel.anchorNode
  if (!anchor || !root.contains(anchor)) return serializeComposer(root).length

  const blocks = ensureParagraphs(root)
  if (!blocks.length) {
    const flat = inlineOffsetIn(root, anchor, sel.anchorOffset)
    return flat ?? serializeComposer(root).length
  }

  const block = blockContaining(anchor, root)
  let offset = 0
  for (let i = 0; i < blocks.length; i++) {
    if (i > 0) offset += 1
    const p = blocks[i]!
    if (p === block) {
      const inner = inlineOffsetIn(p, anchor, sel.anchorOffset)
      return offset + (inner ?? serializeInline(p).length)
    }
    offset += serializeInline(p).length
  }
  return offset
}

/**
 * WebKit paints the caret at the far right of the line if it sits via
 * setStartAfter(chip) with no text node to live in. Always park inside a
 * following Text — create an empty one when needed.
 */
function placeCaretAfterChip(chip: HTMLElement, at: 'start' | 'end' = 'start') {
  const sel = window.getSelection()
  if (!sel) return
  let text: Text
  const next = chip.nextSibling
  if (next?.nodeType === Node.TEXT_NODE) {
    text = next as Text
  } else {
    text = document.createTextNode('')
    chip.after(text)
  }
  const range = document.createRange()
  const off = at === 'end' ? (text.textContent ?? '').length : 0
  range.setStart(text, off)
  range.collapse(true)
  sel.removeAllRanges()
  sel.addRange(range)
}

/** Place caret inside a text node (not setStartAfter — that also jumps right). */
function placeCaretInText(text: Text, offset: number) {
  const sel = window.getSelection()
  if (!sel) return
  const range = document.createRange()
  range.setStart(text, Math.max(0, Math.min(offset, (text.textContent ?? '').length)))
  range.collapse(true)
  sel.removeAllRanges()
  sel.addRange(range)
}

function placeCaretInline(sel: Selection, container: HTMLElement, offset: number) {
  let remaining = Math.max(0, offset)
  const place = (node: Node): boolean => {
    if (node.nodeType === Node.TEXT_NODE) {
      const len = (node.textContent ?? '').length
      if (remaining <= len) {
        const range = document.createRange()
        range.setStart(node, remaining)
        range.collapse(true)
        sel.removeAllRanges()
        sel.addRange(range)
        return true
      }
      remaining -= len
      return false
    }
    if (node.nodeType !== Node.ELEMENT_NODE) return false
    const el = node as HTMLElement
    if (el.dataset.filePath || el.dataset.attachmentId || el.dataset.skillName || el.dataset.terminalChip || el.dataset.artifactChip || el.dataset.promptSlash) {
      const len = chipWireLen(el)
      if (remaining <= len) {
        // Caret “inside” the atomic chip → park in the text after it.
        placeCaretAfterChip(el, 'start')
        return true
      }
      remaining -= len
      return false
    }
    if (el.tagName === 'BR') return false
    for (const child of el.childNodes) {
      if (place(child)) return true
    }
    return false
  }
  for (const child of container.childNodes) {
    if (place(child)) return
  }
  // End of block: if it ends with a chip, park in text after it.
  const last = lastRealChild(container)
  if (isComposerChip(last)) {
    placeCaretAfterChip(last, 'end')
    return
  }
  // Empty <p><br></p> (or offset 0 with only fillers): park at start of the
  // paragraph — NOT selectNodeContents/collapse(false) after <br>, and never
  // leave the selection on the contenteditable root (WebKit paints that caret
  // above the line; the next keystroke inserts a text node before the <p>).
  if (!last) {
    placeCaretAtBlockStart(sel, container)
    return
  }
  const range = document.createRange()
  range.selectNodeContents(container)
  range.collapse(false)
  sel.removeAllRanges()
  sel.addRange(range)
}

function placeCaretAtBlockStart(sel: Selection, container: HTMLElement) {
  const range = document.createRange()
  range.setStart(container, 0)
  range.collapse(true)
  sel.removeAllRanges()
  sel.addRange(range)
}

/**
 * After hydrate / focus, ensure the live selection sits inside a <p>.
 * Clearing the composer replaces DOM nodes and leaves WebKit’s selection on the
 * root (before the empty paragraph) — high caret + first character inserted
 * outside the <p>.
 */
export function ensureComposerCaret(root: HTMLElement, offset?: number) {
  const blocks = ensureParagraphs(root)
  if (!blocks.length) {
    const p = document.createElement('p')
    p.appendChild(document.createElement('br'))
    root.appendChild(p)
  }
  const sel = window.getSelection()
  const inside = !!(
    sel?.rangeCount &&
    sel.focusNode &&
    root.contains(sel.focusNode) &&
    blockContaining(sel.focusNode, root)
  )
  if (inside && offset === undefined) return
  const at = offset ?? serializeComposer(root).length
  setComposerCaret(root, at)
}

/** Place the caret at a serialized offset (after hydrate / chip insert). */
export function setComposerCaret(root: HTMLElement, offset: number) {
  const sel = window.getSelection()
  if (!sel) return
  let remaining = Math.max(0, offset)
  const blocks = ensureParagraphs(root)
  if (!blocks.length) {
    placeCaretInline(sel, root, remaining)
    return
  }
  for (let i = 0; i < blocks.length; i++) {
    if (i > 0) {
      if (remaining <= 0) {
        placeCaretInline(sel, blocks[i]!, 0)
        return
      }
      remaining -= 1
    }
    const lineLen = serializeInline(blocks[i]!).length
    if (remaining <= lineLen) {
      placeCaretInline(sel, blocks[i]!, remaining)
      return
    }
    remaining -= lineLen
  }
  const last = blocks[blocks.length - 1]!
  placeCaretInline(sel, last, serializeInline(last).length)
}

/**
 * Delete a serialized range [start, end) from the composer by rehydrating.
 * Returns the new serialized text.
 */
export function replaceComposerRange(
  root: HTMLElement,
  text: string,
  start: number,
  end: number,
  insert: string,
  attachments: readonly ComposerAtt[] = [],
  opts: { chipFiles?: boolean; chipSkills?: boolean } = {},
): string {
  const next = text.slice(0, start) + insert + text.slice(end)
  hydrateComposer(root, next, attachments, opts)
  return next
}

function lastParagraph(root: HTMLElement): HTMLElement {
  const blocks = ensureParagraphs(root)
  if (blocks.length) return blocks[blocks.length - 1]!
  const p = document.createElement('p')
  p.appendChild(document.createElement('br'))
  root.appendChild(p)
  return p
}

/** Insert a chip at the current caret (or append). Always stays inside a <p>. */
export function insertComposerChip(
  root: HTMLElement,
  chip: HTMLElement,
  opts: { trailingSpace?: boolean } = {},
) {
  const trailingSpace = opts.trailingSpace !== false
  root.focus()
  const sel = window.getSelection()
  if (!sel?.rangeCount || !sel.anchorNode || !root.contains(sel.anchorNode)) {
    const p = lastParagraph(root)
    if (p.childNodes.length === 1 && (p.firstChild as HTMLElement)?.tagName === 'BR') {
      p.replaceChildren()
    }
    p.appendChild(chip)
    if (trailingSpace) {
      const space = document.createTextNode(' ')
      p.appendChild(space)
      placeCaretInText(space, 1)
    } else {
      placeCaretAfterChip(chip, 'start')
    }
    return
  }
  const range = sel.getRangeAt(0)
  // If caret is on the root (between paragraphs), drop into the nearest <p>.
  if (range.startContainer === root) {
    const idx = Math.min(range.startOffset, root.childNodes.length)
    const at = root.childNodes[idx] ?? root.childNodes[idx - 1] ?? null
    const p = isBlockEl(at) ? at as HTMLElement : lastParagraph(root)
    range.selectNodeContents(p)
    range.collapse(true)
  }
  range.deleteContents()
  range.insertNode(chip)
  if (trailingSpace) {
    const space = document.createTextNode(' ')
    chip.after(space)
    placeCaretInText(space, 1)
  } else {
    placeCaretAfterChip(chip, 'start')
  }
}

/**
 * Split the current paragraph at the caret (Shift+Enter). Returns true if handled.
 */
export function insertComposerParagraphBreak(root: HTMLElement): boolean {
  const sel = window.getSelection()
  if (!sel?.rangeCount || !sel.isCollapsed) return false
  if (!root.contains(sel.focusNode)) return false
  const block = blockContaining(sel.focusNode, root) ?? lastParagraph(root)
  const range = sel.getRangeAt(0)

  const after = document.createRange()
  after.setStart(range.startContainer, range.startOffset)
  after.setEnd(block, block.childNodes.length)
  const tail = after.extractContents()

  const next = document.createElement('p')
  if (!tail.childNodes.length || ([...tail.childNodes].every((n) => isInlineFiller(n)))) {
    next.appendChild(document.createElement('br'))
  } else {
    next.appendChild(tail)
  }
  // If the head paragraph emptied, keep a caret placeholder.
  if (![...block.childNodes].some((n) => !isInlineFiller(n))) {
    block.replaceChildren(document.createElement('br'))
  }
  block.after(next)

  const caret = document.createRange()
  caret.selectNodeContents(next)
  caret.collapse(true)
  sel.removeAllRanges()
  sel.addRange(caret)
  return true
}

function closestComposerChip(node: Node | null, root: HTMLElement): HTMLElement | null {
  for (let n: Node | null = node; n && n !== root; n = n.parentNode) {
    if (isComposerChip(n)) return n
  }
  return null
}

/** Skip empty/ZWSP text only — real spaces stay so the user can delete them alone. */
function skipEmptyText(node: Node | null, dir: 'prev' | 'next'): Node | null {
  let n = node
  while (n?.nodeType === Node.TEXT_NODE && (!(n.textContent ?? '').length || isZwspOnly(n.textContent ?? ''))) {
    n = dir === 'prev' ? n.previousSibling : n.nextSibling
  }
  return n
}

/** Walk prev/next through inline fillers (BR, ZWSP) — not across paragraphs. */
function skipInlineFillers(node: Node | null, dir: 'prev' | 'next'): Node | null {
  let n = node
  while (n && isInlineFiller(n)) {
    n = dir === 'prev' ? n.previousSibling : n.nextSibling
  }
  return n
}

function isCaretAtStartOf(el: HTMLElement, range: Range): boolean {
  const start = document.createRange()
  start.selectNodeContents(el)
  start.collapse(true)
  return range.compareBoundaryPoints(Range.START_TO_START, start) === 0
}

function isCaretAtEndOf(el: HTMLElement, range: Range): boolean {
  const end = document.createRange()
  end.selectNodeContents(el)
  end.collapse(false)
  return range.compareBoundaryPoints(Range.START_TO_START, end) === 0
}

/** True if anything non-filler exists after the caret inside the same block. */
function hasContentAfterCaretInBlock(range: Range, block: HTMLElement): boolean {
  const after = range.cloneRange()
  after.collapse(true)
  after.setEnd(block, block.childNodes.length)
  const frag = after.cloneContents()
  const walk = (node: Node): boolean => {
    if (isComposerChip(node)) return true
    if (node.nodeType === Node.TEXT_NODE) {
      const t = node.textContent ?? ''
      return t.length > 0 && !isZwspOnly(t)
    }
    if (node.nodeType === Node.ELEMENT_NODE) {
      const el = node as HTMLElement
      if (el.tagName === 'BR') return false
      for (const c of el.childNodes) {
        if (walk(c)) return true
      }
    }
    return false
  }
  for (const c of frag.childNodes) {
    if (walk(c)) return true
  }
  return false
}

/** Node immediately before the caret (child preceding the collapse point). */
function nodeBeforeCaret(range: Range, root: HTMLElement): Node | null {
  const { startContainer: node, startOffset: offset } = range
  if (node.nodeType === Node.TEXT_NODE) {
    if (offset > 0) return null // caret mid-text — Backspace deletes a char
    return node.previousSibling
  }
  if (node.nodeType === Node.ELEMENT_NODE) {
    if (offset > 0) return node.childNodes[offset - 1] ?? null
    if (node !== root) return node.previousSibling
  }
  return null
}

function chipBeforeCaret(range: Range, root: HTMLElement): HTMLElement | null {
  const { startContainer: node, startOffset: offset } = range
  const enclosing = closestComposerChip(node, root)
  if (enclosing) {
    if (isCaretAtStartOf(enclosing, range)) {
      const prev = skipInlineFillers(enclosing.previousSibling, 'prev')
      return isComposerChip(prev) ? prev : null
    }
    return enclosing
  }

  if (node.nodeType === Node.TEXT_NODE && offset > 0) return null

  const block = blockContaining(node, root)
  const prev = nodeBeforeCaret(range, root)
  const direct = skipEmptyText(prev, 'prev')
  if (isComposerChip(direct)) return direct

  // Trailing WebKit <br> after a chip in the same paragraph.
  if (
    prev &&
    isInlineFiller(skipEmptyText(prev, 'prev') ?? prev) &&
    block &&
    !hasContentAfterCaretInBlock(range, block)
  ) {
    const through = skipInlineFillers(prev, 'prev')
    if (isComposerChip(through)) return through
  }
  return null
}

function chipAfterCaret(range: Range, root: HTMLElement): HTMLElement | null {
  const { startContainer: node, startOffset: offset } = range
  const enclosing = closestComposerChip(node, root)
  if (enclosing) {
    if (isCaretAtEndOf(enclosing, range)) {
      const next = skipInlineFillers(enclosing.nextSibling, 'next')
      return isComposerChip(next) ? next : null
    }
    return enclosing
  }
  if (node.nodeType === Node.TEXT_NODE) {
    const len = node.textContent?.length ?? 0
    if (offset < len) return null
    const next = skipInlineFillers(node.nextSibling, 'next')
    return isComposerChip(next) ? next : null
  }
  if (node.nodeType === Node.ELEMENT_NODE) {
    const next = skipInlineFillers(node.childNodes[offset] ?? null, 'next')
    if (isComposerChip(next)) return next
    if (offset >= node.childNodes.length && node !== root && !isBlockEl(node)) {
      const after = skipInlineFillers(node.nextSibling, 'next')
      return isComposerChip(after) ? after : null
    }
  }
  return null
}

function removeComposerChipAt(chip: HTMLElement): HTMLElement {
  const parent = chip.parentNode as HTMLElement | null
  const index = parent ? Array.prototype.indexOf.call(parent.childNodes, chip) : -1
  if (parent) {
    let n = chip.nextSibling
    while (n && isInlineFiller(n)) {
      const dead = n
      n = n.nextSibling
      dead.parentNode?.removeChild(dead)
    }
  }
  const prev = chip.previousSibling
  chip.remove()
  if (parent && isBlockEl(parent) && ![...parent.childNodes].some((c) => !isInlineFiller(c))) {
    parent.replaceChildren(document.createElement('br'))
    const sel = window.getSelection()
    if (sel) {
      const caret = document.createRange()
      caret.setStart(parent, 0)
      caret.collapse(true)
      sel.removeAllRanges()
      sel.addRange(caret)
    }
    return chip
  }
  // Prefer sitting in a neighboring text node — never leave caret on the <p>
  // boundary after a chip (WebKit far-right bug).
  if (prev?.nodeType === Node.TEXT_NODE) {
    placeCaretInText(prev as Text, (prev.textContent ?? '').length)
  } else if (parent?.childNodes[index]?.nodeType === Node.TEXT_NODE) {
    placeCaretInText(parent.childNodes[index] as Text, 0)
  } else if (isComposerChip(prev)) {
    placeCaretAfterChip(prev, 'start')
  } else if (parent && index >= 0) {
    const sel = window.getSelection()
    if (sel) {
      const at = parent.childNodes[index] ?? null
      const caret = document.createRange()
      if (at) caret.setStartBefore(at)
      else caret.setStart(parent, parent.childNodes.length)
      caret.collapse(true)
      sel.removeAllRanges()
      sel.addRange(caret)
    }
  }
  return chip
}

/**
 * Collect inline nodes into line buckets, splitting on blocks / <br> / \\n in text.
 * Moves live chip nodes (does not recreate from @path).
 */
function extractLines(root: HTMLElement): Node[][] {
  const lines: Node[][] = []
  let cur: Node[] = []
  const flush = () => {
    lines.push(cur)
    cur = []
  }

  const takeText = (raw: string) => {
    const parts = raw.split('\n')
    for (let i = 0; i < parts.length; i++) {
      if (i > 0) flush()
      if (parts[i]) cur.push(document.createTextNode(parts[i]!))
    }
  }

  const absorb = (node: Node) => {
    if (isComposerChip(node)) {
      cur.push(node)
      return
    }
    if (node.nodeType === Node.TEXT_NODE) {
      takeText(node.textContent ?? '')
      return
    }
    if (node.nodeType !== Node.ELEMENT_NODE) return
    const el = node as HTMLElement
    if (el.tagName === 'BR') {
      flush()
      return
    }
    if (isBlockEl(el)) {
      // Block boundary: absorb children as their own line(s), then end the line.
      if (cur.length) flush()
      const before = lines.length
      for (const c of [...el.childNodes]) absorb(c)
      // Empty block → blank line.
      if (lines.length === before && !cur.length) flush()
      else if (cur.length) flush()
      return
    }
    // Unknown inline wrapper — lift children.
    for (const c of [...(el as HTMLElement).childNodes]) absorb(c)
  }

  for (const child of [...root.childNodes]) absorb(child)
  if (cur.length || !lines.length) flush()
  return lines
}

function cleanLine(nodes: Node[]): Node[] {
  const out: Node[] = []
  for (const n of nodes) {
    if (n.nodeType === Node.TEXT_NODE) {
      const t = n.textContent ?? ''
      if (!t || isZwspOnly(t)) continue
      out.push(n)
      continue
    }
    if (isInlineFiller(n)) continue // drop mid-line BRs; empty lines get a fresh <br>
    out.push(n)
  }
  // Trailing inline fillers already dropped; if line ends with chip-only junk, fine.
  return out
}

/** True when the composer has loose text/div/br structure that should be wrapped into <p>s. */
function composerDomIsDirty(root: HTMLElement): boolean {
  if (!root.childNodes.length) return true
  for (const child of root.childNodes) {
    if (child.nodeType === Node.TEXT_NODE) {
      if (child.textContent) return true
      continue
    }
    if (child.nodeType !== Node.ELEMENT_NODE) continue
    const el = child as HTMLElement
    if (el.tagName === 'BR' || el.tagName === 'DIV') return true
    // Chip as a direct child of root (not wrapped in <p>).
    if (el.dataset.filePath || el.dataset.attachmentId || el.dataset.skillName || el.dataset.terminalChip || el.dataset.artifactChip || el.dataset.promptSlash) return true
    if (el.tagName !== 'P') return true
    // Nested blocks / literal newlines inside a paragraph.
    for (const n of el.childNodes) {
      if (isBlockEl(n)) return true
      if (n.nodeType === Node.TEXT_NODE && (n.textContent ?? '').includes('\n')) return true
    }
  }
  return false
}

/** Last non-filler child of a paragraph (chips / real text). */
function lastRealChild(p: HTMLElement): Node | null {
  let n: Node | null = p.lastChild
  while (
    n &&
    (isInlineFiller(n) ||
      (n.nodeType === Node.TEXT_NODE && !(n.textContent ?? '').length))
  ) {
    n = n.previousSibling
  }
  return n
}

function promptArgInputHasFocus(root: HTMLElement): boolean {
  const ae = document.activeElement as HTMLElement | null
  return !!ae && ae.classList.contains('chat-composer-prompt-arg-input') && root.contains(ae)
}

/**
 * After deleting the space following a chip, WebKit leaves a <br>; sitting on
 * that (or on setStartAfter(chip)) paints the caret at the far right. Strip
 * trailing <br>s, keep/create a text node as the caret holder, park inside it.
 */
function tidyCaretAfterChip(root: HTMLElement) {
  // Nested <input>s live in the chip; window.getSelection() still points at the
  // contenteditable. Parking the caret would blur the arg field on every key.
  if (promptArgInputHasFocus(root)) return
  let pin: HTMLElement | null = null
  for (const p of ensureParagraphs(root)) {
    const real = lastRealChild(p)
    if (!isComposerChip(real)) continue
    // Drop trailing <br> / ZWSP after the chip — keep a real Text holder.
    let n = real.nextSibling
    while (n) {
      const next = n.nextSibling
      if (n.nodeType === Node.ELEMENT_NODE && (n as HTMLElement).tagName === 'BR') {
        n.parentNode?.removeChild(n)
        pin = real
      } else if (n.nodeType === Node.TEXT_NODE && isZwspOnly(n.textContent ?? '')) {
        n.parentNode?.removeChild(n)
        pin = real
      } else {
        break
      }
      n = next
    }
  }

  const sel = window.getSelection()
  if (!sel?.rangeCount || !sel.isCollapsed) {
    if (pin) placeCaretAfterChip(pin, 'start')
    return
  }
  if (!root.contains(sel.focusNode)) {
    if (pin) placeCaretAfterChip(pin, 'start')
    return
  }

  const range = sel.getRangeAt(0)
  const block = blockContaining(range.startContainer, root)
  if (!block) {
    if (pin) placeCaretAfterChip(pin, 'start')
    return
  }
  const real = lastRealChild(block)
  if (!isComposerChip(real)) return

  // Caret stuck after a chip with no real following text (or we just stripped
  // a <br>) — park inside a text node, never setStartAfter(chip).
  const next = real.nextSibling
  const inTextAfter =
    range.startContainer.nodeType === Node.TEXT_NODE &&
    range.startContainer === next
  if (pin || !inTextAfter || !hasContentAfterCaretInBlock(range, block)) {
    // If a trailing space remains, sit after it; otherwise at start of holder.
    if (next?.nodeType === Node.TEXT_NODE && (next.textContent ?? '').startsWith(' ')) {
      placeCaretInText(next as Text, 1)
    } else {
      placeCaretAfterChip(real, 'start')
    }
  }
}

/**
 * Ensure each file/skill chip has a whitespace boundary before it in the DOM
 * so serialize → FILE_AT_RE / collectFileMentions keep treating it as a mention.
 * Re-inserts a space when the user deletes the one before a chip (or parks two
 * chips flush against each other).
 */
export function ensureChipBoundariesInDom(root: HTMLElement) {
  const blocks = ensureParagraphs(root)
  const lines = blocks.length ? blocks : [root]
  for (const block of lines) {
    for (const node of [...block.childNodes]) {
      if (!isComposerChip(node)) continue
      const el = node as HTMLElement
      if (!el.dataset.filePath && !el.dataset.skillName && !el.dataset.terminalChip && !el.dataset.artifactChip) continue
      const prev = skipInlineFillers(node.previousSibling, 'prev')
      if (prev) {
        if (prev.nodeType === Node.TEXT_NODE) {
          const t = prev.textContent ?? ''
          if (t.length > 0 && !/\s$/.test(t)) prev.textContent = `${t} `
        } else if (isComposerChip(prev)) {
          block.insertBefore(document.createTextNode(' '), node)
        }
      }
      // Terminal / artifact chips also need a trailing boundary — their regexes
      // / Go collectors require (?=\s|$) after the token.
      if (!el.dataset.terminalChip && !el.dataset.artifactChip) continue
      const next = skipInlineFillers(node.nextSibling, 'next')
      if (!next) continue
      if (next.nodeType === Node.TEXT_NODE) {
        const t = next.textContent ?? ''
        if (t.length > 0 && !/^\s/.test(t)) next.textContent = ` ${t}`
      } else if (isComposerChip(next)) {
        block.insertBefore(document.createTextNode(' '), next)
      }
    }
  }
}

/**
 * Normalize to one <p> per line; strip WebKit empty div/br junk.
 * Preserves live chips (no @path re-chipify). No-ops when already clean so
 * routine typing/backspace doesn’t rebuild the DOM every keystroke.
 */
export function normalizeComposerDom(root: HTMLElement) {
  ensureChipBoundariesInDom(root)
  if (promptArgInputHasFocus(root)) return
  if (!composerDomIsDirty(root)) {
    tidyCaretAfterChip(root)
    return
  }

  const hadFocus = root.contains(window.getSelection()?.focusNode ?? null)
  const offset = hadFocus ? composerCaretOffset(root) : 0

  const lines = extractLines(root).map(cleanLine)
  // Blank lines are intentional. Ensure at least one paragraph.
  if (!lines.length) lines.push([])

  root.replaceChildren()
  for (const line of lines) {
    const p = document.createElement('p')
    if (!line.length) {
      p.appendChild(document.createElement('br'))
    } else {
      for (const n of line) p.appendChild(n)
    }
    root.appendChild(p)
  }

  if (hadFocus) {
    setComposerCaret(root, Math.min(offset, serializeComposer(root).length))
  }
  tidyCaretAfterChip(root)
}

/**
 * True when the caret is inside or flush after a file chip. The chip serializes
 * to `@path`, so mentionQueryAt would otherwise treat that as an in-progress
 * @query once the trailing space is gone.
 */
export function isCaretAfterFileChip(root: HTMLElement): boolean {
  return isCaretAfterChipKind(root, 'filePath')
}

/** Same for skill chips (`/name`) vs skillQueryAt. */
export function isCaretAfterSkillChip(root: HTMLElement): boolean {
  return isCaretAfterChipKind(root, 'skillName')
}

/** Same for @terminal chips vs mentionQueryAt (`@terminal` looks like a query). */
export function isCaretAfterTerminalChip(root: HTMLElement): boolean {
  return isCaretAfterChipKind(root, 'terminalChip')
}

/** Same for @artifact chips vs mentionQueryAt (`@artifact` looks like a query). */
export function isCaretAfterArtifactChip(root: HTMLElement): boolean {
  return isCaretAfterChipKind(root, 'artifactChip')
}

function isCaretAfterChipKind(root: HTMLElement, kind: 'filePath' | 'skillName' | 'terminalChip' | 'artifactChip'): boolean {
  const sel = window.getSelection()
  if (!sel?.rangeCount || !sel.isCollapsed) return false
  if (!root.contains(sel.focusNode)) return false
  const range = sel.getRangeAt(0)

  const enclosing = closestComposerChip(range.startContainer, root)
  if (enclosing?.dataset[kind]) {
    return !isCaretAtStartOf(enclosing, range)
  }

  if (range.startContainer.nodeType === Node.TEXT_NODE && range.startOffset > 0) {
    return false
  }

  const prev = skipInlineFillers(nodeBeforeCaret(range, root), 'prev')
  return !!(prev && isComposerChip(prev) && prev.dataset[kind])
}

/**
 * If Backspace/Delete would hit a chip, remove the whole chip in one keypress
 * (browsers otherwise nibble oddly around contenteditable=false nodes).
 * Returns the removed chip, or null if the key should fall through.
 */
export function deleteAdjacentComposerChip(
  root: HTMLElement,
  key: 'Backspace' | 'Delete',
): HTMLElement | null {
  // Nested prompt <input>s own Backspace/Delete. window.getSelection() still
  // points at the contenteditable=false chip, so treating that as "adjacent"
  // would wipe /linear (and its typed args) on every character delete.
  if (promptArgInputHasFocus(root)) return null
  // Do not normalize/rebuild here — this runs on every Backspace via keydown and
  // beforeinput. Replacing <p> nodes invalidates the browser’s delete target so
  // normal character deletion silently fails.
  const sel = window.getSelection()
  if (!sel?.rangeCount || !sel.isCollapsed) return null
  if (!root.contains(sel.focusNode)) return null
  const range = sel.getRangeAt(0)
  const chip = key === 'Backspace' ? chipBeforeCaret(range, root) : chipAfterCaret(range, root)
  if (!chip || !root.contains(chip)) return null
  return removeComposerChipAt(chip)
}

/** Min/max composer chrome: 2 lines → grow through 6, then scroll. */
const COMPOSER_LINE_PX = 12.5 * 1.4
const COMPOSER_PAD_PX = 16 // padding 8px top + bottom
export const COMPOSER_MIN_HEIGHT_PX = Math.round(COMPOSER_PAD_PX + COMPOSER_LINE_PX * 2)
export const COMPOSER_MAX_HEIGHT_PX = Math.round(COMPOSER_PAD_PX + COMPOSER_LINE_PX * 6)

/**
 * Grow the composer with its content up to ~6 lines, then keep overflow:auto.
 * Call after input / hydrate / chip insert so height tracks the text.
 */
export function resizeComposerHeight(root: HTMLElement) {
  // Collapse first so scrollHeight reflects content, not the previous box.
  root.style.height = `${COMPOSER_MIN_HEIGHT_PX}px`
  const next = Math.min(
    Math.max(root.scrollHeight, COMPOSER_MIN_HEIGHT_PX),
    COMPOSER_MAX_HEIGHT_PX,
  )
  root.style.height = `${next}px`
}

/** Viewport rect of the collapsed caret in a contenteditable composer. */
export function composerCaretClientRect(root: HTMLElement): DOMRect | null {
  const sel = window.getSelection()
  if (!sel?.rangeCount || !root.contains(sel.focusNode)) return null
  const range = sel.getRangeAt(0).cloneRange()
  range.collapse(false)
  const rects = range.getClientRects()
  const rect = rects.length ? rects[rects.length - 1]! : range.getBoundingClientRect()
  if (!rect || (rect.width === 0 && rect.height === 0 && rect.top === 0 && rect.bottom === 0)) return null
  return rect
}

/**
 * Scroll the composer's overflow box so the caret stays visible — needed after
 * long pastes (contenteditable does not do this on its own).
 */
export function scrollComposerCaretIntoView(root: HTMLElement) {
  const rect = composerCaretClientRect(root)
  if (!rect) {
    root.scrollTop = root.scrollHeight
    return
  }
  const box = root.getBoundingClientRect()
  const pad = 6
  if (rect.bottom > box.bottom - pad) root.scrollTop += rect.bottom - box.bottom + pad
  else if (rect.top < box.top + pad) root.scrollTop -= box.top - rect.top + pad
}
