// File-type icon classes for the explorer, tabs and review lists. The glyph is
// painted on :before (see file-icons.css).
import './file-icons.css'
import { isLightTheme } from './stores'

const ICONS: [RegExp, string, string][] = [
  [/_test\.go$/i, 'test-go-icon medium-cyan', 'test-go-icon dark-cyan'],
  [/\.go$/i, 'go-icon medium-blue', 'go-icon medium-blue'],
  [/^readme(\.|$)/i, 'book-icon medium-blue', 'book-icon medium-blue'],
]

/** Pass themeKey from a reactive `$theme` read so Svelte re-renders on switch. */
export function fileIconClass(name: string, themeKey: string): string {
  const base = name.split(/[/\\]/).pop() || name
  const hit = ICONS.find(([re]) => re.test(base))
  return `icon ${hit ? hit[isLightTheme(themeKey) ? 1 : 2] : 'text-icon medium-blue'}`
}
