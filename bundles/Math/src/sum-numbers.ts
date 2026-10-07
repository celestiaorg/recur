import { replaceEdited, type MacroContext, type MacroResult } from '@recur/sdk'

const NUM = /[+-]?(?:0x[0-9a-fA-F]+|0b[01]+|0o[0-7]+|\d+\.?\d*(?:e[+-]?\d+)?)/gi

export default function (ctx: MacroContext): MacroResult {
  const nums = collect(ctx.input)
  if (!nums.length) throw new Error("No numbers in selection")
  const total = nums.reduce((a, b) => a + b, 0)
  return replaceEdited(ctx, formatNum(total))
}

function collect(s: string): number[] {
  const out: number[] = []
  const re = new RegExp(NUM.source, "gi")
  let m: RegExpExecArray | null
  while ((m = re.exec(s))) {
    const raw = m[0]
    if (/^0x/i.test(raw)) out.push(parseInt(raw, 16))
    else if (/^0b/i.test(raw)) out.push(parseInt(raw.slice(2), 2))
    else if (/^0o/i.test(raw)) out.push(parseInt(raw.slice(2), 8))
    else out.push(Number(raw))
  }
  return out
}

function formatNum(n: number): string {
  if (!Number.isFinite(n)) throw new Error("Result is not a finite number")
  if (Object.is(n, -0)) return "0"
  if (Number.isInteger(n) && Math.abs(n) <= Number.MAX_SAFE_INTEGER) return String(n)
  return String(Number(n.toPrecision(12)))
}
