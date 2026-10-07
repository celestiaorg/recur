import { Text } from '@codemirror/state'

/** CodeMirror stores documents with `\n`; disk may still be CRLF/CR. */
export function normalizeEditorNewlines(s: string): string {
  if (!s.includes('\r')) return s
  return s.replace(/\r\n|\r/g, '\n')
}

/** True when the two buffers are the same document, ignoring CR vs LF. */
export function editorTextEq(a: string, b: string): boolean {
  if (a === b) return true
  if (!a.includes('\r') && !b.includes('\r')) return false
  return normalizeEditorNewlines(a) === normalizeEditorNewlines(b)
}

/** CodeMirror `Text` for a string, with CR/CRLF folded to LF. */
export function editorText(s: string): Text {
  return Text.of(normalizeEditorNewlines(s).split('\n'))
}

/** True when `savedDoc` is a different document than the editor's clean baseline. */
export function diskBaselineMoved(clean: Text | undefined, savedDoc: string): boolean {
  if (!clean) return true
  // `clean` is already LF-normalized; skip the rope toString when lengths differ.
  if (!savedDoc.includes('\r')) {
    return clean.length !== savedDoc.length || clean.toString() !== savedDoc
  }
  return normalizeEditorNewlines(savedDoc) !== clean.toString()
}

/**
 * Dirty flag + disk snapshot for a newly mounted buffer.
 * Must not touch an EditorView — first mount has none yet, and reading
 * `view.state` there blanks the Code pane (Diff/Preview still render).
 *
 * Clean mounts return no `clean` rope: the caller already has `state.doc`
 * from EditorState.create, and rebuilding it here would scan/split the
 * whole buffer a second time.
 */
export function bufferMountState(
  initialDoc: string,
  savedDoc?: string | null,
): { dirty: true; clean: Text } | { dirty: false } {
  if (savedDoc != null && !editorTextEq(savedDoc, initialDoc)) {
    return { dirty: true, clean: editorText(savedDoc) }
  }
  return { dirty: false }
}

/**
 * Compare a live CodeMirror doc to the last clean snapshot.
 * `Text.eq` is O(1) on length mismatch and same-object undo; if it misses
 * (equal text rebuilt as a different tree), fall back to toString.
 */
export function isBufferDirty(doc: Text, clean: Text | null | undefined): boolean {
  if (!clean) return false
  if (doc.eq(clean)) return false
  if (doc.length !== clean.length || doc.lines !== clean.lines) return true
  return doc.toString() !== clean.toString()
}
