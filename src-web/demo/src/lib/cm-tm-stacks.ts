/**
 * Incremental vscode-textmate rule-stack + token checkpoints.
 *
 * Tokenizing a TextMate grammar is stack-based: line N+1 needs the stack from
 * line N. Recur used to restart at line 1 on every viewport paint, so a Svelte
 * buffer scrolled to line 5000 re-tokenized 5000 lines on every scroll frame.
 *
 * After an edit, keep stale stacks so we can stop as soon as a line's ending
 * state matches its cached state — usually 1–3 lines, not the whole viewport.
 */
import { INITIAL, type StateStack } from 'vscode-textmate'

/** Insert `ins` empty slots at `index` after removing `del`, without spreading. */
function spliceHoles<T>(arr: Array<T | undefined>, index: number, del: number, ins: number): Array<T | undefined> {
  if (ins <= 8_192) {
    arr.splice(index, del, ...(ins > 0 ? Array<undefined>(ins) : []))
    return arr
  }
  return arr.slice(0, index).concat(Array<T | undefined>(ins), arr.slice(index + del))
}

export class TmStackCache {
  /** after[n] = stack after tokenizing 1-based line n. Index 0 unused. */
  private after: Array<StateStack | undefined> = []
  /** tokens[n] = tokenizeLine2 pairs for 1-based line n. Index 0 unused. */
  private tokens: Array<ArrayLike<number> | undefined> = []
  /** Highest n such that after[1]…after[n] are filled with no holes. */
  private filledThrough = 0

  get filled(): number {
    return this.filledThrough
  }

  /** Line to start tokenizing from, and the stack entering that line. */
  resumeAt(fromLine: number): { startLine: number; stack: StateStack | null } {
    const before = Math.min(Math.max(fromLine, 1) - 1, this.filledThrough)
    if (before >= 1) {
      const stack = this.after[before]
      if (stack) return { startLine: before + 1, stack }
    }
    return { startLine: 1, stack: INITIAL }
  }

  remember(line: number, stack: StateStack | null, tokens?: ArrayLike<number>): void {
    if (line < 1 || !stack) return
    this.after[line] = stack
    if (tokens) this.tokens[line] = tokens
    if (line === this.filledThrough + 1) this.filledThrough = line
  }

  stackAt(line: number): StateStack | undefined {
    return this.after[line]
  }

  tokensAt(line: number): ArrayLike<number> | undefined {
    return this.tokens[line]
  }

  hasTokens(fromLine: number, toLine: number): boolean {
    if (fromLine > toLine) return true
    for (let n = fromLine; n <= toLine; n++) {
      if (!this.tokens[n]) return false
    }
    return true
  }

  /**
   * Line `n` ended with the same stack as before, so later cached lines are
   * valid again while they still have both a stack and tokens.
   */
  markStable(n: number): void {
    let end = n
    while (this.after[end + 1] && this.tokens[end + 1]) end++
    this.filledThrough = Math.max(this.filledThrough, end)
  }

  /**
   * 1-based splice matching a document line replace. Drops validity at
   * `fromLine`. Same-length replaces keep stale stacks so early-stop can
   * `equals` the previous end state; a line-count change shifts later rows.
   */
  splice(fromLine: number, deleteCount: number, insertCount: number): void {
    const i = Math.max(1, fromLine)
    const del = Math.max(0, deleteCount)
    const ins = Math.max(0, insertCount)
    if (del !== ins) {
      // Not `splice(i, del, ...Array(ins))`: a full-document replace makes
      // `ins` the whole line count, and spreading that many arguments throws
      // RangeError (max call stack) on a large file.
      this.after = spliceHoles(this.after, i, del, ins)
      this.tokens = spliceHoles(this.tokens, i, del, ins)
    } else {
      // Replaced lines keep their stale stacks but not their tokens, so
      // `markStable` cannot carry an early-stop on the first replaced line
      // over the rest of them (a full-document live refresh replaces all).
      for (let n = i; n < i + ins; n++) this.tokens[n] = undefined
    }
    this.filledThrough = Math.min(this.filledThrough, i - 1)
  }
}

export type LineTokenizer = (
  text: string,
  stack: StateStack | null,
) => { tokens: ArrayLike<number>; ruleStack: StateStack }

/**
 * Tokenize `fromLine..toLine` (1-based), resuming from the last checkpoint.
 * When `stopWhenStable` is set, stop after a line whose end stack matches the
 * stale cached stack *and* the rest of the requested range still has tokens.
 */
export function tokenizeLineRange(opts: {
  lineText: (n: number) => string
  cache: TmStackCache
  tokenize: LineTokenizer
  fromLine: number
  toLine: number
  stopWhenStable?: boolean
  budget?: number
  /** performance.now() timestamp to stop after (at least one line still runs). */
  deadlineMs?: number
}): { tokenized: number; stableAt: number | null; lastLine: number } {
  const { cache, tokenize, stopWhenStable } = opts
  const toLine = opts.toLine
  if (toLine < 1 || opts.fromLine > toLine) {
    return { tokenized: 0, stableAt: null, lastLine: toLine }
  }
  const { startLine, stack: startStack } = cache.resumeAt(opts.fromLine)
  let stack = startStack
  let tokenized = 0
  let stableAt: number | null = null
  let lastLine = startLine - 1
  const end =
    opts.budget != null ? Math.min(toLine, startLine + Math.max(0, opts.budget) - 1) : toLine
  for (let n = startLine; n <= end; n++) {
    const stale = stopWhenStable ? cache.stackAt(n) : undefined
    const result = tokenize(opts.lineText(n), stack)
    stack = result.ruleStack
    cache.remember(n, stack, result.tokens)
    tokenized++
    lastLine = n
    if (stopWhenStable && stale && stack.equals(stale)) {
      cache.markStable(n)
      if (cache.hasTokens(n + 1, toLine)) {
        stableAt = n
        break
      }
    }
    if (opts.deadlineMs != null && performance.now() > opts.deadlineMs) break
  }
  return { tokenized, stableAt, lastLine: stableAt ?? lastLine }
}

