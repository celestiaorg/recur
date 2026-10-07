import { BundleService } from '../../bindings/recur/internal/app'
import { forgetLanguageExtensions } from './cm-lang'
import { bundlesEpoch, invalidateCommandCache } from './commands'
import { pushMenuThemes } from './menu'
import { listPromptActionsUnfiltered } from './promptActions'
import { invalidateSnippetCache } from './snippets'
import { ensureTerminalThemes } from './terminal'
import { ensureTextMateBootstrap } from './tm-bootstrap'

/** Re-read installed bundles without restarting. Resolves to the number of skipped bundles. */
export async function reloadBundles(): Promise<number> {
  await BundleService.InvalidateCatalog()
  invalidateCommandCache()
  invalidateSnippetCache()
  forgetLanguageExtensions()
  const [skips] = await Promise.all([
    BundleService.ListSkippedBundles().catch(() => []),
    ensureTextMateBootstrap(true),
    ensureTerminalThemes(true),
    listPromptActionsUnfiltered(true),
  ])
  pushMenuThemes()
  bundlesEpoch.update((n) => n + 1)
  return skips?.length ?? 0
}
