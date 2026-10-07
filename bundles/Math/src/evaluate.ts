import { replaceEdited, type MacroContext, type MacroResult } from '@recur/sdk'

export default function (ctx: MacroContext): MacroResult {
  const src = ctx.input.trim()
  if (!src) throw new Error("Nothing to evaluate")
  return replaceEdited(ctx, formatNum(evaluate(src)))
}

function formatNum(n: number): string {
  if (!Number.isFinite(n)) throw new Error("Result is not a finite number")
  if (Object.is(n, -0)) return "0"
  if (Number.isInteger(n) && Math.abs(n) <= Number.MAX_SAFE_INTEGER) return String(n)
  return String(Number(n.toPrecision(12)))
}

function evaluate(src: string): number {
  const p = new Parser(src)
  const n = p.parseExpr()
  p.skipWs()
  if (p.i < p.s.length) throw new Error(`Unexpected ${JSON.stringify(p.s.slice(p.i))}`)
  return n
}

class Parser {
  s: string
  i = 0
  constructor(s: string) {
    this.s = s
  }

  skipWs() {
    while (this.i < this.s.length && /\s/.test(this.s[this.i]!)) this.i++
  }

  parseExpr(): number {
    let n = this.parseTerm()
    for (;;) {
      this.skipWs()
      const op = this.s[this.i]
      if (op !== "+" && op !== "-") return n
      this.i++
      const r = this.parseTerm()
      n = op === "+" ? n + r : n - r
    }
  }

  parseTerm(): number {
    let n = this.parsePower()
    for (;;) {
      this.skipWs()
      const op = this.s[this.i]
      if (op !== "*" && op !== "/" && op !== "%") return n
      this.i++
      const r = this.parsePower()
      if (op === "*") n *= r
      else if (op === "/") n /= r
      else n %= r
    }
  }

  parsePower(): number {
    const n = this.parseUnary()
    this.skipWs()
    if (this.s[this.i] === "^" || this.s.slice(this.i, this.i + 2) === "**") {
      this.i += this.s[this.i] === "^" ? 1 : 2
      return n ** this.parsePower()
    }
    return n
  }

  parseUnary(): number {
    this.skipWs()
    if (this.s[this.i] === "+") {
      this.i++
      return this.parseUnary()
    }
    if (this.s[this.i] === "-") {
      this.i++
      return -this.parseUnary()
    }
    return this.parsePrimary()
  }

  parsePrimary(): number {
    this.skipWs()
    if (this.s[this.i] === "(") {
      this.i++
      const n = this.parseExpr()
      this.skipWs()
      if (this.s[this.i] !== ")") throw new Error("Missing )")
      this.i++
      return n
    }
    return this.parseNumber()
  }

  parseNumber(): number {
    this.skipWs()
    const rest = this.s.slice(this.i)
    const hex = rest.match(/^0x[0-9a-fA-F]+/)
    if (hex) {
      this.i += hex[0].length
      return parseInt(hex[0], 16)
    }
    const bin = rest.match(/^0b[01]+/)
    if (bin) {
      this.i += bin[0].length
      return parseInt(bin[0].slice(2), 2)
    }
    const oct = rest.match(/^0o[0-7]+/)
    if (oct) {
      this.i += oct[0].length
      return parseInt(oct[0].slice(2), 8)
    }
    const dec = rest.match(/^\d+(?:\.\d+)?(?:[eE][+-]?\d+)?/)
    if (!dec) throw new Error(`Expected number at ${JSON.stringify(rest.slice(0, 16))}`)
    this.i += dec[0].length
    return Number(dec[0])
  }
}
