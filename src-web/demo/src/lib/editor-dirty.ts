import { Text } from '@codemirror/state'

/** CodeMirror stores documents with `\n`; disk may still be CRLF/CR. */
export function normalizeEditorNewlines(s: string): string {
  if (!s.includes('\r')) return s
  return s.replace(/\r\n|\r/g, '\n')
}
