/**
 * Shared TM paint Facet — lives outside cm-textmate.ts so merge/diff editors can
 * suppress token washes without pulling the highlighter into the main chunk.
 */
import { Facet, type Extension } from '@codemirror/state'

/** When false, skip TextMate token backgrounds (merge/diff line washes win). Default true. */
export const tmPaintTokenBackgrounds = Facet.define<boolean, boolean>({
  combine: (v) => (v.length ? v[v.length - 1]! : true),
})

/** Merge/diff editors: keep TM foreground, drop token background fills. */
export function suppressTmTokenBackgrounds(): Extension {
  return tmPaintTokenBackgrounds.of(false)
}
