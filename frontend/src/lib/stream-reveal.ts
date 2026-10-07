/** Paint through the last newline. No newline or `complete` paints the whole block. */
export function completeStreamPrefix(text: string, revealed: number, complete = false): number {
  if (revealed >= text.length || complete) return text.length
  const nl = text.lastIndexOf('\n')
  if (nl < 0) return text.length
  if (nl < revealed) return revealed
  return nl + 1
}
