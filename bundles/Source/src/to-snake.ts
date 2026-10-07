import { type MacroContext, type MacroResult } from '@recur/sdk'

function words(s: string): string[] {
  return s
    .replace(/([a-z0-9])([A-Z])/g, "$1_$2")
    .replace(/([A-Z]+)([A-Z][a-z])/g, "$1_$2")
    .split(/[_-]+/)
    .filter(Boolean)
    .map((w) => w.toLowerCase())
}

function toSnake(s: string): string {
  return words(s).join("_")
}

/** Convert identifiers to snake_case (selection, else current word). */
export default function (ctx: MacroContext): MacroResult {
  const text = ctx.input.replace(/[A-Za-z_][A-Za-z0-9_-]*/g, toSnake)
  return { text, output: "replaceInput" }
}
