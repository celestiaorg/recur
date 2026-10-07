import { type MacroContext, type MacroResult } from '@recur/sdk'

function words(s: string): string[] {
  return s
    .replace(/([a-z0-9])([A-Z])/g, "$1_$2")
    .replace(/([A-Z]+)([A-Z][a-z])/g, "$1_$2")
    .split(/[_-]+/)
    .filter(Boolean)
    .map((w) => w.toLowerCase())
}

function toCamel(s: string): string {
  const w = words(s)
  if (!w.length) return s
  return w[0] + w.slice(1).map((x) => x.charAt(0).toUpperCase() + x.slice(1)).join("")
}

/** Convert identifiers to camelCase (selection, else current word). */
export default function (ctx: MacroContext): MacroResult {
  const text = ctx.input.replace(/[A-Za-z_][A-Za-z0-9_-]*/g, toCamel)
  return { text, output: "replaceInput" }
}
