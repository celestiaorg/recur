/**
 * Load TextMate grammars + themes from BundleService into the client registry.
 */
import {
  ListGrammars,
  ListThemes,
  ReadGrammar,
  ReadTheme,
} from '../../bindings/recur/internal/app/bundleservice'
import { get } from 'svelte/store'
import type { GrammarInfo, ThemeInfo } from '../../bindings/recur/internal/app/models'
import { editorTheme, setEditorTheme, settings } from './stores'
import {
  addGrammar,
  forgetBundleContribs,
  loadAndRegisterTheme,
  listThemeMeta,
  refreshTmColorMapStyle,
  setActiveEditorBackground,
  setActiveRootBackgrounds,
  setActiveTheme,
} from './tm-registry'

let boot: Promise<void> | null = null

/**
 * Idempotent bootstrap, safe to call from app mount and editor open. `reload`
 * re-reads bundles and rejects (keeping the loaded set) if they cannot be listed.
 */
export function ensureTextMateBootstrap(reload = false): Promise<void> {
  if (boot && !reload) return boot
  boot = (async () => {
    const listed = <T>(what: string) => (err: unknown): T[] => {
      if (reload) throw err
      console.warn(`${what} failed`, err)
      return []
    }
    const grammars = await ListGrammars().catch(listed<GrammarInfo>('ListGrammars'))
    const themes = await ListThemes().catch(listed<ThemeInfo>('ListThemes'))
    if (reload) forgetBundleContribs()
    for (const g of grammars) {
      addGrammar(g, () => ReadGrammar(g.scopeName))
    }
    for (const t of themes) {
      try {
        await loadAndRegisterTheme(t, () => ReadTheme(t.id))
      } catch (err) {
        console.warn(`failed to load theme ${t.name}`, err)
      }
    }
    // Apply the user's saved editor theme, not the default: `initEditorTheme()`
    // has already restored it, but nothing could resolve it until the bundle
    // themes registered just above. Falling back to Charcoal here left a
    // restored session painting in the default until the next file switch
    // rebuilt the extensions.
    const savedKey = get(editorTheme)
    const disabled = new Set(
      (get(settings)?.disabledBundleThemes ?? []).filter(Boolean),
    )
    const preferred =
      listThemeMeta().find((t) => t.id === savedKey && !disabled.has(t.id)) ??
      listThemeMeta().find((t) => t.id === 'recur' && !disabled.has(t.id)) ??
      listThemeMeta().find((t) => t.uiTheme === 'dark' && !disabled.has(t.id)) ??
      listThemeMeta().find((t) => !disabled.has(t.id))
    if (preferred) {
      if (preferred.id !== savedKey) setEditorTheme(preferred.id)
      const parsed = await loadAndRegisterTheme(preferred, () => ReadTheme(preferred.id))
      setActiveTheme(parsed.rawTheme)
      setActiveEditorBackground(parsed.colors['editor.background'] ?? '#1e1e1e')
      setActiveRootBackgrounds(parsed.rootBackgrounds)
      refreshTmColorMapStyle()
    }
  })().catch((err) => {
    boot = null
    console.error('TextMate bootstrap failed', err)
    if (reload) throw err
  })
  return boot
}
