/** Minimal TOML tokenizer for the splash code samples. */
export type Token = { kind: string; text: string };

const pattern =
  /(#[^\n]*)|("""[\s\S]*?"""|"(?:[^"\\\n]|\\.)*")|(^\[\[?[^\]\n]+\]\]?)|(^[A-Za-z_][\w.-]*)(?=\s*=)|(\b\d+\b|\btrue\b|\bfalse\b)/gm;
const kinds = ["comment", "string", "table", "key", "literal"];

export function highlightToml(source: string): Token[] {
  const tokens: Token[] = [];
  let last = 0;
  for (const match of source.matchAll(pattern)) {
    if (match.index > last)
      tokens.push({ kind: "plain", text: source.slice(last, match.index) });
    const group = match.findIndex((g, i) => i > 0 && g !== undefined);
    tokens.push({ kind: kinds[group - 1], text: match[0] });
    last = match.index + match[0].length;
  }
  if (last < source.length)
    tokens.push({ kind: "plain", text: source.slice(last) });
  return tokens;
}
