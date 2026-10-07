import { Output, type MacroResult } from '@recur/sdk'

/** TextMate Markdown Hard Linebreak (⌃↩): two trailing spaces then a newline. */
export default function (): MacroResult {
  return { text: '  \n', output: Output.insertText }
}
