// Recur Bundle macro SDK: import from `@recur/sdk` in command scripts.
// Compile resolves this module via esbuild (embedded twin: internal/bundles/macro_sdk.ts).

export type OutputRouting =
  | "discard"
  | "replaceSelection"
  | "replaceDocument"
  | "replaceInput"
  | "insertText"
  | "insertSnippet"
  | "newDocument"
  | "showAsTooltip"
  | "copyToClipboard"
  | "showAsToast"

export type MacroContext = {
  input: string
  selectedText: string
  document: string
  currentLine: string
  lineNumber: number
  column: number
  currentWord: string
  filepath: string
  filename: string
  scope: string
  clipboard: string
  tabSize: number
  softTabs: boolean
  prompts: Record<string, string>
}

export type MacroResult = string | {
  text: string
  output?: OutputRouting
  /** Scratch-tab language/extension for newDocument (e.g. "diff", "json", "html", "md"). */
  language?: string
  /** Open the scratch on the Preview chip when language supports it (html/md/svg). */
  openInPreview?: boolean
}

/** Context for [[prompt]] chat actions (workspace vars + focused editor snapshot). */
export type PromptContext = {
  workspaceId: string
  projectId: string
  worktree: string
  port: number
  selectedText: string
  document: string
  currentLine: string
  lineNumber: number
  column: number
  currentWord: string
  filepath: string
  filename: string
  scope: string
  clipboard: string
  tabSize: number
  softTabs: boolean
  /** Operator looking-at view: workspace, local_pr, github_pr. */
  surface?: string
  /** Trailing composer remainder after `/slash` (empty when none). */
  args?: string
  /** Named [[prompt.arg]] values. */
  params?: Record<string, string>
}

/**
 * Result of a [[prompt]] action. `prompt` is sent as the user turn text —
 * `@path` and `/skill` tokens inside it are real composer dialect; the host
 * wraps them on send (no pre-wrapping in the script).
 */
export type PromptResult = {
  /** Sent as the user turn. Empty = no-send success (toast `display` only). */
  prompt: string
  /** UI label; default = prompt name; coerced to a single line ≤ 120 chars. */
  display?: string
}

/** Context for an optional [[prompt]] `complete` script. */
export type PromptCompleteContext = {
  /** Value of the focused [[prompt.arg]] slot. */
  args: string
  /** Name of the focused slot (e.g. "issue"). */
  arg?: string
  /** All named [[prompt.arg]] values so far. */
  params?: Record<string, string>
  /** Workspace LSP diagnostics as `path:line:col severity (source): message`. TypeScript complete only. */
  diagnostics?: string[]
}

export type PromptCompleteHit = {
  /** Replaces `ctx.args` (and is what Enter runs). */
  insert: string
  label: string
  detail?: string
  badge?: string
  /** Host-handled, e.g. `settings:integrations`. Empty = insert/run. */
  action?: string
}

export type PromptCompleteResult = {
  hits?: PromptCompleteHit[]
  message?: string
}

export const Output: { readonly [K in OutputRouting]: K } = Object.freeze({
  discard: "discard",
  replaceSelection: "replaceSelection",
  replaceDocument: "replaceDocument",
  replaceInput: "replaceInput",
  insertText: "insertText",
  insertSnippet: "insertSnippet",
  newDocument: "newDocument",
  showAsTooltip: "showAsTooltip",
  copyToClipboard: "copyToClipboard",
  showAsToast: "showAsToast",
})

/** Replace the selection when one exists; otherwise the whole document. */
export function replaceEdited(ctx: MacroContext, text: string): MacroResult {
  return {
    text,
    output: ctx.selectedText ? Output.replaceSelection : Output.replaceDocument,
  }
}

/** Strip line/block comments and trailing commas outside strings (JSONC). */
export function stripJSONC(text: string): string {
  let out = ""
  let i = 0
  const n = text.length
  let inString = false
  let escape = false
  while (i < n) {
    const c = text[i]!
    if (inString) {
      out += c
      if (escape) escape = false
      else if (c === "\\") escape = true
      else if (c === '"') inString = false
      i++
      continue
    }
    if (c === '"') {
      inString = true
      out += c
      i++
      continue
    }
    if (c === "/" && text[i + 1] === "/") {
      i += 2
      while (i < n && text[i] !== "\n" && text[i] !== "\r") i++
      continue
    }
    if (c === "/" && text[i + 1] === "*") {
      i += 2
      while (i < n && !(text[i] === "*" && text[i + 1] === "/")) i++
      if (i < n) i += 2
      continue
    }
    if (c === ",") {
      const j = skipJSONCSpaceAndComments(text, i + 1)
      if (j < n && (text[j] === "}" || text[j] === "]")) {
        i++
        continue
      }
    }
    out += c
    i++
  }
  return out
}

function skipJSONCSpaceAndComments(text: string, i: number): number {
  const n = text.length
  while (i < n) {
    const c = text[i]!
    if (c === " " || c === "\t" || c === "\n" || c === "\r") {
      i++
      continue
    }
    if (c === "/" && text[i + 1] === "/") {
      i += 2
      while (i < n && text[i] !== "\n" && text[i] !== "\r") i++
      continue
    }
    if (c === "/" && text[i + 1] === "*") {
      i += 2
      while (i < n && !(text[i] === "*" && text[i + 1] === "/")) i++
      if (i < n) i += 2
      continue
    }
    break
  }
  return i
}

/** Parse JSON or JSONC (comments + trailing commas). */
export function parseJSONC(text: string): unknown {
  return JSON.parse(stripJSONC(text))
}
