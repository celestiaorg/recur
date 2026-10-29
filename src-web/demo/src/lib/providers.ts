import { nameListed, toggleDisabledName } from './skills'

export const PROVIDER_CLAUDE = 'claude-code'
export const PROVIDER_CURSOR = 'cursor'
export const PROVIDER_CODEX = 'codex'
export const PROVIDER_CHATGPT = 'chatgpt'

const CATALOGUE_PROVIDERS = [PROVIDER_CLAUDE, PROVIDER_CURSOR, PROVIDER_CODEX] as const

/** True when the provider is on in Settings → Providers (default on). */
export function providerEnabled(disabled: string[] | undefined | null, slug: string): boolean {
  return !nameListed(disabled, slug)
}

/** Provider slug from a ModelRef (`cursor/composer-2.5` → `cursor`). */
export function modelProvider(ref: string): string {
  const i = ref.indexOf('/')
  return i > 0 ? ref.slice(0, i) : PROVIDER_CLAUDE
}

/**
 * Text follow-ups inject into the live turn instead of sitting in the Recur
 * queue until idle. Images still queue — the follow-up Send drops them. MA
 * and Loop-mode conductor chats (PA/PA*) keep sendMaestroMessage, which has
 * no Inject and would Swap-cancel the live reply.
 */
export function chatSteerFollowUp(
  busy: boolean,
  mode: string,
  hasAttachments: boolean,
  code: string,
): boolean {
  return busy && !hasAttachments && code !== 'MA' && mode !== 'loop'
}

/**
 * True when this provider would appear in the model catalogue.
 * cursorReady / codexReady must mean the provider actually works
 * (authenticated + on), not merely a saved key or an installed binary.
 * extraReady are configured OpenAI-compatible slugs from Settings.
 */
export function providerOffersModels(
  slug: string,
  disabled: string[] | undefined | null,
  cursorReady: boolean,
  codexReady: boolean,
  extraReady: string[] | undefined | null = null,
): boolean {
  if (!providerEnabled(disabled, slug)) return false
  switch (slug) {
    case PROVIDER_CLAUDE:
      return true
    case PROVIDER_CURSOR:
      return cursorReady
    case PROVIDER_CODEX:
      return codexReady
    default:
      return extraReady?.includes(slug) ?? false
  }
}

/**
 * False when turning this provider off would leave no models in the pickers.
 * Unused Cursor/Codex can always be collapsed. UI-only; SetSettings reclamps.
 */
export function canDisableProvider(
  slug: string,
  disabled: string[] | undefined | null,
  cursorReady: boolean,
  codexReady: boolean,
  extraReady: string[] | undefined | null = null,
): boolean {
  if (!providerEnabled(disabled, slug)) return true
  const next = toggleDisabledName(disabled ?? undefined, slug, false)
  const catalogue = [...CATALOGUE_PROVIDERS, ...(extraReady ?? [])]
  return catalogue.some((p) => providerOffersModels(p, next, cursorReady, codexReady, extraReady))
}
