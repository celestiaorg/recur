<script lang="ts">
  import { untrack } from 'svelte'
  import { get } from 'svelte/store'
  import {
    view, policy, settings, go, themes, theme, setTheme, themeAuto, setThemeAuto, isLightTheme, themeLight, themeDark, setSystemTheme, providerUsage,
    settingsSection, type SettingsSection,
    editorTheme, setEditorTheme, editorThemeAuto, setEditorThemeAuto, editorThemeLight, editorThemeDark, setSystemEditorTheme, editorPrefs, setEditorPrefs, setEditorIndentForLang, pushToast, toastEditorTheme,
    EDITOR_FONT_SIZES, EDITOR_TAB_SIZES, EDITOR_INDENT_OPTIONS, editorFontStack,
    EDITOR_WHITESPACE_OPTIONS, EDITOR_LINE_HEIGHTS, EDITOR_LINE_HEIGHT_LABELS,
    EDITOR_RULER_COLUMNS, EDITOR_KEYMAP_OPTIONS,
    resolveEditorIndent, editorIndentLabel,
    CODE_PANEL_KEYS, CODE_PANEL_LABELS, codePanelSides, setCodePanelSide,
    explorerPrefs, setExplorerExclude, resetExplorerExclude,
    terminalPrefs, setTerminalPrefs, terminalThemeAuto, setTerminalThemeAuto, terminalThemeLight, terminalThemeDark, setSystemTerminalTheme, TERMINAL_LINE_HEIGHTS, TERMINAL_LINE_HEIGHT_LABELS,
    TERMINAL_MATCH_EDITOR, resolveTerminalFont,
    type CodePanelKey, type PanelSide, type Theme,
  } from '../lib/stores'
  import { skillKind as classifySkill, skillLocation, mapSkillRows, toggleDisabledName, type SkillRow } from '../lib/skills'
  import {
    LINEAR_MCP_ID, customMCPServers, linearMCPOn, upsertLinearEnabled,
    setCustomMCPEnabled, removeMCPServer, updateMCPServer, slugMCPID, validMCPID, type MCPServerRow,
  } from '../lib/mcp'
  import { simplifyEnabled } from '../lib/simplify'
  import {
    PROVIDER_CLAUDE, PROVIDER_CODEX, PROVIDER_CURSOR,
    canDisableProvider, providerEnabled,
  } from '../lib/providers'
  import {
    compatibleConfigured,
  } from '../lib/compatible-providers'
  import type { CompatibleProvider } from '../../bindings/recur/internal/domain/models'
  import CompatibleProviderCard from '../components/CompatibleProviderCard.svelte'
  import CompatibleProviderModal from '../components/CompatibleProviderModal.svelte'
  import {
    allEditorThemesForPicker,
    filterEditorThemesForList,
    type EditorThemeCard,
  } from '../lib/cm-themes'
  import { pushMenuThemes } from '../lib/menu'
  import { EDITOR_INDENT_LANGS, LANGUAGE_INDENT_DEFAULTS } from '../lib/cm-lang'
  import { listMonospaceFonts } from '../lib/mono-fonts'
  import { PolicyService, SettingsService, CodeService, loadInitial, prefetchProviderUsage, refreshProviderUsage, call, callOk, bridgeErrorMessage } from '../lib/api'
  import { openExternalUrl } from '../lib/open-external'
  import { canDisconnectCursor, cursorQuota, snapshotHasProviderError } from '../lib/providerUsage'
  import {
    loadModelCatalog, peekModelCatalog, invalidateModelCatalog,
    normalizeFlags, flagsEqual, flagsForModel,
    type CatalogModel,
  } from '../lib/models'
  import { CHAT_MODES, type ChatMode } from '../lib/modes'
  import LoopPolicyPickers from '../components/LoopPolicyPickers.svelte'
  import {
    autonomyFromPolicyId, defaultLoopId, defaultPolicyId,
    enabledLoops, enabledPolicies, fallbackLoops, fallbackPolicies,
    listLoops, listPolicies, type LoopInfo, type PolicyInfo,
  } from '../lib/loops'
  import Select, { type SelectOption } from '../components/Select.svelte'
  import Icon, { type IconName } from '../components/Icon.svelte'
  import Kbd from '../components/Kbd.svelte'
  import ModelPickers from '../components/ModelPickers.svelte'
  import SettingsGroup from '../components/SettingsGroup.svelte'
  import SettingsGroups from '../components/SettingsGroups.svelte'
  import type { ClaudeStatus, CodexStatus, CursorStatus } from '../../bindings/recur/internal/app/models'
  import { Events } from '@wailsio/runtime'
  import {
    listAllCommandsUnfiltered,
    type RegisteredCommand,
  } from '../lib/commands'
  import { listAllSnippetsUnfiltered, type RegisteredSnippet } from '../lib/snippets'
  import { listPromptActionsUnfiltered } from '../lib/promptActions'
  import { BundleService, LSPService } from '../../bindings/recur/internal/app'
  import type { BundleSettingInfo, GrammarInfo, LanguageServerInfo, PromptActionInfo, ThemeInfo } from '../../bindings/recur/internal/app/models'
  import { ensureTerminalThemes, terminalThemeCatalog } from '../lib/terminal'

  type Section = SettingsSection
  let query = $state('')
  let clearing = $state(false)
  let confirmingClear = $state(false)
  let logsPath = $state('')
  let githubLogin = $state('')
  let monoFontOptions = $state<SelectOption[]>([])
  let monoFontsLoading = $state(false)
  /** Bundle TextMate themes; refreshed after TextMate bootstrap. */
  let editorThemeCards = $state<EditorThemeCard[]>(allEditorThemesForPicker())
  let editorThemeQuery = $state('')
  let editorThemeKindFilter = $state<'all' | 'dark' | 'light'>('all')
  const editorThemeKindFilters = ['all', 'dark', 'light'] as const
  const editorThemeList = $derived(
    filterEditorThemesForList(editorThemeCards, editorThemeQuery, editorThemeKindFilter),
  )
  const selectedEditorTheme = $derived(
    editorThemeCards.find((t) => t.key === $editorTheme) ?? null,
  )
  const appChromeTheme = $derived(themes.find((x) => x.key === $theme) ?? themes[0])
  const lightEditorThemes = $derived(filterEditorThemesForList(editorThemeCards, editorThemeQuery, 'light'))
  const darkEditorThemes = $derived(filterEditorThemesForList(editorThemeCards, editorThemeQuery, 'dark'))
  let editorThemeBoxEl = $state<HTMLDivElement | undefined>()
  const appThemeFor = (mode: 'light' | 'dark') =>
    themes.find((x) => x.key === (mode === 'light' ? $themeLight : $themeDark)) ?? themes[0]

  function scrollSelectedEditorThemeIntoView() {
    editorThemeBoxEl?.querySelectorAll('.edth-row.on').forEach((row) => row.scrollIntoView({ block: 'nearest' }))
  }

  function revealSelectedEditorTheme() {
    editorThemeQuery = ''
    editorThemeKindFilter = 'all'
    queueMicrotask(scrollSelectedEditorThemeIntoView)
  }

  $effect(() => {
    if ($settingsSection !== 'appearance') return
    void import('../lib/tm-bootstrap').then((m) => m.ensureTextMateBootstrap()).then(() => {
      editorThemeCards = allEditorThemesForPicker()
      pushMenuThemes()
      queueMicrotask(scrollSelectedEditorThemeIntoView)
    })
    void ensureTerminalThemes().then(() => pushMenuThemes())
  })

  $effect(() => {
    // Scroll the active row into view when the selection changes (not on every filter keystroke).
    void $editorTheme
    if ($settingsSection !== 'appearance') return
    queueMicrotask(scrollSelectedEditorThemeIntoView)
  })

  // ---- Claude Code account ----
  let claudeStatus = $state<ClaudeStatus | null>(null)
  let claudeBusy = $state(false)

  // ---- Cursor provider ----
  let cursorStatus = $state<CursorStatus | null>(null)
  let cursorUsageBusy = $state(false)
  const cursorUsage = $derived(cursorQuota($providerUsage))
  const cursorUsageDisconnectable = $derived(canDisconnectCursor($providerUsage))
  const cursorConnecting = $derived(!!cursorUsage?.connecting)
  const cursorSignedIn = $derived(cursorUsageDisconnectable)
  const cursorAccountName = $derived(cursorStatus?.name || '')
  const cursorAccountEmail = $derived(cursorUsage?.email || cursorStatus?.email || '')

  // Connect is the only Cursor sign-in. The minted SDK key follows that session.
  const cursorConfigured = $derived(cursorSignedIn)
  const claudeOn = $derived(providerEnabled($settings?.disabledProviders, PROVIDER_CLAUDE))
  const cursorOn = $derived(providerEnabled($settings?.disabledProviders, PROVIDER_CURSOR))
  const codexOn = $derived(providerEnabled($settings?.disabledProviders, PROVIDER_CODEX))

  // Probe Claude / Cursor / Codex status whenever the Providers pane is open
  // and that provider is on. Skip turned-off providers so we don't start a
  // Cursor bridge just to render a collapsed row.
  //
  // Separate effects so Cursor's settings.set does not cancel the Claude
  // probe. Prefetch paints the last snapshot immediately. Skip it here
  // when Claude is on — that effect already awaits the same call. Force a
  // collect only when the probe says logged in but the snapshot still
  // shows a Claude auth error (prefetch's collect is usually self-healing).
  $effect(() => {
    if ($settingsSection !== 'agent' || claudeOn) return
    void prefetchProviderUsage()
  })

  $effect(() => {
    if ($settingsSection !== 'agent' || !claudeOn) return
    let cancelled = false
    claudeBusy = true
    const painted = prefetchProviderUsage()
    SettingsService.GetClaudeStatus()
      .then(async (s) => {
        if (!cancelled) claudeStatus = s
        await painted
        if (cancelled || !s?.authenticated) return
        if (snapshotHasProviderError(get(providerUsage), get(settings)?.disabledProviders)) {
          void refreshProviderUsage()
        }
      })
      .catch(() => { if (!cancelled) claudeStatus = null })
      .finally(() => { if (!cancelled) claudeBusy = false })
    return () => { cancelled = true }
  })

  $effect(() => {
    if ($settingsSection !== 'agent' || !cursorOn) return
    let cancelled = false
    const prevKey = $settings?.cursorApiKey ?? ''
    SettingsService.GetCursorStatus()
      .then(async (s) => {
        if (cancelled) return
        cursorStatus = s
        // Orphan-key wipe happens in GetCursorStatus — pull settings only
        // when the stored key actually changed so this effect does not loop.
        if (!prevKey) return
        const next = await SettingsService.GetSettings()
        if (!cancelled && next && (next.cursorApiKey ?? '') !== prevKey) settings.set(next)
      })
      .catch(() => { if (!cancelled) cursorStatus = null })
    return () => { cancelled = true }
  })

  $effect(() => {
    if ($settingsSection !== 'agent' || !codexOn) return
    let cancelled = false
    // Probe even before auth so the Binary path shows.
    codexBusy = true
    SettingsService.GetCodexStatus()
      .then((s) => { if (!cancelled) codexStatus = s })
      .catch(() => { if (!cancelled) codexStatus = null })
      .finally(() => { if (!cancelled) codexBusy = false })
    return () => { cancelled = true }
  })

  async function connectCursorUsage() {
    cursorUsageBusy = true
    try {
      const r = await SettingsService.ConnectCursorUsage()
      if (r?.error) {
        pushToast('error', r.error)
        return
      }
      if (r?.loginUrl) await openExternalUrl(r.loginUrl)
    } catch (err) {
      pushToast('error', bridgeErrorMessage(err) || 'Could not start Cursor sign-in')
    } finally {
      cursorUsageBusy = false
    }
  }

  function cancelCursorUsage() {
    void SettingsService.CancelCursorUsageConnect()
  }

  async function disconnectCursorUsage() {
    cursorUsageBusy = true
    try {
      await SettingsService.DisconnectCursorUsage()
      cursorStatus = cursorStatus
        ? { ...cursorStatus, authenticated: false, name: '', email: '', account: '', error: '' }
        : null
      const next = await SettingsService.GetSettings()
      if (next) settings.set(next)
      void prefetchProviderUsage()
    } catch (err) {
      pushToast('error', bridgeErrorMessage(err) || 'Could not disconnect Cursor')
    } finally {
      cursorUsageBusy = false
    }
  }

  // ---- Codex provider (app-server) ----
  let codexKeyDraft = $state('')
  let codexKeyDirty = $state(false)
  let codexKeyVisible = $state(false)
  let codexKeyTest = $state<{ ok: boolean; text: string } | null>(null)
  let codexKeyTesting = $state(false)
  let codexStatus = $state<CodexStatus | null>(null)
  let codexBusy = $state(false)
  const codexConfigured = $derived(!!($settings?.codexApiKey || $settings?.codexChatGPTAuth))
  const cursorReady = $derived(cursorSignedIn)
  // Connected but unable to run anything: the SDK bridge is fetched on demand.
  const cursorSdkMissing = $derived(cursorOn && cursorSignedIn && !!cursorStatus && !cursorStatus.bridgeInstalled)
  const codexReady = $derived(!!codexStatus?.authenticated)
  const compatibleProviders = $derived($settings?.compatibleProviders ?? [])
  const extraReady = $derived(
    compatibleProviders.filter(compatibleConfigured).map((p) => p.id),
  )

  let addCompatOpen = $state(false)
  let compatTest = $state<Record<string, { ok: boolean; text: string } | null>>({})
  let compatTesting = $state<Record<string, boolean>>({})

  function saveCompatible(list: CompatibleProvider[]) {
    invalidateModelCatalog()
    return patchSettings({ compatibleProviders: list })
  }

  function updateCompatible(next: CompatibleProvider) {
    if (!(next.models ?? []).some((m) => m.id?.trim())) {
      const prev = compatibleProviders.find((p) => p.id === next.id)
      if (prev?.models?.some((m) => m.id?.trim())) {
        next = { ...next, models: prev.models }
      } else {
        return
      }
    }
    saveCompatible(compatibleProviders.map((p) => (p.id === next.id ? next : p)))
  }

  function removeCompatible(id: string) {
    saveCompatible(compatibleProviders.filter((p) => p.id !== id))
    const { [id]: _drop, ...rest } = compatTest
    compatTest = rest
  }

  async function testCompatible(p: CompatibleProvider) {
    compatTesting = { ...compatTesting, [p.id]: true }
    compatTest = { ...compatTest, [p.id]: null }
    try {
      const r = await SettingsService.TestCompatibleProvider(p)
      compatTest = {
        ...compatTest,
        [p.id]: r?.ok ? { ok: true, text: 'Connected' } : { ok: false, text: r?.error || 'Could not reach endpoint' },
      }
    } catch (err) {
      compatTest = { ...compatTest, [p.id]: { ok: false, text: bridgeErrorMessage(err) || 'Could not reach endpoint' } }
    } finally {
      compatTesting = { ...compatTesting, [p.id]: false }
    }
  }

  $effect(() => {
    if (!codexKeyDirty) codexKeyDraft = $settings?.codexApiKey ?? ''
  })

  // ---- Bundle [[setting]] (Settings → Integrations) ----
  let bundleSettingSchema = $state<BundleSettingInfo[]>([])
  let bundleSettingsLoading = $state(false)
  let bundleSettingsErr = $state('')
  let bundleSettingDrafts = $state<Record<string, string>>({})
  let bundleSettingFocus = $state('')
  let bundleSettingVisible = $state<Record<string, boolean>>({})
  let bundleSettingSave = Promise.resolve<unknown>(null)

  $effect(() => {
    if ($settingsSection !== 'integrations') return
    let cancelled = false
    bundleSettingsLoading = true
    bundleSettingsErr = ''
    BundleService.ListBundleSettings()
      .then((rows) => {
        if (!cancelled) bundleSettingSchema = rows ?? []
      })
      .catch((err) => {
        if (!cancelled) {
          bundleSettingSchema = []
          bundleSettingsErr = err instanceof Error ? err.message : String(err)
        }
      })
      .finally(() => {
        if (!cancelled) bundleSettingsLoading = false
      })
    return () => { cancelled = true }
  })

  $effect(() => {
    const values = $settings?.bundleValues ?? {}
    const focus = bundleSettingFocus
    const schema = bundleSettingSchema
    const prev = untrack(() => bundleSettingDrafts)
    bundleSettingDrafts = Object.fromEntries(schema.map((s) => {
      if (focus === s.id) return [s.id, prev[s.id] ?? values[s.id] ?? '']
      return [s.id, values[s.id] ?? '']
    }))
  })

  const bundleSettingGroups = $derived.by(() => {
    const meta = new Map<string, BundleSettingInfo[]>()
    for (const s of bundleSettingSchema) {
      const name = s.bundleName || 'Bundle'
      let list = meta.get(name)
      if (!list) {
        list = []
        meta.set(name, list)
      }
      list.push(s)
    }
    return [...meta.entries()]
      .sort((a, b) => a[0].localeCompare(b[0]))
      .map(([bundle, items]) => ({ bundle, items }))
  })

  function commitBundleSetting(id: string, v: string) {
    bundleSettingSave = bundleSettingSave.then(() => {
      const live: Record<string, string> = {}
      for (const [k, val] of Object.entries($settings?.bundleValues ?? {})) {
        if (val) live[k] = val
      }
      if (v) live[id] = v
      else delete live[id]
      if (($settings?.bundleValues?.[id] ?? '') === v) return Promise.resolve(null)
      return patchSettings({ bundleValues: live })
    })
  }

  function testCodexKey() {
    // Probe the draft / ChatGPT-auth toggle only — Connected status above stays
    // on the saved settings until Save.
    codexKeyTesting = true
    codexKeyTest = null
    SettingsService.TestCodexKey(codexKeyDraft, !!$settings?.codexChatGPTAuth)
      .then((s) => {
        if (s?.authenticated) {
          const who = s.email || s.loginMethod || ''
          codexKeyTest = {
            ok: true,
            text: who ? `Connected successfully as ${who}` : 'Connected successfully',
          }
        } else {
          codexKeyTest = { ok: false, text: s?.error || 'Key was rejected' }
        }
      })
      .catch((err) => {
        codexKeyTest = { ok: false, text: bridgeErrorMessage(err) || 'Could not reach Codex' }
      })
      .finally(() => { codexKeyTesting = false })
  }

  function saveCodexKey() {
    codexKeyDirty = false
    codexKeyTest = null
    invalidateModelCatalog()
    codexBusy = true
    patchSettings({ codexApiKey: codexKeyDraft })
      .then((s) => {
        if (!s) return null
        return SettingsService.GetCodexStatus()
      })
      .then((s) => { if (s) codexStatus = s })
      .catch(() => { codexStatus = null })
      .finally(() => { codexBusy = false })
  }

  // ---- Bridge install ----
  // The bridge is ~50MB compressed / ~155MB unpacked (a bundled Node runtime),
  // so Recur fetches it on demand rather than shipping it. Progress arrives as
  // `cursor.install` bus events while InstallCursorBridge() runs.
  let installing = $state(false)
  let installMsg = $state('')
  let installPct = $state(-1)

  $effect(() => {
    const off = Events.On('cursor.install', (e: any) => {
      const d = e.data ?? {}
      installMsg = d.message ?? ''
      installPct = typeof d.percent === 'number' ? d.percent : -1
    })
    return () => { off?.() }
  })

  function installBridge() {
    installing = true
    installMsg = 'Starting…'
    installPct = -1
    call(SettingsService.InstallCursorBridge(), 'Could not install the Cursor SDK')
      .then((s) => {
        if (s) {
          cursorStatus = s
          if (s.bridgeInstalled) pushToast('success', 'Cursor SDK installed')
        }
      })
      .finally(() => {
        installing = false
        installMsg = ''
        installPct = -1
      })
  }

  // One grouped model list across every enabled, configured provider — picking
  // a model is what selects the provider for that role. Claude Code is present
  // unless turned off; Cursor appears once Connect has minted a key and is on.
  let catalogModels = $state<CatalogModel[]>(peekModelCatalog())

  $effect(() => {
    if ($settingsSection !== 'models') return
    // Re-fetch when Cursor/Codex enablement or provider toggles change.
    void cursorConfigured
    void codexConfigured
    void claudeOn
    void cursorOn
    void codexOn
    let cancelled = false
    loadModelCatalog()
      .then((models) => { if (!cancelled) catalogModels = models })
      .catch(() => {})
    return () => { cancelled = true }
  })

  function modeFlags(ref: string, cur: { [k: string]: string | undefined } | undefined | null): Record<string, string> {
    return normalizeFlags(flagsForModel(catalogModels, ref), cur)
  }

  type ModeSlot = ChatMode | 'title'

  function modeModelRef(mode: ModeSlot): string {
    const s = $settings
    const fallback = 'claude-code/claude-sonnet-5-5'
    switch (mode) {
      case 'ask': return s?.modelAsk || fallback
      case 'plan': return s?.modelPlan || fallback
      case 'agent': return s?.modelAgent || fallback
      case 'review': return s?.modelReview || s?.modelLoop || fallback
      case 'loop': return s?.modelLoop || fallback
      case 'title': return s?.modelWorkspaceTitle || 'claude-code/claude-haiku-4-5-20251001'
    }
  }

  function modeFlagsCur(mode: ModeSlot): { [k: string]: string | undefined } | undefined | null {
    const s = $settings
    switch (mode) {
      case 'ask': return s?.flagsAsk
      case 'plan': return s?.flagsPlan
      case 'agent': return s?.flagsAgent
      case 'review': return s?.flagsReview
      case 'loop': return s?.flagsLoop
      case 'title': return s?.flagsWorkspaceTitle
    }
  }

  function setModel(mode: ModeSlot, ref: string, flags: Record<string, string>) {
    switch (mode) {
      case 'ask': patchSettings({ modelAsk: ref, flagsAsk: flags }); break
      case 'plan': patchSettings({ modelPlan: ref, flagsPlan: flags }); break
      case 'agent': patchSettings({ modelAgent: ref, flagsAgent: flags }); break
      case 'review': patchSettings({ modelReview: ref, flagsReview: flags }); break
      case 'loop': patchSettings({ modelLoop: ref, flagsLoop: flags }); break
      case 'title': patchSettings({ modelWorkspaceTitle: ref, flagsWorkspaceTitle: flags }); break
    }
  }

  function setFlags(mode: ModeSlot, next: Record<string, string>) {
    if (flagsEqual(modeFlagsCur(mode) ?? undefined, next)) return
    switch (mode) {
      case 'ask': patchSettings({ flagsAsk: next }); break
      case 'plan': patchSettings({ flagsPlan: next }); break
      case 'agent': patchSettings({ flagsAgent: next }); break
      case 'review': patchSettings({ flagsReview: next }); break
      case 'loop': patchSettings({ flagsLoop: next }); break
      case 'title': patchSettings({ flagsWorkspaceTitle: next }); break
    }
  }

  $effect(() => {
    if ($settingsSection !== 'general') {
      branchPrefixDirty = false
      branchPrefixDraft = ''
      return
    }
    let cancelled = false
    SettingsService.GitHubLogin()
      .then((login) => {
        if (!cancelled) githubLogin = (login ?? '').trim()
      })
      .catch(() => {
        if (!cancelled) githubLogin = ''
      })
    return () => { cancelled = true }
  })

  const savedBranchPrefix = $derived.by(() => {
    if ($settings?.branchNamePrefix != null) return String($settings.branchNamePrefix)
    if ($settings?.branchPrefixMode === 'none') return ''
    return 'recur'
  })
  let branchPrefixDirty = $state(false)
  let branchPrefixDraft = $state('')
  // Preset mousedown blurs the input (firing change) before click. Skip that
  // commit so we don't race two SetSettings calls.
  let skipPrefixCommit = false
  const branchPrefixValue = $derived(branchPrefixDirty ? branchPrefixDraft : savedBranchPrefix)
  const branchPrefixExample = $derived(
    branchPrefixValue.trim() ? `${branchPrefixValue.trim()}/crispy-quiche` : 'crispy-quiche',
  )
  const prefixPreset = $derived.by(() => {
    if (branchPrefixDirty) return ''
    if (savedBranchPrefix === '') return 'none'
    if (savedBranchPrefix === 'recur') return 'recur'
    if (githubLogin && savedBranchPrefix.toLowerCase() === githubLogin.toLowerCase()) return 'github'
    return ''
  })

  function commitBranchPrefix(raw: string) {
    branchPrefixDirty = false
    if (skipPrefixCommit) {
      skipPrefixCommit = false
      return
    }
    const v = raw.trim()
    if (v === savedBranchPrefix) return
    void patchSettings({ branchNamePrefix: v })
  }

  function pickBranchPrefix(kind: 'recur' | 'github' | 'none') {
    skipPrefixCommit = false
    branchPrefixDirty = false
    if (kind === 'github') {
      if (!githubLogin) return
      void patchSettings({ branchNamePrefix: githubLogin })
      return
    }
    void patchSettings({ branchNamePrefix: kind === 'none' ? '' : 'recur' })
  }
  $effect(() => {
    if ($settingsSection !== 'editor') return
    let cancelled = false
    monoFontsLoading = true
    listMonospaceFonts()
      .then((fonts) => {
        if (cancelled) return
        const opts = fonts.map((f) => ({ value: f.family, label: f.label }))
        const cur = $editorPrefs.fontFace
        if (cur && !opts.some((o) => o.value === cur)) {
          opts.unshift({ value: cur, label: cur })
        }
        monoFontOptions = opts
      })
      .finally(() => {
        if (!cancelled) monoFontsLoading = false
      })
    return () => { cancelled = true }
  })

  function fontOptionStyle(opt: SelectOption): string {
    const size = $editorPrefs.fontSize
    return `font-family:${editorFontStack(opt.value)};font-size:${size}px;line-height:1.45`
  }

  const fontValueStyle = $derived(
    `font-family:${editorFontStack($editorPrefs.fontFace)};font-size:${$editorPrefs.fontSize}px`,
  )

  // The terminal font picker gets a "Match editor" entry on top of the same list,
  // so the two stay linked until you deliberately separate them.
  const termFontOptions = $derived<SelectOption[]>([
    { value: TERMINAL_MATCH_EDITOR, label: 'Match editor' },
    ...monoFontOptions,
  ])
  const termFont = $derived(resolveTerminalFont($terminalPrefs, $editorPrefs))
  const termFontValueStyle = $derived(
    `font-family:${editorFontStack(termFont.fontFace)};font-size:${termFont.fontSize}px`,
  )
  function termFontOptionStyle(opt: SelectOption): string {
    if (!opt.value) return `font-size:${termFont.fontSize}px;line-height:1.45`
    return `font-family:${editorFontStack(opt.value)};font-size:${termFont.fontSize}px;line-height:1.45`
  }

  function setSide(key: CodePanelKey, side: PanelSide) {
    setCodePanelSide(key, side)
  }

  function updateExclude(i: number, value: string) {
    const next = [...$explorerPrefs.exclude]
    next[i] = value
    setExplorerExclude(next)
  }

  function removeExclude(i: number) {
    setExplorerExclude($explorerPrefs.exclude.filter((_, j) => j !== i))
  }

  function addExclude() {
    setExplorerExclude([...$explorerPrefs.exclude, ''])
  }

  const nav = [
    {
      group: 'App',
      items: [
        { id: 'general' as const, label: 'General', icon: 'general', keywords: 'general simplify default yagni stdlib sleep caffeinate awake power prevent idle mac agent turn timeout minutes limit deadline concurrent loops agents slots cap governor branch prefix git workspace name github schedule keep last' },
        { id: 'appearance' as const, label: 'Appearance', icon: 'appearance', keywords: 'theme color palette chrome app editor theme dark light charcoal slate cool glow catppuccin mocha latte paper nord terminal xterm ansi solarized vga tango campbell homebrew snazzy ubuntu' },
        { id: 'editor' as const, label: 'Code editor', icon: 'editor', keywords: 'editor code font size face wrap tab indent spaces go python typescript keymap vim emacs whitespace caret ruler panel left right files scm source control outline problems search refs explorer exclude glob hidden max open tabs gutter diff hints terminal' },
        { id: 'skills' as const, label: 'Skills', icon: 'skills', keywords: 'skills agent skill slash disable enable claude cursor codex recur' },
        { id: 'data' as const, label: 'Data', icon: 'data', keywords: 'clear wipe reset delete logs log reveal finder' },
      ],
    },
    {
      group: 'Agent',
      items: [
        { id: 'agent' as const, label: 'Providers', icon: 'providers', keywords: 'providers account claude cursor codex login auth permissions bypass bash enable disable off usage quota' },
        { id: 'models' as const, label: 'Models', icon: 'models', keywords: 'orchestrator coder review opus sonnet haiku fable flags effort context child workspace inherit parent' },
      ],
    },
    {
      group: 'Loops',
      items: [
        { id: 'loops' as const, label: 'Loops', icon: 'loops', keywords: 'loop policy cautious balanced aggressive default rails autonomy guardrails' },
      ],
    },
    {
      group: 'Workflow',
      items: [
        { id: 'prs' as const, label: 'Pull requests', icon: 'prs', keywords: 'merge squash rebase method pr pull request strategy commit message title description' },
        { id: 'integrations' as const, label: 'Integrations', icon: 'integrations', keywords: 'linear api key github issues tracker ticket workspace kickoff jira bundle setting' },
        { id: 'mcp' as const, label: 'MCP', icon: 'mcp', keywords: 'mcp model context protocol linear tools http stdio server agent backlog triage' },
      ],
    },
    {
      group: 'Bundles',
      items: [
        { id: 'bundle-commands' as const, label: 'Commands', icon: 'command', keywords: 'bundles commands macros willsave format on save palette text source encode json go rust disable enable' },
        { id: 'bundle-prompts' as const, label: 'Prompt actions', icon: 'prompt', keywords: 'bundles prompts slash chat commit review simplify-review simplify-audit fix-findings fix-checks update open-pr open-draft-pr linear issue disable enable' },
        { id: 'bundle-grammars' as const, label: 'Grammars', icon: 'braces', keywords: 'bundles grammars syntax highlighting language file types go typescript python disable enable' },
        { id: 'bundle-lsp' as const, label: 'Language servers', icon: 'braces', keywords: 'bundles language server lsp gopls pyright ruff rust-analyzer taplo marksman disable enable' },
        { id: 'bundle-themes' as const, label: 'Themes', icon: 'palette', keywords: 'bundles themes editor colour color charcoal paper cool glow syntax disable enable' },
        { id: 'bundle-snippets' as const, label: 'Snippets', icon: 'code', keywords: 'bundles snippets tab trigger wrap html go python javascript shell disable enable' },
      ],
    },
  ]

  const filteredNav = $derived.by(() => {
    const q = query.trim().toLowerCase()
    if (!q) return nav
    return nav
      .map((g) => ({
        ...g,
        items: g.items.filter((i) =>
          i.label.toLowerCase().includes(q) || i.keywords.includes(q),
        ),
      }))
      .filter((g) => g.items.length > 0)
  })

  $effect(() => {
    const ids = filteredNav.flatMap((g) => g.items.map((i) => i.id))
    if (ids.length && !ids.includes($settingsSection)) settingsSection.set(ids[0])
  })

  const titles: Record<Section, string> = {
    agent: 'Providers',
    models: 'Models',
    loops: 'Loops',
    prs: 'Pull requests',
    integrations: 'Integrations',
    mcp: 'MCP',
    general: 'General',
    appearance: 'Appearance',
    editor: 'Code editor',
    skills: 'Skills',
    data: 'Data',
    'bundle-commands': 'Commands',
    'bundle-prompts': 'Prompt actions',
    'bundle-grammars': 'Grammars',
    'bundle-lsp': 'Language servers',
    'bundle-themes': 'Themes',
    'bundle-snippets': 'Snippets',
  }

  // ---- Skills (enable/disable) — Recur + user/home only; repo skills live on Project. ----
  type SkillKind = 'recur' | 'user'
  let skillRows = $state<SkillRow[]>([])
  let skillsLoading = $state(false)
  let skillsErr = $state('')

  function skillKind(s: SkillRow): SkillKind {
    const k = classifySkill(s)
    return k === 'recur' ? 'recur' : 'user'
  }

  $effect(() => {
    if ($settingsSection !== 'skills') return
    let cancelled = false
    skillsLoading = true
    skillsErr = ''
    // Empty project/workspace → home + Recur skills only (no repo scan).
    CodeService.ListAllSkills('', '')
      .then((list) => {
        if (cancelled) return
        skillRows = mapSkillRows(list).filter((s) => s.scope !== 'workspace')
      })
      .catch((err) => {
        if (cancelled) return
        skillRows = []
        skillsErr = err instanceof Error ? err.message : String(err)
      })
      .finally(() => {
        if (!cancelled) skillsLoading = false
      })
    return () => { cancelled = true }
  })

  type SkillGroup = { kind: SkillKind; label: string; blurb: string; skills: SkillRow[] }

  const skillGroups = $derived.by((): SkillGroup[] => {
    const buckets: Record<SkillKind, SkillRow[]> = { recur: [], user: [] }
    for (const s of skillRows) {
      buckets[skillKind(s)].push(s)
    }
    const meta: { kind: SkillKind; label: string; blurb: string }[] = [
      { kind: 'recur', label: 'Recur', blurb: 'Built into Recur.' },
      { kind: 'user', label: 'Your skills', blurb: 'Skills installed for Claude, Cursor, Codex, and similar tools.' },
    ]
    return meta
      .map((m) => ({
        ...m,
        skills: [...buckets[m.kind]].sort((a, b) => a.name.localeCompare(b.name)),
      }))
      .filter((g) => g.skills.length > 0)
  })

  function skillEnabled(name: string): boolean {
    const disabled = $settings?.disabledSkills
    if (!disabled?.length) return true
    return !disabled.includes(name)
  }

  function setSkillEnabled(name: string, enabled: boolean) {
    if (!$settings) return
    patchSettings({ disabledSkills: toggleDisabledName($settings.disabledSkills, name, enabled) })
  }

  // ---- MCP servers (Settings → MCP) ----
  const linearOn = $derived(linearMCPOn($settings?.mcpServers as MCPServerRow[] | undefined))
  const linearKeySet = $derived(!!($settings?.bundleValues?.['recur.linear.api-key'] ?? '').trim())
  const customMCPs = $derived(customMCPServers($settings?.mcpServers as MCPServerRow[] | undefined))
  let mcpAddKind = $state<'' | 'http' | 'stdio'>('')
  let mcpEditID = $state('')
  let mcpAddName = $state('')
  let mcpAddURL = $state('')
  let mcpAddAuth = $state('')
  let mcpAddCommand = $state('')
  let mcpAddArgs = $state('')
  let mcpAddErr = $state('')
  const mcpEditing = $derived(customMCPs.find((s) => s.id === mcpEditID))

  function resetMCPForm() {
    mcpAddKind = ''
    mcpEditID = ''
    mcpAddName = ''
    mcpAddURL = ''
    mcpAddAuth = ''
    mcpAddCommand = ''
    mcpAddArgs = ''
    mcpAddErr = ''
  }

  function setLinearMCP(on: boolean) {
    if (!$settings) return
    patchSettings({ mcpServers: upsertLinearEnabled($settings.mcpServers as MCPServerRow[] | undefined, on) })
  }

  function setMCPEnabled(id: string, on: boolean) {
    if (!$settings) return
    patchSettings({ mcpServers: setCustomMCPEnabled($settings.mcpServers as MCPServerRow[] | undefined, id, on) })
  }

  function deleteMCP(id: string) {
    if (!$settings) return
    if (mcpEditID === id) resetMCPForm()
    patchSettings({ mcpServers: removeMCPServer($settings.mcpServers as MCPServerRow[] | undefined, id) })
  }

  function startMCPEdit(s: MCPServerRow) {
    mcpEditID = s.id || ''
    mcpAddKind = s.transport === 'stdio' ? 'stdio' : 'http'
    mcpAddName = s.name || s.id || ''
    mcpAddURL = s.url || ''
    mcpAddAuth = ''
    mcpAddCommand = s.command || ''
    mcpAddArgs = (s.args ?? []).join(' ')
    mcpAddErr = ''
  }

  function mcpAuthHeader(raw: string): string {
    const auth = raw.trim()
    return auth.startsWith('Bearer ') ? auth : `Bearer ${auth}`
  }

  function submitMCPAdd() {
    if (!$settings) return
    const name = mcpAddName.trim()
    if (mcpEditID) {
      const cur = ($settings.mcpServers ?? []).find((s) => s.id === mcpEditID)
      if (!cur) {
        mcpAddErr = 'That server is gone.'
        return
      }
      const patch: MCPServerRow = { name: name || cur.id, transport: mcpAddKind || cur.transport || 'http' }
      if (mcpAddKind === 'stdio') {
        const cmd = mcpAddCommand.trim()
        if (!cmd) {
          mcpAddErr = 'Command is required.'
          return
        }
        patch.command = cmd
        patch.args = mcpAddArgs.trim().split(/\s+/).filter(Boolean)
        patch.url = ''
        patch.headers = undefined
      } else {
        const url = mcpAddURL.trim()
        if (!url) {
          mcpAddErr = 'URL is required.'
          return
        }
        patch.url = url
        patch.command = ''
        patch.args = undefined
        const auth = mcpAddAuth.trim()
        // simplify: empty Authorization keeps the stored header (rotate by typing a new token).
        patch.headers = auth ? { Authorization: mcpAuthHeader(auth) } : cur.headers
      }
      mcpAddErr = ''
      const id = mcpEditID
      resetMCPForm()
      patchSettings({ mcpServers: updateMCPServer($settings.mcpServers as MCPServerRow[] | undefined, id, patch) })
      return
    }
    const id = slugMCPID(name)
    if (!validMCPID(id)) {
      mcpAddErr = 'Name must start with a letter (a–z) and use letters, digits, or hyphens.'
      return
    }
    if (id === LINEAR_MCP_ID || ($settings.mcpServers ?? []).some((s) => s.id === id)) {
      mcpAddErr = 'That id is already in use.'
      return
    }
    const row: MCPServerRow = { id, name: name || id, enabled: true, transport: mcpAddKind || 'http' }
    if (mcpAddKind === 'stdio') {
      const cmd = mcpAddCommand.trim()
      if (!cmd) {
        mcpAddErr = 'Command is required.'
        return
      }
      row.command = cmd
      const args = mcpAddArgs.trim().split(/\s+/).filter(Boolean)
      if (args.length) row.args = args
    } else {
      const url = mcpAddURL.trim()
      if (!url) {
        mcpAddErr = 'URL is required.'
        return
      }
      row.url = url
      const auth = mcpAddAuth.trim()
      if (auth) row.headers = { Authorization: mcpAuthHeader(auth) }
    }
    resetMCPForm()
    patchSettings({ mcpServers: [...($settings.mcpServers ?? []), row] })
  }

  // ---- Bundles (command / prompt / grammar / theme enable/disable) ----
  let bundleCmds = $state<RegisteredCommand[]>([])
  let bundlePrompts = $state<PromptActionInfo[]>([])
  let bundleGrammars = $state<GrammarInfo[]>([])
  let bundleThemes = $state<ThemeInfo[]>([])
  let bundleSnippets = $state<RegisteredSnippet[]>([])
  let cmdsLoading = $state(false)
  let cmdsErr = $state('')
  let promptsLoading = $state(false)
  let promptsErr = $state('')
  let grammarsLoading = $state(false)
  let grammarsErr = $state('')
  let themesLoading = $state(false)
  let themesErr = $state('')
  let snippetsLoading = $state(false)
  let snippetsErr = $state('')
  let bundleSkips = $state<{ name: string; source: string; reason: string; dir: string }[]>([])
  let grammarQuery = $state('')
  let themeQuery = $state('')
  let snippetQuery = $state('')
  let languageServers = $state<LanguageServerInfo[]>([])
  let lspLoading = $state(false)
  let lspErr = $state('')

  $effect(() => {
    if ($settingsSection !== 'bundle-commands') return
    let cancelled = false
    cmdsLoading = true
    cmdsErr = ''
    Promise.all([
      listAllCommandsUnfiltered(),
      BundleService.ListSkippedBundles().catch(() => [] as { name: string; source: string; reason: string; dir: string }[]),
    ])
      .then(([cmds, skips]) => {
        if (!cancelled) {
          bundleCmds = cmds
          bundleSkips = skips ?? []
        }
      })
      .catch((err) => {
        if (!cancelled) {
          bundleCmds = []
          bundleSkips = []
          cmdsErr = err instanceof Error ? err.message : String(err)
        }
      })
      .finally(() => {
        if (!cancelled) cmdsLoading = false
      })
    return () => { cancelled = true }
  })

  $effect(() => {
    if ($settingsSection !== 'bundle-prompts') return
    let cancelled = false
    promptsLoading = true
    promptsErr = ''
    listPromptActionsUnfiltered()
      .then((prompts) => {
        if (!cancelled) bundlePrompts = prompts ?? []
      })
      .catch((err) => {
        if (!cancelled) {
          bundlePrompts = []
          promptsErr = err instanceof Error ? err.message : String(err)
        }
      })
      .finally(() => {
        if (!cancelled) promptsLoading = false
      })
    return () => { cancelled = true }
  })

  $effect(() => {
    if ($settingsSection !== 'bundle-grammars') return
    let cancelled = false
    grammarsLoading = true
    grammarsErr = ''
    BundleService.ListGrammars()
      .then((grammars) => {
        if (!cancelled) bundleGrammars = grammars ?? []
      })
      .catch((err) => {
        if (!cancelled) {
          bundleGrammars = []
          grammarsErr = err instanceof Error ? err.message : String(err)
        }
      })
      .finally(() => {
        if (!cancelled) grammarsLoading = false
      })
    return () => { cancelled = true }
  })

  $effect(() => {
    if ($settingsSection !== 'bundle-themes') return
    let cancelled = false
    themesLoading = true
    themesErr = ''
    BundleService.ListThemes()
      .then((tmThemes) => {
        if (!cancelled) bundleThemes = tmThemes ?? []
      })
      .catch((err) => {
        if (!cancelled) {
          bundleThemes = []
          themesErr = err instanceof Error ? err.message : String(err)
        }
      })
      .finally(() => {
        if (!cancelled) themesLoading = false
      })
    return () => { cancelled = true }
  })

  $effect(() => {
    if ($settingsSection !== 'bundle-snippets') return
    let cancelled = false
    snippetsLoading = true
    snippetsErr = ''
    listAllSnippetsUnfiltered()
      .then((snips) => {
        if (!cancelled) bundleSnippets = snips ?? []
      })
      .catch((err) => {
        if (!cancelled) {
          bundleSnippets = []
          snippetsErr = err instanceof Error ? err.message : String(err)
        }
      })
      .finally(() => {
        if (!cancelled) snippetsLoading = false
      })
    return () => { cancelled = true }
  })

  type BundleCatGroup = { category: string; commands: RegisteredCommand[] }

  const bundleGroups = $derived.by((): BundleCatGroup[] => {
    const meta = new Map<string, { order: number; commands: RegisteredCommand[] }>()
    for (const c of bundleCmds) {
      const cat = c.category || 'Commands'
      let g = meta.get(cat)
      if (!g) {
        g = { order: c.order, commands: [] }
        meta.set(cat, g)
      } else if (c.order < g.order) {
        g.order = c.order
      }
      g.commands.push(c)
    }
    return [...meta.entries()]
      .sort((a, b) => (a[1].order !== b[1].order ? a[1].order - b[1].order : a[0].localeCompare(b[0])))
      .map(([category, g]) => ({
        category,
        commands: [...g.commands].sort((a, b) =>
          a.order !== b.order ? a.order - b.order : a.label.localeCompare(b.label),
        ),
      }))
  })

  function commandEnabled(id: string): boolean {
    const disabled = $settings?.disabledBundleCommands
    if (!disabled?.length) return true
    return !disabled.includes(id)
  }

  function setCommandEnabled(id: string, enabled: boolean) {
    if (!$settings) return
    patchSettings({ disabledBundleCommands: toggleDisabledName($settings.disabledBundleCommands, id, enabled) })
  }

  const promptGroups = $derived.by(() => {
    const items = [...bundlePrompts].sort((a, b) => a.slash.localeCompare(b.slash) || a.name.localeCompare(b.name))
    const meta = new Map<string, PromptActionInfo[]>()
    for (const p of items) {
      const name = p.bundleName || 'Prompts'
      let list = meta.get(name)
      if (!list) {
        list = []
        meta.set(name, list)
      }
      list.push(p)
    }
    return [...meta.entries()]
      .sort((a, b) => a[0].localeCompare(b[0]))
      .map(([bundle, items]) => ({ bundle, items }))
  })

  function promptOn(id: string): boolean {
    const disabled = $settings?.disabledBundlePrompts
    if (!disabled?.length) return true
    return !disabled.includes(id)
  }

  function setPromptEnabled(id: string, enabled: boolean) {
    if (!$settings) return
    patchSettings({ disabledBundlePrompts: toggleDisabledName($settings.disabledBundlePrompts, id, enabled) })
  }

  function grammarTitle(g: GrammarInfo): string {
    return (g.language || g.scopeName || g.id).trim()
  }

  function grammarBlurb(g: GrammarInfo): string {
    return (g.fileTypes ?? []).join(', ')
  }

  const grammarGroups = $derived.by(() => {
    const q = grammarQuery.trim().toLowerCase()
    const items = [...bundleGrammars]
      .filter((g) => {
        if (!q) return true
        const hay = [grammarTitle(g), g.scopeName, g.id, g.bundleName, ...(g.fileTypes ?? [])]
          .join(' ')
          .toLowerCase()
        return hay.includes(q)
      })
      .sort((a, b) => {
        const ta = grammarTitle(a).toLowerCase()
        const tb = grammarTitle(b).toLowerCase()
        return ta.localeCompare(tb) || a.id.localeCompare(b.id)
      })
    const meta = new Map<string, GrammarInfo[]>()
    for (const g of items) {
      const name = g.bundleName || 'Grammars'
      let list = meta.get(name)
      if (!list) {
        list = []
        meta.set(name, list)
      }
      list.push(g)
    }
    return [...meta.entries()]
      .sort((a, b) => a[0].localeCompare(b[0]))
      .map(([bundle, grammars]) => ({ bundle, items: grammars }))
  })

  function grammarOn(id: string): boolean {
    const disabled = $settings?.disabledBundleGrammars
    if (!disabled?.length) return true
    return !disabled.includes(id)
  }

  function setGrammarEnabled(id: string, enabled: boolean) {
    if (!$settings) return
    patchSettings({ disabledBundleGrammars: toggleDisabledName($settings.disabledBundleGrammars, id, enabled) })
  }

  function themeBlurb(t: ThemeInfo): string {
    return t.uiTheme === 'light' ? 'Light' : 'Dark'
  }

  const themeGroups = $derived.by(() => {
    const q = themeQuery.trim().toLowerCase()
    const items = [...bundleThemes]
      .filter((t) => {
        if (!q) return true
        const hay = [t.name, t.id, t.bundleName, t.uiTheme].join(' ').toLowerCase()
        return hay.includes(q)
      })
      .sort((a, b) => {
        const ao = a.order || 0
        const bo = b.order || 0
        if (ao !== bo && (ao === 0 || bo === 0)) return ao === 0 ? 1 : -1
        if (ao !== bo) return ao - bo
        return a.name.localeCompare(b.name)
      })
    const meta = new Map<string, ThemeInfo[]>()
    for (const t of items) {
      const name = t.bundleName || 'Themes'
      let list = meta.get(name)
      if (!list) {
        list = []
        meta.set(name, list)
      }
      list.push(t)
    }
    return [...meta.entries()]
      .sort((a, b) => a[0].localeCompare(b[0]))
      .map(([bundle, items]) => ({ bundle, items }))
  })

  function themeOn(id: string): boolean {
    const disabled = $settings?.disabledBundleThemes
    if (!disabled?.length) return true
    return !disabled.includes(id)
  }

  function setThemeEnabled(id: string, enabled: boolean) {
    if (!$settings) return
    patchSettings({ disabledBundleThemes: toggleDisabledName($settings.disabledBundleThemes, id, enabled) })
      .then(() => {
        editorThemeCards = allEditorThemesForPicker()
        pushMenuThemes()
      })
  }

  function snippetBlurb(s: RegisteredSnippet): string {
    const bits: string[] = []
    if (s.tabTrigger) bits.push(s.tabTrigger)
    if (s.keyEquivalentDisplay) bits.push(s.keyEquivalentDisplay)
    return bits.join(' · ')
  }

  const snippetGroups = $derived.by(() => {
    const q = snippetQuery.trim().toLowerCase()
    const items = [...bundleSnippets]
      .filter((s) => {
        if (!q) return true
        const hay = [s.label, s.id, s.tabTrigger, s.bundleName].join(' ').toLowerCase()
        return hay.includes(q)
      })
    const meta = new Map<string, RegisteredSnippet[]>()
    for (const s of items) {
      const name = s.bundleName || 'Snippets'
      let list = meta.get(name)
      if (!list) {
        list = []
        meta.set(name, list)
      }
      list.push(s)
    }
    return [...meta.entries()]
      .sort((a, b) => a[0].localeCompare(b[0]))
      .map(([bundle, items]) => ({ bundle, items }))
  })

  function snippetOn(id: string): boolean {
    const disabled = $settings?.disabledBundleSnippets
    if (!disabled?.length) return true
    return !disabled.includes(id)
  }

  function setSnippetEnabled(id: string, enabled: boolean) {
    if (!$settings) return
    patchSettings({ disabledBundleSnippets: toggleDisabledName($settings.disabledBundleSnippets, id, enabled) })
  }

  $effect(() => {
    if ($settingsSection !== 'bundle-lsp') return
    let cancelled = false
    lspLoading = true
    lspErr = ''
    LSPService.ListLanguageServers()
      .then((servers) => {
        if (!cancelled) languageServers = servers ?? []
      })
      .catch((err) => {
        if (!cancelled) {
          languageServers = []
          lspErr = err instanceof Error ? err.message : String(err)
        }
      })
      .finally(() => {
        if (!cancelled) lspLoading = false
      })
    return () => { cancelled = true }
  })

  function languageServerEnabled(id: string): boolean {
    const disabled = $settings?.disabledLanguageServers
    if (!disabled?.length) return true
    return !disabled.includes(id)
  }

  function setLanguageServerEnabled(id: string, enabled: boolean) {
    if (!$settings) return
    patchSettings({ disabledLanguageServers: toggleDisabledName($settings.disabledLanguageServers, id, enabled) })
  }

  $effect(() => {
    if ($settingsSection !== 'data') return
    SettingsService.LogsPath()
      .then((p) => { logsPath = p ?? '' })
      .catch(() => { logsPath = '' })
  })

  async function revealLogs() {
    await callOk(SettingsService.RevealLogs(), 'Could not reveal logs')
  }

  async function clearAll() {
    clearing = true
    try {
      if (!(await callOk(SettingsService.ClearAllData(), 'Could not clear data'))) return
      await loadInitial()
      confirmingClear = false
      pushToast('success', 'All data cleared')
      go('welcome')
    } finally {
      clearing = false
    }
  }

  let catalogLoops = $state<LoopInfo[]>(fallbackLoops())
  let catalogPolicies = $state<PolicyInfo[]>(fallbackPolicies())
  const loops = $derived(enabledLoops(catalogLoops, $settings))
  const policies = $derived(enabledPolicies(catalogPolicies, $settings))
  let defaultLoop = $state(defaultLoopId())
  let defaultPolicy = $state(defaultPolicyId())

  $effect(() => {
    defaultLoop = defaultLoopId($settings)
    defaultPolicy = defaultPolicyId($settings)
  })

  $effect(() => {
    if ($settingsSection !== 'loops') return
    void Promise.all([listLoops(), listPolicies()]).then(([ls, ps]) => {
      if (ls.length) catalogLoops = ls
      if (ps.length) catalogPolicies = ps
    })
  })

  function persistLoopPolicy(nextLoop: string, nextPolicy: string) {
    defaultLoop = nextLoop
    defaultPolicy = nextPolicy
    void patchSettings({ defaultLoop: nextLoop, defaultPolicy: nextPolicy })
    const a = autonomyFromPolicyId(nextPolicy)
    if ($policy && $policy.autonomy !== a) {
      void call(PolicyService.SetPolicy({ ...$policy, autonomy: a } as any), 'Could not save policy')
        .then((p) => { if (p) policy.set(p) })
    }
  }

  type SettingsPatch = {
    modelAsk?: string
    modelPlan?: string
    modelAgent?: string
    modelReview?: string
    modelLoop?: string
    modelWorkspaceTitle?: string
    flagsAsk?: Record<string, string>
    flagsPlan?: Record<string, string>
    flagsAgent?: Record<string, string>
    flagsReview?: Record<string, string>
    flagsLoop?: Record<string, string>
    flagsWorkspaceTitle?: Record<string, string>
    childWorkspaceModel?: string
    bypassPermissions?: boolean
    defaultMergeMethod?: string
    defaultUpdateBaseMethod?: string
    defaultSquashCommitMessage?: string
    defaultMergeCommitMessage?: string
    branchNamePrefix?: string
    cursorApiKey?: string
    cursorBridgePath?: string
    codexApiKey?: string
    codexBinPath?: string
    codexChatGPTAuth?: boolean
    codexNetworkAccess?: boolean
    cursorStrictConfig?: boolean
    cursorAttributeCommits?: boolean
    cursorAttributePRs?: boolean
    bundleValues?: Record<string, string>
    preventSleepWhileAgentsRun?: boolean
    queueFollowUps?: boolean
    agentTurnTimeoutMinutes?: number
    scheduledWorkspaceKeep?: number
    maxLoops?: number
    maxAgents?: number
    disabledBundleCommands?: string[]
    disabledBundlePrompts?: string[]
    disabledBundleGrammars?: string[]
    disabledBundleThemes?: string[]
    disabledBundleSnippets?: string[]
    disabledLanguageServers?: string[]
    disabledSkills?: string[]
    mcpServers?: MCPServerRow[]
    simplify?: boolean
    disabledProviders?: string[]
    compatibleProviders?: CompatibleProvider[]
    defaultLoop?: string
    defaultPolicy?: string
  }

  function setProviderEnabled(slug: string, on: boolean) {
    if (!$settings) return
    if (!on && !canDisableProvider(slug, $settings.disabledProviders, cursorReady, codexReady, extraReady)) {
      pushToast('error', 'Keep at least one provider on')
      return
    }
    invalidateModelCatalog()
    void patchSettings({ disabledProviders: toggleDisabledName($settings.disabledProviders, slug, on) })
      .then(() => { void refreshProviderUsage() })
  }

  function patchSettings(patch: SettingsPatch): Promise<typeof $settings | null> {
    if (!$settings) return Promise.resolve(null)
    const next = { ...$settings, ...patch }
    const changed = (Object.keys(patch) as (keyof SettingsPatch)[]).some((k) => {
      const a = next[k] as unknown
      const b = ($settings as any)[k]
      if (Array.isArray(a) || Array.isArray(b)) {
        return JSON.stringify(a ?? []) !== JSON.stringify(b ?? [])
      }
      if (a && b && typeof a === 'object' && typeof b === 'object') {
        return !flagsEqual(a as Record<string, string>, b as Record<string, string>)
      }
      return a !== b
    })
    if (!changed) return Promise.resolve($settings)
    return call(SettingsService.SetSettings(next as any), 'Could not save settings').then((s) => {
      if (s) { settings.set(s); pushToast('success', 'Settings saved') }
      return s
    })
  }

  function parseGovernorCap(raw: string, fallback: number): number {
    const t = raw.trim()
    if (t === '') return fallback
    const n = Number(t)
    if (!Number.isFinite(n)) return fallback
    const v = Math.round(n)
    return v < 0 ? fallback : v
  }

  const navIcons: Record<string, IconName> = {
    providers: 'plug',
    models: 'cpu',
    loops: 'refresh-ccw',
    autonomy: 'refresh-ccw',
    guardrails: 'shield',
    general: 'sliders-horizontal',
    appearance: 'monitor',
    editor: 'code',
    skills: 'slash',
    prs: 'git-pull-request',
    integrations: 'hash',
    mcp: 'plug',
    data: 'trash',
    command: 'command',
    prompt: 'prompt',
    braces: 'braces',
    palette: 'palette',
    code: 'code',
  }
</script>

{#snippet themeModeRow(auto: boolean, setAuto: (on: boolean) => void)}
  <div class="set-row">
    <div class="set-copy">
      <div class="set-title">Theme mode</div>
    </div>
    <div class="set-ctrl">
      <div class="seg" role="group" aria-label="Theme mode">
        <button type="button" class:on={!auto} aria-pressed={!auto} onclick={() => setAuto(false)}>Single theme</button>
        <button type="button" class:on={auto} aria-pressed={auto} onclick={() => setAuto(true)}>Sync with system</button>
      </div>
    </div>
  </div>
{/snippet}

{#snippet editorThemeRows(list: EditorThemeCard[], current: string, pick: (t: EditorThemeCard) => void)}
  {#if list.length === 0}
    <div class="edth-empty">No themes match.</div>
  {:else}
    <div class="edth-list" role="listbox" aria-label="Editor themes">
      {#each list as t, i (t.key)}
        {#if t.pinned && i === 0}
          <div class="edth-label">Recur</div>
        {:else if !t.pinned && (i === 0 || list[i - 1]?.pinned)}
          <div class="edth-label">All themes</div>
        {/if}
        <button
          type="button"
          class="edth-row"
          class:on={current === t.key}
          class:pinned={t.pinned}
          role="option"
          aria-selected={current === t.key}
          onclick={() => pick(t)}
        >
          <span class="edth-swatch" style="background:{t.bg}" aria-hidden="true">
            <i style="background:{t.b}"></i>
            <i style="background:{t.a}"></i>
            <i style="background:{t.c}"></i>
          </span>
          <span class="edth-name">{t.name}</span>
          <span class="edth-kind">{t.kind}</span>
          <Icon name="check" class="edth-check" strokeWidth={2.6} />
        </button>
      {/each}
    </div>
  {/if}
{/snippet}

{#snippet terminalGrid(current: string, pick: (id: string) => void, app: Theme, mode?: 'light' | 'dark')}
  <div class="thgrid">
    <button type="button" class="thcard" class:on={current === TERMINAL_MATCH_EDITOR} onclick={() => pick(TERMINAL_MATCH_EDITOR)} aria-pressed={current === TERMINAL_MATCH_EDITOR}>
      <div class="thprev" style="background:{app.bg}">
        <div class="throw">
          <span class="thdot" style="background:{app.a}"></span>
          <span class="thdot" style="background:{app.ok}"></span>
          <span class="thdot" style="background:{app.warn}"></span>
        </div>
        <span class="tha" style="color:{app.text}">ls<span style="color:{app.ok}"> ~</span></span>
      </div>
      <div class="thmeta"><span class="thn">Match app</span>
        <Icon name="check" class="thcheck" strokeWidth={2.6} /></div>
    </button>
    {#each $terminalThemeCatalog.filter((t) => !mode || (t.uiTheme === 'light') === (mode === 'light')) as t (t.id)}
      <button type="button" class="thcard" class:on={current === t.id} onclick={() => pick(t.id)} aria-pressed={current === t.id}>
        <div class="thprev" style="background:{t.palette.background}">
          <div class="throw">
            <span class="thdot" style="background:{t.palette.red || t.palette.foreground}"></span>
            <span class="thdot" style="background:{t.palette.green || t.palette.foreground}"></span>
            <span class="thdot" style="background:{t.palette.blue || t.palette.foreground}"></span>
          </div>
          <span class="tha" style="color:{t.palette.foreground}">ls<span style="color:{t.palette.green || t.palette.foreground}"> ~</span></span>
        </div>
        <div class="thmeta"><span class="thn">{t.name}</span>
          <Icon name="check" class="thcheck" strokeWidth={2.6} /></div>
      </button>
    {/each}
  </div>
{/snippet}

{#snippet themeGrid(current: string, pick: (key: string) => void, mode?: 'light' | 'dark')}
  <div class="thgrid">
    {#each themes.filter((t) => !mode || isLightTheme(t.key) === (mode === 'light')) as t (t.key)}
      <button type="button" class="thcard" class:on={current === t.key} onclick={() => pick(t.key)} aria-pressed={current === t.key}>
        <div class="thprev" style="background:{t.bg}">
          <div class="throw"><span class="thdot" style="background:{t.a}"></span><span class="thdot" style="background:{t.ok}"></span><span class="thdot" style="background:{t.warn}"></span></div>
          <span class="tha" style="color:{t.text}">recur<span style="color:{t.a}">_</span></span>
        </div>
        <div class="thmeta"><span class="thn">{t.name}</span>
          <Icon name="check" class="thcheck" strokeWidth={2.6} /></div>
      </button>
    {/each}
  </div>
{/snippet}


<section class="view" class:on={$view === 'settings'} id="v-settings">
  <div class="set-shell">
    <aside class="set-nav">
      <label class="set-search">
        <Icon name="search" />
        <input type="search" placeholder="Search settings" bind:value={query} autocomplete="off" spellcheck="false" />
      </label>
      <nav class="set-nav-list" aria-label="Settings">
        {#each filteredNav as g (g.group)}
          <div class="set-group">{g.group}</div>
          {#each g.items as item (item.id)}
            <button
              type="button"
              class="set-nav-item"
              class:on={$settingsSection === item.id}
              aria-current={$settingsSection === item.id ? 'page' : undefined}
              onclick={() => settingsSection.set(item.id)}
            >
              <span class="set-nav-ico"><Icon name={navIcons[item.icon]} /></span>
              {item.label}
            </button>
          {/each}
        {/each}
        {#if !filteredNav.length}
          <div class="set-nav-empty">No matching settings</div>
        {/if}
      </nav>
    </aside>

    <div class="set-main">
      <header class="set-head">
        <h2>{titles[$settingsSection]}</h2>
      </header>
      <div class="set-body">

        {#if $settingsSection === 'agent'}
          <SettingsGroups>
            {#if $settings}
              <SettingsGroup title="Permissions">
                <div class="set-rows code-panel-sides">
                  <div class="set-row">
                    <div class="set-copy">
                      <div class="set-title">Bypass tool permissions</div>
                      <div class="set-desc">Skip permission prompts in Claude Code and Codex</div>
                    </div>
                    <div class="set-ctrl">
                      <button
                        type="button"
                        class="tog"
                        class:on={!!$settings.bypassPermissions}
                        aria-label="toggle bypass permissions"
                        aria-pressed={!!$settings.bypassPermissions}
                        onclick={() => patchSettings({ bypassPermissions: !$settings!.bypassPermissions })}
                      ></button>
                    </div>
                  </div>
                </div>
              </SettingsGroup>
            {/if}
            <SettingsGroup
              title="Claude Code"
              override={claudeOn}
              overrideLabel="Enable Claude Code"
              onOverrideChange={(on) => setProviderEnabled(PROVIDER_CLAUDE, on)}
            >
              <div class="set-rows code-panel-sides">
                  <div class="set-row set-row-stack">
                    <div class="acct-status">
                      {#if claudeBusy && !claudeStatus}
                        <div class="acct-status-head muted">Checking…</div>
                      {:else if claudeStatus?.authenticated}
                        <div class="acct-status-head ok">
                          <span class="acct-status-dot" aria-hidden="true"></span>
                          Connected
                        </div>
                        <dl class="acct-status-table">
                          {#if claudeStatus.path}
                            <div class="acct-status-row"><dt>Path</dt><dd title={claudeStatus.path}>{claudeStatus.path}</dd></div>
                          {/if}
                          {#if claudeStatus.version}
                            <div class="acct-status-row"><dt>Version</dt><dd>{claudeStatus.version}</dd></div>
                          {/if}
                          {#if claudeStatus.provider}
                            <div class="acct-status-row"><dt>Provider</dt><dd>{claudeStatus.provider}</dd></div>
                          {/if}
                          {#if claudeStatus.loginMethod}
                            <div class="acct-status-row"><dt>Login method</dt><dd>{claudeStatus.loginMethod}</dd></div>
                          {/if}
                          {#if claudeStatus.organization}
                            <div class="acct-status-row"><dt>Organization</dt><dd>{claudeStatus.organization}</dd></div>
                          {/if}
                          {#if claudeStatus.email}
                            <div class="acct-status-row"><dt>Email</dt><dd>{claudeStatus.email}</dd></div>
                          {/if}
                        </dl>
                        <div class="set-desc">To switch accounts, run <code>claude auth login</code> in your terminal.</div>
                      {:else}
                        <div class="acct-status-head bad">
                          <span class="acct-status-dot" aria-hidden="true"></span>
                          {claudeStatus?.error || 'Not connected'}
                        </div>
                        {#if claudeStatus?.path || claudeStatus?.version}
                          <dl class="acct-status-table">
                            {#if claudeStatus.path}
                              <div class="acct-status-row"><dt>Path</dt><dd title={claudeStatus.path}>{claudeStatus.path}</dd></div>
                            {/if}
                            {#if claudeStatus.version}
                              <div class="acct-status-row"><dt>Version</dt><dd>{claudeStatus.version}</dd></div>
                            {/if}
                          </dl>
                        {/if}
                        {#if claudeStatus?.version || claudeStatus?.path}
                          <div class="set-desc">Run <code>claude auth login</code> in your terminal, then reopen Settings.</div>
                        {:else}
                          <div class="set-desc">Install Claude Code and run <code>claude auth login</code>, then reopen Settings.</div>
                        {/if}
                      {/if}
                    </div>
                  </div>
                </div>
            </SettingsGroup>

            {#if $settings}
              <SettingsGroup
                title="Cursor"
                override={cursorOn}
                overrideLabel="Enable Cursor"
                onOverrideChange={(on) => setProviderEnabled(PROVIDER_CURSOR, on)}
              >
                <div class="set-rows code-panel-sides">
                    <div class="set-row set-row-stack">
                      <div class="acct-status">
                        <div class="acct-status-bar">
                          {#if cursorConnecting}
                            <div class="acct-status-head muted">Waiting for browser sign-in…</div>
                          {:else if cursorSdkMissing}
                            <div class="acct-status-head warn">
                              <span class="acct-status-dot" aria-hidden="true"></span>
                              Connected, SDK not installed
                            </div>
                          {:else if cursorSignedIn}
                            <div class="acct-status-head ok">
                              <span class="acct-status-dot" aria-hidden="true"></span>
                              Connected
                            </div>
                          {:else}
                            <div class="acct-status-head bad">
                              <span class="acct-status-dot" aria-hidden="true"></span>
                              Not connected
                            </div>
                          {/if}
                          <div class="set-ctrl" class:set-ctrl-pair={cursorSignedIn && !cursorConnecting}>
                            {#if cursorConnecting}
                              <button class="tbtn" type="button" onclick={cancelCursorUsage}>Cancel</button>
                            {:else if cursorSignedIn}
                              <button class="tbtn" type="button" disabled={cursorUsageBusy} onclick={connectCursorUsage}>
                                {cursorUsageBusy ? 'Opening…' : 'Reconnect'}
                              </button>
                              <button class="tbtn" type="button" disabled={cursorUsageBusy} onclick={disconnectCursorUsage}>Disconnect</button>
                            {:else}
                              <button class="tbtn" type="button" disabled={cursorUsageBusy} onclick={connectCursorUsage}>
                                {cursorUsageBusy ? 'Opening…' : 'Connect'}
                              </button>
                            {/if}
                          </div>
                        </div>
                        {#if cursorSdkMissing}
                          <div class="set-note warn acct-sdk-note" role="status">
                            <Icon name="triangle-alert" />
                            <span>
                              {#if installing}
                                {installMsg || 'Installing…'}{installPct >= 0 ? ` · ${installPct}%` : ''}
                              {:else}
                                The Cursor SDK is not installed, so Cursor agents cannot run yet. About 50 MB to download.
                              {/if}
                            </span>
                            <button class="tbtn" type="button" disabled={installing} onclick={installBridge}>
                              {installing ? 'Installing…' : 'Install SDK'}
                            </button>
                          </div>
                        {/if}
                        {#if cursorSignedIn && (cursorAccountName || cursorAccountEmail)}
                          <dl class="acct-status-table">
                            {#if cursorAccountName}
                              <div class="acct-status-row"><dt>Name</dt><dd>{cursorAccountName}</dd></div>
                            {/if}
                            {#if cursorAccountEmail}
                              <div class="acct-status-row"><dt>Email</dt><dd>{cursorAccountEmail}</dd></div>
                            {/if}
                          </dl>
                        {/if}
                      </div>
                    </div>

                    <div class="set-row">
                      <div class="set-copy">
                        <div class="set-title">SDK</div>
                        <div class="set-desc">
                          {#if installing}
                            {installMsg || 'Installing…'}{installPct >= 0 ? ` — ${installPct}%` : ''}
                          {:else if cursorStatus?.bridgeInstalled}
                            Installed{cursorStatus.bridgeVersion ? ` · v${cursorStatus.bridgeVersion}` : ''}{#if cursorStatus.updateAvailable && cursorStatus.pinnedVersion} — update available (v{cursorStatus.pinnedVersion}){/if}
                          {:else}
                            Not installed — about 50 MB to download, 155 MB on disk
                          {/if}
                        </div>
                        {#if installing && installPct >= 0}
                          <div class="prog" role="progressbar" aria-valuenow={installPct} aria-valuemin="0" aria-valuemax="100">
                            <div class="prog-fill" style="width:{installPct}%"></div>
                          </div>
                        {/if}
                      </div>
                      <div class="set-ctrl">
                        <button class="tbtn" type="button" disabled={installing} onclick={installBridge}>
                          {installing ? 'Installing…' : cursorStatus?.updateAvailable ? 'Update' : cursorStatus?.bridgeInstalled ? 'Reinstall' : 'Install'}
                        </button>
                      </div>
                    </div>

                    <div class="set-row">
                      <div class="set-copy">
                        <div class="set-title">Ignore project Cursor config</div>
                        <div class="set-desc">Skip this repo’s <code>.cursor/rules</code> and <code>AGENTS.md</code>, and your <code>~/.cursor</code> settings, when running Cursor agents</div>
                      </div>
                      <div class="set-ctrl">
                        <button
                          type="button"
                          class="tog"
                          class:on={!!$settings.cursorStrictConfig}
                          aria-label="toggle strict cursor config"
                          aria-pressed={!!$settings.cursorStrictConfig}
                          onclick={() => patchSettings({ cursorStrictConfig: !$settings!.cursorStrictConfig })}
                        ></button>
                      </div>
                    </div>

                    <div class="set-row">
                      <div class="set-copy">
                        <div class="set-title">Commit attribution</div>
                        <div class="set-desc">Let Cursor agents append a <code>Made with Cursor</code> trailer to commits they create. Off by default.</div>
                      </div>
                      <div class="set-ctrl">
                        <button
                          type="button"
                          class="tog"
                          class:on={!!$settings.cursorAttributeCommits}
                          aria-label="toggle cursor commit attribution"
                          aria-pressed={!!$settings.cursorAttributeCommits}
                          onclick={() => patchSettings({ cursorAttributeCommits: !$settings!.cursorAttributeCommits })}
                        ></button>
                      </div>
                    </div>

                    <div class="set-row">
                      <div class="set-copy">
                        <div class="set-title">PR attribution</div>
                        <div class="set-desc">Let Cursor agents append attribution to pull requests they open. Off by default.</div>
                      </div>
                      <div class="set-ctrl">
                        <button
                          type="button"
                          class="tog"
                          class:on={!!$settings.cursorAttributePRs}
                          aria-label="toggle cursor PR attribution"
                          aria-pressed={!!$settings.cursorAttributePRs}
                          onclick={() => patchSettings({ cursorAttributePRs: !$settings!.cursorAttributePRs })}
                        ></button>
                      </div>
                    </div>
                  </div>
              </SettingsGroup>

              <SettingsGroup
                title="Codex"
                override={codexOn}
                overrideLabel="Enable Codex"
                onOverrideChange={(on) => setProviderEnabled(PROVIDER_CODEX, on)}
              >
                <div class="set-rows code-panel-sides">
                  <div class="set-row set-row-stack">
                    <div class="acct-status">
                      {#if codexBusy && !codexStatus}
                        <div class="acct-status-head muted">Checking…</div>
                      {:else if !codexConfigured}
                        <div class="acct-status-head bad">
                          <span class="acct-status-dot" aria-hidden="true"></span>
                          Not configured
                        </div>
                        {#if codexStatus?.binPath}
                          <dl class="acct-status-table">
                            <div class="acct-status-row"><dt>Path</dt><dd title={codexStatus.binPath}>{codexStatus.binPath}</dd></div>
                          </dl>
                        {/if}
                        <div class="set-desc">Add an API key or enable ChatGPT auth below to connect Codex models.</div>
                      {:else if codexStatus?.authenticated}
                        <div class="acct-status-head ok">
                          <span class="acct-status-dot" aria-hidden="true"></span>
                          Connected
                        </div>
                        <dl class="acct-status-table">
                          {#if codexStatus.binPath}
                            <div class="acct-status-row"><dt>Path</dt><dd title={codexStatus.binPath}>{codexStatus.binPath}</dd></div>
                          {/if}
                          {#if codexStatus.loginMethod}
                            <div class="acct-status-row"><dt>Login method</dt><dd>{codexStatus.loginMethod}</dd></div>
                          {/if}
                          {#if codexStatus.plan}
                            <div class="acct-status-row"><dt>Plan</dt><dd>{codexStatus.plan}</dd></div>
                          {/if}
                          {#if codexStatus.email}
                            <div class="acct-status-row"><dt>Email</dt><dd>{codexStatus.email}</dd></div>
                          {/if}
                        </dl>
                        <div class="set-desc">To switch ChatGPT accounts, run <code>codex login</code> in your terminal.</div>
                      {:else}
                        <div class="acct-status-head bad">
                          <span class="acct-status-dot" aria-hidden="true"></span>
                          {codexStatus?.error || 'Not connected'}
                        </div>
                        {#if codexStatus?.binPath}
                          <dl class="acct-status-table">
                            <div class="acct-status-row"><dt>Path</dt><dd title={codexStatus.binPath}>{codexStatus.binPath}</dd></div>
                          </dl>
                        {/if}
                        {#if !codexStatus?.binInstalled}
                          <div class="set-desc">Install with <code>brew install --cask codex</code>, or set a path / <code>$RECUR_CODEX</code>.</div>
                        {:else}
                          <div class="set-desc">Set an API key or enable ChatGPT auth, or run <code>codex login</code> in your terminal.</div>
                        {/if}
                      {/if}
                    </div>
                  </div>

                  <div class="set-row">
                    <div class="set-copy">
                      <div class="set-title">Use ChatGPT login</div>
                      <div class="set-desc">Reuse the account from <code>codex login</code> on this machine.</div>
                    </div>
                    <div class="set-ctrl">
                      <button
                        type="button"
                        class="tog"
                        class:on={!!$settings.codexChatGPTAuth}
                        aria-label="toggle Codex ChatGPT auth"
                        aria-pressed={!!$settings.codexChatGPTAuth}
                        onclick={() => {
                          const next = !$settings!.codexChatGPTAuth
                          invalidateModelCatalog()
                          codexBusy = true
                          // Turning ChatGPT login on clears a leftover API key so
                          // the (now-hidden) key cannot keep winning in the runner.
                          const patch: SettingsPatch = { codexChatGPTAuth: next }
                          if (next) {
                            patch.codexApiKey = ''
                            codexKeyDraft = ''
                            codexKeyDirty = false
                            codexKeyTest = null
                          }
                          patchSettings(patch)
                            .then((s) => {
                              if (!s) return null
                              return SettingsService.GetCodexStatus()
                            })
                            .then((s) => { if (s) codexStatus = s })
                            .catch(() => { codexStatus = null })
                            .finally(() => { codexBusy = false })
                        }}
                      ></button>
                    </div>
                  </div>

                  {#if !$settings.codexChatGPTAuth}
                    <div class="set-row set-row-stack">
                      <div class="set-copy">
                        <div class="set-title">API key</div>
                        <div class="set-desc">Exported as <code>CODEX_API_KEY</code>. Stored locally in Recur’s data directory.</div>
                      </div>
                      <div class="path-pick">
                        <input
                          class="inp"
                          type={codexKeyVisible ? 'text' : 'password'}
                          placeholder="sk-…"
                          autocomplete="off"
                          spellcheck="false"
                          aria-label="Codex API key"
                          value={codexKeyDraft}
                          oninput={(e) => {
                            codexKeyDraft = e.currentTarget.value
                            codexKeyDirty = true
                            codexKeyTest = null
                          }}
                        />
                        <button
                          class="tbtn"
                          type="button"
                          aria-label={codexKeyVisible ? 'Hide API key' : 'Show API key'}
                          aria-pressed={codexKeyVisible}
                          title={codexKeyVisible ? 'Hide' : 'Show'}
                          disabled={!codexKeyDraft}
                          onclick={() => { codexKeyVisible = !codexKeyVisible }}
                        >
                          <Icon name={codexKeyVisible ? 'eye-off' : 'eye'} />
                        </button>
                        <button class="tbtn" type="button" disabled={codexKeyTesting || !codexKeyDraft} onclick={testCodexKey}>Test</button>
                        <button class="tbtn" type="button" disabled={!codexKeyDirty || codexKeyTesting} onclick={saveCodexKey}>Save</button>
                      </div>
                      {#if codexKeyTesting}
                        <div class="set-field-msg muted">Testing…</div>
                      {:else if codexKeyTest}
                        <div class="set-field-msg" class:ok={codexKeyTest.ok} class:bad={!codexKeyTest.ok}>{codexKeyTest.text}</div>
                      {/if}
                    </div>
                  {/if}

                  <div class="set-row">
                    <div class="set-copy">
                      <div class="set-title">Sandbox network access</div>
                      <div class="set-desc">Allow Codex Agent commands to use the network. Review/Ask/Plan already can (GitHub <code>gh</code>).</div>
                    </div>
                    <div class="set-ctrl">
                      <button
                        type="button"
                        class="tog"
                        class:on={!!$settings.codexNetworkAccess}
                        aria-label="toggle Codex sandbox network access"
                        aria-pressed={!!$settings.codexNetworkAccess}
                        onclick={() => patchSettings({ codexNetworkAccess: !$settings!.codexNetworkAccess })}
                      ></button>
                    </div>
                  </div>

                  <div class="set-row set-row-stack">
                    <div class="set-copy">
                      <div class="set-title">Binary</div>
                      <div class="set-desc">
                        {#if codexStatus?.binInstalled}
                          Found{codexStatus.binPath ? ` · ${codexStatus.binPath}` : ''}
                        {:else}
                          Install with <code>brew install --cask codex</code>, or set a path / <code>$RECUR_CODEX</code>
                        {/if}
                      </div>
                    </div>
                    <div class="path-pick">
                      <input
                        class="inp"
                        type="text"
                        placeholder={codexStatus?.binPath || '/path/to/codex'}
                        autocomplete="off"
                        spellcheck="false"
                        aria-label="Codex binary path"
                        value={$settings.codexBinPath ?? ''}
                        onchange={(e) => patchSettings({ codexBinPath: e.currentTarget.value })}
                      />
                    </div>
                  </div>
                </div>
              </SettingsGroup>

              {#each compatibleProviders as p (p.id)}
                <SettingsGroup
                  title={p.name || p.id}
                  override={providerEnabled($settings.disabledProviders, p.id)}
                  overrideLabel="Enable {p.name || p.id}"
                  onOverrideChange={(on) => setProviderEnabled(p.id, on)}
                >
                  <CompatibleProviderCard
                    provider={p}
                    testing={!!compatTesting[p.id]}
                    testMsg={compatTest[p.id] ?? null}
                    onChange={updateCompatible}
                    onTest={() => void testCompatible(p)}
                    onRemove={() => removeCompatible(p.id)}
                  />
                </SettingsGroup>
              {/each}

              <SettingsGroup title="OpenAI-compatible">
                <div class="set-rows code-panel-sides">
                  <div class="set-row set-row-stack">
                    <div class="set-copy">
                      <div class="set-title">Add an HTTP endpoint</div>
                      <div class="set-desc">
                        Recur owns the coding loop and calls <code>POST /v1/responses</code>.
                        Add OpenAI, OpenRouter, Groq, or any compatible server. Multiple providers are allowed.
                      </div>
                    </div>
                    <div class="set-ctrl">
                      <button class="tbtn" type="button" onclick={() => { addCompatOpen = true }}>
                        Add provider
                      </button>
                    </div>
                  </div>
                </div>
              </SettingsGroup>
            {/if}
          </SettingsGroups>
        {/if}

        {#if addCompatOpen}
          <CompatibleProviderModal
            takenIds={compatibleProviders.map((p) => p.id)}
            onClose={() => { addCompatOpen = false }}
            onAdd={async (p) => {
              const s = await saveCompatible([...compatibleProviders, p])
              if (s) addCompatOpen = false
            }}
          />
        {/if}

        {#if $settingsSection === 'models'}
          {#if $settings}
            {@const titleRef = modeModelRef('title')}
            <div class="set-rows">
              {#each CHAT_MODES as m (m.value)}
                {@const ref = modeModelRef(m.value)}
                {@const title = m.settingsTitle ?? m.label}
                {@const desc = m.settingsDesc ?? m.hint}
                <div class="set-row">
                  <div class="set-copy">
                    <div class="set-title">{title}</div>
                    <div class="set-desc">{desc}</div>
                  </div>
                  <div class="set-ctrl">
                    <ModelPickers
                      chip ghost
                      models={catalogModels}
                      model={ref}
                      flags={modeFlags(ref, modeFlagsCur(m.value))}
                      modelLabel="{title} model"
                      flagsLabel="{title} flags"
                      onModelChange={(r, next) => setModel(m.value, r, next)}
                      onFlagsChange={(v) => setFlags(m.value, v)}
                    />
                  </div>
                </div>
              {/each}
              <div class="set-row">
                <div class="set-copy">
                  <div class="set-title">Workspace titles</div>
                  <div class="set-desc">Names new workspaces from the first chat message</div>
                </div>
                <div class="set-ctrl">
                  <ModelPickers
                    chip ghost
                    models={catalogModels}
                    model={titleRef}
                    flags={modeFlags(titleRef, $settings.flagsWorkspaceTitle)}
                    modelLabel="Workspace title model"
                    flagsLabel="Workspace title flags"
                    onModelChange={(ref, next) => setModel('title', ref, next)}
                    onFlagsChange={(v) => setFlags('title', v)}
                  />
                </div>
              </div>
            </div>
            <div class="set-rows">
              <div class="set-row">
                <div class="set-copy">
                  <div class="set-title">Child workspace model</div>
                  <div class="set-desc">When Agent opens a child workspace. Mode default uses the model for that child's mode above.</div>
                </div>
                <div class="set-ctrl">
                  <div class="merge-method-radios">
                    {#each [['parent', 'Calling chat'], ['mode', 'Mode default']] as [val, label]}
                      <button type="button" class="mm-radio" class:on={($settings?.childWorkspaceModel || 'parent') === val} onclick={() => patchSettings({ childWorkspaceModel: val })}>
                        <span class="mm-dot"></span>{label}
                      </button>
                    {/each}
                  </div>
                </div>
              </div>
            </div>
          {/if}
        {/if}

        {#if $settingsSection === 'loops'}
          <div class="set-rows">
            <div class="set-row set-row-stack">
              <div class="set-copy">
                <div class="set-title">Default loop and policy</div>
                <div class="set-desc">Used on New Task and when a workspace has not picked its own. A run pins both after it starts.</div>
              </div>
              <LoopPolicyPickers
                bind:loopId={defaultLoop}
                bind:policyId={defaultPolicy}
                {loops}
                {policies}
                ghost={false}
                up={false}
                showTip
                onLoopChange={(id) => persistLoopPolicy(id, defaultPolicy)}
                onPolicyChange={(id) => persistLoopPolicy(defaultLoop, id)}
              />
            </div>
          </div>
        {/if}

        {#if $settingsSection === 'general'}
          <SettingsGroups>
            {#if $settings}
              <SettingsGroup title="Simplify" blurb="Keep agents from over-building. They reuse what’s already here and ship the smallest change that works.">
                <div class="set-rows code-panel-sides">
                  <div class="set-row">
                    <div class="set-copy">
                      <div class="set-title">Default Simplify</div>
                      <div class="set-desc">On by default for new chats</div>
                    </div>
                    <div class="set-ctrl">
                      <button
                        type="button"
                        class="tog"
                        class:on={simplifyEnabled($settings)}
                        aria-label="toggle default simplify"
                        aria-pressed={simplifyEnabled($settings)}
                        onclick={() => patchSettings({ simplify: !simplifyEnabled($settings!) })}
                      ></button>
                    </div>
                  </div>
                </div>
              </SettingsGroup>
              <SettingsGroup title="Concurrency" blurb="How many loops and agent turns can run at once.">
                <div class="set-rows code-panel-sides">
                  <div class="set-row">
                    <div class="set-copy">
                      <div class="set-title">Max concurrent loops</div>
                      <div class="set-desc">Further loops wait in queue until a slot frees. Default 4. 0 is unlimited.</div>
                    </div>
                    <div class="set-ctrl">
                      <input
                        class="inp set-num"
                        type="number"
                        min="0"
                        step="1"
                        value={$settings.maxLoops ?? 4}
                        aria-label="Max concurrent loops"
                        onchange={(e) => {
                          const el = e.currentTarget
                          const loops = parseGovernorCap(el.value, 4)
                          void patchSettings({ maxLoops: loops }).then((s) => {
                            el.value = String(s?.maxLoops ?? $settings?.maxLoops ?? 4)
                          })
                        }}
                      />
                    </div>
                  </div>
                  <div class="set-row">
                    <div class="set-copy">
                      <div class="set-title">Max concurrent agents</div>
                      <div class="set-desc">Agent turns wait for a free slot. Default 8. 0 is unlimited.</div>
                    </div>
                    <div class="set-ctrl">
                      <input
                        class="inp set-num"
                        type="number"
                        min="0"
                        step="1"
                        value={$settings.maxAgents ?? 8}
                        aria-label="Max concurrent agents"
                        onchange={(e) => {
                          const el = e.currentTarget
                          const agents = parseGovernorCap(el.value, 8)
                          void patchSettings({ maxAgents: agents }).then((s) => {
                            el.value = String(s?.maxAgents ?? $settings?.maxAgents ?? 8)
                          })
                        }}
                      />
                    </div>
                  </div>
                </div>
              </SettingsGroup>
              <SettingsGroup title="Agents" blurb="How interactive agents run.">
                <div class="set-rows code-panel-sides">
                  <div class="set-row">
                    <div class="set-copy">
                      <div class="set-title">Turn time limit</div>
                      <div class="set-desc">Minutes before an agent chat turn is stopped. Default 240 (4 hours); max 10080 (7 days). Stop still cancels immediately.</div>
                    </div>
                    <div class="set-ctrl">
                      <input
                        class="inp set-num"
                        type="number"
                        min="5"
                        max="10080"
                        step="1"
                        value={$settings.agentTurnTimeoutMinutes || 240}
                        aria-label="Agent turn time limit in minutes"
                        onchange={(e) => {
                          const el = e.currentTarget
                          const n = Number(el.value)
                          const minutes = Number.isFinite(n) ? Math.round(n) : 240
                          void patchSettings({ agentTurnTimeoutMinutes: minutes }).then((s) => {
                            // Backend clamp can return the already-stored value (e.g. 0 → 240),
                            // so the value={} expression may not change — rewrite the DOM.
                            el.value = String(
                              (s?.agentTurnTimeoutMinutes ?? $settings?.agentTurnTimeoutMinutes) || 240,
                            )
                          })
                        }}
                      />
                    </div>
                  </div>
                  <div class="set-row">
                    <div class="set-copy">
                      <div class="set-title">Queue follow-ups while an agent runs</div>
                      <div class="set-desc">{$settings.queueFollowUps ? '⏎ queues until the turn ends and ⌘⏎ steers.' : '⏎ steers the running turn and ⌘⏎ queues.'}</div>
                    </div>
                    <div class="set-ctrl">
                      <button
                        type="button"
                        class="tog"
                        class:on={!!$settings.queueFollowUps}
                        aria-label="toggle queue follow-ups while an agent runs"
                        aria-pressed={!!$settings.queueFollowUps}
                        onclick={() => patchSettings({ queueFollowUps: !$settings!.queueFollowUps })}
                      ></button>
                    </div>
                  </div>
                </div>
              </SettingsGroup>
              <SettingsGroup title="Power" blurb="Keep the machine awake while agents work.">
                <div class="set-rows code-panel-sides">
                  <div class="set-row">
                    <div class="set-copy">
                      <div class="set-title">Prevent sleep while agents run</div>
                      <div class="set-desc">Uses macOS caffeinate so idle sleep cannot interrupt an agent turn</div>
                    </div>
                    <div class="set-ctrl">
                      <button
                        type="button"
                        class="tog"
                        class:on={!!$settings.preventSleepWhileAgentsRun}
                        aria-label="toggle prevent sleep while agents run"
                        aria-pressed={!!$settings.preventSleepWhileAgentsRun}
                        onclick={() => patchSettings({ preventSleepWhileAgentsRun: !$settings!.preventSleepWhileAgentsRun })}
                      ></button>
                    </div>
                  </div>
                </div>
              </SettingsGroup>
              <SettingsGroup title="Workspaces" blurb="How new workspace branches are named, and how scheduled runs are kept.">
                <div class="set-rows code-panel-sides">
                  <div class="set-row set-row-stack">
                    <div class="set-copy">
                      <div class="set-title">Branch name prefix</div>
                      <div class="set-desc">Prepended to new workspace branch names, e.g. <code aria-live="polite">{branchPrefixExample}</code>. Leave blank for none.</div>
                    </div>
                    <div class="branch-prefix">
                      <div class="branch-prefix-row">
                        <input
                          class="inp"
                          type="text"
                          placeholder="No prefix"
                          autocomplete="off"
                          spellcheck="false"
                          aria-label="Branch name prefix"
                          value={branchPrefixValue}
                          oninput={(e) => {
                            skipPrefixCommit = false
                            branchPrefixDirty = true
                            branchPrefixDraft = e.currentTarget.value
                          }}
                          onfocus={() => { skipPrefixCommit = false }}
                          onchange={(e) => commitBranchPrefix(e.currentTarget.value)}
                        />
                        <div class="seg" role="group" aria-label="Prefix presets">
                          <button
                            type="button"
                            class:on={prefixPreset === 'recur'}
                            aria-pressed={prefixPreset === 'recur'}
                            onmousedown={(e) => { if (e.button === 0) skipPrefixCommit = true }}
                            onclick={() => pickBranchPrefix('recur')}
                          >Recur</button>
                          <button
                            type="button"
                            class:on={prefixPreset === 'github'}
                            aria-pressed={prefixPreset === 'github'}
                            title={githubLogin || 'Requires gh login'}
                            disabled={!githubLogin}
                            onmousedown={(e) => { if (e.button === 0) skipPrefixCommit = true }}
                            onclick={() => pickBranchPrefix('github')}
                          >GitHub</button>
                          <button
                            type="button"
                            class:on={prefixPreset === 'none'}
                            aria-pressed={prefixPreset === 'none'}
                            onmousedown={(e) => { if (e.button === 0) skipPrefixCommit = true }}
                            onclick={() => pickBranchPrefix('none')}
                          >None</button>
                        </div>
                      </div>
                    </div>
                  </div>
                  <div class="set-row">
                    <div class="set-copy">
                      <div class="set-title">Keep scheduled workspaces</div>
                      <div class="set-desc">Keep the last N live workspaces per schedule. Older settled runs are removed. 0 keeps all.</div>
                    </div>
                    <div class="set-ctrl">
                      <input
                        class="inp set-num"
                        type="number"
                        min="0"
                        step="1"
                        value={$settings.scheduledWorkspaceKeep ?? 0}
                        aria-label="Keep last N scheduled workspaces per schedule"
                        onchange={(e) => {
                          const el = e.currentTarget
                          const n = Number(el.value)
                          const keep = Number.isFinite(n) ? Math.max(0, Math.round(n)) : 0
                          void patchSettings({ scheduledWorkspaceKeep: keep }).then((s) => {
                            el.value = String(s?.scheduledWorkspaceKeep ?? $settings?.scheduledWorkspaceKeep ?? 0)
                          })
                        }}
                      />
                    </div>
                  </div>
                </div>
              </SettingsGroup>
            {/if}
          </SettingsGroups>
        {/if}

        {#if $settingsSection === 'appearance'}
          <SettingsGroups>
            <SettingsGroup title="App theme" blurb="Chrome, sidebars, and panels.">
              {@render themeModeRow($themeAuto, setThemeAuto)}
              {#if $themeAuto}
                {#each [{ mode: 'light' as const, label: 'Light mode', cur: $themeLight }, { mode: 'dark' as const, label: 'Dark mode', cur: $themeDark }] as row (row.mode)}
                  <div class="thauto-l">{row.label}</div>
                  {@render themeGrid(row.cur, (key) => setSystemTheme(row.mode, key), row.mode)}
                {/each}
              {:else}
                {@render themeGrid($theme, setTheme)}
              {/if}
            </SettingsGroup>
            <SettingsGroup title="Code editor theme" blurb="How code looks in the editor.">
              {@render themeModeRow($editorThemeAuto, setEditorThemeAuto)}
              {#if !$editorThemeAuto && selectedEditorTheme}
                <button
                  type="button"
                  class="edth-current"
                  title="Show in list"
                  onclick={revealSelectedEditorTheme}
                >
                  <span class="edth-swatch" style="background:{selectedEditorTheme.bg}" aria-hidden="true">
                    <i style="background:{selectedEditorTheme.b}"></i>
                    <i style="background:{selectedEditorTheme.a}"></i>
                    <i style="background:{selectedEditorTheme.c}"></i>
                  </span>
                  <span class="edth-current-copy">
                    <span class="edth-current-label">Selected</span>
                    <span class="edth-current-name">{selectedEditorTheme.name}</span>
                  </span>
                  <span class="edth-kind">{selectedEditorTheme.kind}</span>
                  <Icon name="check" class="edth-check on" strokeWidth={2.6} />
                </button>
              {/if}
              <div class="edth-toolbar">
                <input
                  class="edth-search"
                  type="search"
                  placeholder="Search themes…"
                  bind:value={editorThemeQuery}
                  aria-label="Search editor themes"
                />
                {#if !$editorThemeAuto}
                  <div class="seg" role="group" aria-label="Filter by kind">
                    {#each editorThemeKindFilters as k (k)}
                      <button
                        type="button"
                        class:on={editorThemeKindFilter === k}
                        onclick={() => { editorThemeKindFilter = k }}
                      >{k === 'all' ? 'All' : k === 'dark' ? 'Dark' : 'Light'}</button>
                    {/each}
                  </div>
                {/if}
              </div>
              <div class="edth-box" bind:this={editorThemeBoxEl}>
                {#if $editorThemeAuto}
                  <div class="thauto-l">Light mode</div>
                  {@render editorThemeRows(lightEditorThemes, $editorThemeLight, (t) => setSystemEditorTheme('light', t.key))}
                  <div class="thauto-l">Dark mode</div>
                  {@render editorThemeRows(darkEditorThemes, $editorThemeDark, (t) => setSystemEditorTheme('dark', t.key))}
                {:else}
                  {@render editorThemeRows(editorThemeList, $editorTheme, (t) => { setEditorTheme(t.key); toastEditorTheme(t.name) })}
                {/if}
              </div>
            </SettingsGroup>
            <SettingsGroup title="Terminal theme" blurb="ANSI colours in the dock. Match app follows the chrome palette.">
              {@render themeModeRow($terminalThemeAuto, setTerminalThemeAuto)}
              {#if $terminalThemeAuto}
                <div class="thauto-l">Light mode</div>
                {@render terminalGrid($terminalThemeLight, (id) => setSystemTerminalTheme('light', id), $themeAuto ? appThemeFor('light') : appChromeTheme, 'light')}
                <div class="thauto-l">Dark mode</div>
                {@render terminalGrid($terminalThemeDark, (id) => setSystemTerminalTheme('dark', id), $themeAuto ? appThemeFor('dark') : appChromeTheme, 'dark')}
              {:else}
                {@render terminalGrid($terminalPrefs.theme, (id) => setTerminalPrefs({ theme: id }), appChromeTheme)}
              {/if}
            </SettingsGroup>
          </SettingsGroups>
        {/if}

        {#if $settingsSection === 'editor'}
          <SettingsGroups>
            <SettingsGroup title="Editor" blurb="Font, indent, keymap, and display for the observatory CodeMirror editor.">
              <div class="set-rows code-panel-sides editor-prefs">
                <div class="set-row">
                  <div class="set-copy">
                    <div class="set-title">Font</div>
                    <div class="set-desc">System monospace fonts{#if monoFontsLoading} · scanning…{/if}</div>
                  </div>
                  <div class="set-ctrl">
                    <Select
                      class="font-sel"
                      label="Editor font"
                      value={$editorPrefs.fontFace}
                      options={monoFontOptions}
                      disabled={!monoFontOptions.length}
                      valueStyle={fontValueStyle}
                      optionStyle={fontOptionStyle}
                      onchange={(v) => setEditorPrefs({ fontFace: v })}
                    />
                  </div>
                </div>
                <div class="set-row">
                  <div class="set-copy">
                    <div class="set-title">Font size</div>
                  </div>
                  <div class="set-ctrl">
                    <div class="seg" role="group" aria-label="Editor font size">
                      {#each EDITOR_FONT_SIZES as n (n)}
                        <button
                          type="button"
                          class:on={$editorPrefs.fontSize === n}
                          aria-pressed={$editorPrefs.fontSize === n}
                          onclick={() => setEditorPrefs({ fontSize: n })}
                        >{n}</button>
                      {/each}
                    </div>
                  </div>
                </div>
                <div class="set-row">
                  <div class="set-copy">
                    <div class="set-title">Line height</div>
                  </div>
                  <div class="set-ctrl">
                    <div class="seg" role="group" aria-label="Line height">
                      {#each EDITOR_LINE_HEIGHTS as n (n)}
                        <button
                          type="button"
                          class:on={$editorPrefs.lineHeight === n}
                          aria-pressed={$editorPrefs.lineHeight === n}
                          onclick={() => setEditorPrefs({ lineHeight: n })}
                        >{EDITOR_LINE_HEIGHT_LABELS[n]}</button>
                      {/each}
                    </div>
                  </div>
                </div>
                <div class="set-row">
                  <div class="set-copy">
                    <div class="set-title">Wrap long lines</div>
                  </div>
                  <div class="set-ctrl">
                    <div class="seg" role="group" aria-label="Line wrapping">
                      <button
                        type="button"
                        class:on={$editorPrefs.lineWrapping}
                        aria-pressed={$editorPrefs.lineWrapping}
                        onclick={() => setEditorPrefs({ lineWrapping: true })}
                      >Wrap</button>
                      <button
                        type="button"
                        class:on={!$editorPrefs.lineWrapping}
                        aria-pressed={!$editorPrefs.lineWrapping}
                        onclick={() => setEditorPrefs({ lineWrapping: false })}
                      >Scroll</button>
                    </div>
                  </div>
                </div>
                <div class="set-row">
                  <div class="set-copy">
                    <div class="set-title">Tab size</div>
                    <div class="set-desc">Display width of tab characters.</div>
                  </div>
                  <div class="set-ctrl">
                    <div class="seg" role="group" aria-label="Tab size">
                      {#each EDITOR_TAB_SIZES as n (n)}
                        <button
                          type="button"
                          class:on={$editorPrefs.tabSize === n}
                          aria-pressed={$editorPrefs.tabSize === n}
                          onclick={() => setEditorPrefs({ tabSize: n })}
                        >{n}</button>
                      {/each}
                    </div>
                  </div>
                </div>
                <div class="set-row">
                  <div class="set-copy">
                    <div class="set-title">Default indent</div>
                    <div class="set-desc">What Tab inserts when a language has no override.</div>
                  </div>
                  <div class="set-ctrl">
                    <div class="seg" role="group" aria-label="Default indent style">
                      {#each EDITOR_INDENT_OPTIONS as opt (opt.key)}
                        <button
                          type="button"
                          class:on={$editorPrefs.indent === opt.key}
                          aria-pressed={$editorPrefs.indent === opt.key}
                          onclick={() => setEditorPrefs({ indent: opt.key })}
                        >{opt.label}</button>
                      {/each}
                    </div>
                  </div>
                </div>
              </div>
              <SettingsGroup
                class="set-subsec-nested"
                title="Indent by language"
                blurb="Auto uses common conventions (Go tabs, TS 2 spaces, …). Override anytime."
              >
                <div class="set-rows code-panel-sides indent-lang-rows">
                  {#each EDITOR_INDENT_LANGS as lang (lang.id)}
                    {@const override = $editorPrefs.indentByLang[lang.id]}
                    {@const effective = resolveEditorIndent($editorPrefs, lang.id)}
                    {@const convention = LANGUAGE_INDENT_DEFAULTS[lang.id]}
                    <div class="set-row">
                      <div class="set-copy">
                        <div class="set-title">{lang.label}</div>
                        <div class="set-desc">
                          {#if override}
                            Override · {editorIndentLabel(override)}
                          {:else if convention}
                            Auto · {editorIndentLabel(convention)}{#if lang.hint} ({lang.hint}){/if}
                          {:else}
                            Auto · {editorIndentLabel(effective)} (global)
                          {/if}
                        </div>
                      </div>
                      <div class="set-ctrl">
                        <div class="seg" role="group" aria-label="{lang.label} indent">
                          <button
                            type="button"
                            class:on={!override}
                            aria-pressed={!override}
                            onclick={() => setEditorIndentForLang(lang.id, null)}
                          >Auto</button>
                          {#each EDITOR_INDENT_OPTIONS as opt (opt.key)}
                            <button
                              type="button"
                              class:on={override === opt.key}
                              aria-pressed={override === opt.key}
                              onclick={() => setEditorIndentForLang(lang.id, opt.key)}
                            >{opt.short}</button>
                          {/each}
                        </div>
                      </div>
                    </div>
                  {/each}
                </div>
              </SettingsGroup>
              <div class="set-rows code-panel-sides editor-prefs">
                <div class="set-row">
                  <div class="set-copy">
                    <div class="set-title">Render whitespace</div>
                    <div class="set-desc">Show spaces and tabs as marks.</div>
                  </div>
                  <div class="set-ctrl">
                    <div class="seg" role="group" aria-label="Render whitespace">
                      {#each EDITOR_WHITESPACE_OPTIONS as opt (opt.key)}
                        <button
                          type="button"
                          class:on={$editorPrefs.whitespace === opt.key}
                          aria-pressed={$editorPrefs.whitespace === opt.key}
                          onclick={() => setEditorPrefs({ whitespace: opt.key })}
                        >{opt.label}</button>
                      {/each}
                    </div>
                  </div>
                </div>
                <div class="set-row">
                  <div class="set-copy">
                    <div class="set-title">Special characters</div>
                    <div class="set-desc">Highlight NBSP, zero-width, and other invisible chars.</div>
                  </div>
                  <div class="set-ctrl">
                    <div class="seg" role="group" aria-label="Special characters">
                      <button type="button" class:on={$editorPrefs.specialChars} aria-pressed={$editorPrefs.specialChars} onclick={() => setEditorPrefs({ specialChars: true })}>On</button>
                      <button type="button" class:on={!$editorPrefs.specialChars} aria-pressed={!$editorPrefs.specialChars} onclick={() => setEditorPrefs({ specialChars: false })}>Off</button>
                    </div>
                  </div>
                </div>
                <div class="set-row">
                  <div class="set-copy">
                    <div class="set-title">Line numbers</div>
                  </div>
                  <div class="set-ctrl">
                    <div class="seg" role="group" aria-label="Line numbers">
                      <button type="button" class:on={$editorPrefs.lineNumbers} aria-pressed={$editorPrefs.lineNumbers} onclick={() => setEditorPrefs({ lineNumbers: true })}>On</button>
                      <button type="button" class:on={!$editorPrefs.lineNumbers} aria-pressed={!$editorPrefs.lineNumbers} onclick={() => setEditorPrefs({ lineNumbers: false })}>Off</button>
                    </div>
                  </div>
                </div>
                <div class="set-row">
                  <div class="set-copy">
                    <div class="set-title">Highlight active line</div>
                  </div>
                  <div class="set-ctrl">
                    <div class="seg" role="group" aria-label="Highlight active line">
                      <button type="button" class:on={$editorPrefs.activeLine} aria-pressed={$editorPrefs.activeLine} onclick={() => setEditorPrefs({ activeLine: true })}>On</button>
                      <button type="button" class:on={!$editorPrefs.activeLine} aria-pressed={!$editorPrefs.activeLine} onclick={() => setEditorPrefs({ activeLine: false })}>Off</button>
                    </div>
                  </div>
                </div>
                <div class="set-row">
                  <div class="set-copy">
                    <div class="set-title">Fold gutter</div>
                  </div>
                  <div class="set-ctrl">
                    <div class="seg" role="group" aria-label="Fold gutter">
                      <button type="button" class:on={$editorPrefs.foldGutter} aria-pressed={$editorPrefs.foldGutter} onclick={() => setEditorPrefs({ foldGutter: true })}>On</button>
                      <button type="button" class:on={!$editorPrefs.foldGutter} aria-pressed={!$editorPrefs.foldGutter} onclick={() => setEditorPrefs({ foldGutter: false })}>Off</button>
                    </div>
                  </div>
                </div>
                <div class="set-row">
                  <div class="set-copy">
                    <div class="set-title">Diff gutter</div>
                    <div class="set-desc">Add, modify, and delete hints versus the workspace base.</div>
                  </div>
                  <div class="set-ctrl">
                    <div class="seg" role="group" aria-label="Diff gutter">
                      <button type="button" class:on={$editorPrefs.diffGutter} aria-pressed={$editorPrefs.diffGutter} onclick={() => setEditorPrefs({ diffGutter: true })}>On</button>
                      <button type="button" class:on={!$editorPrefs.diffGutter} aria-pressed={!$editorPrefs.diffGutter} onclick={() => setEditorPrefs({ diffGutter: false })}>Off</button>
                    </div>
                  </div>
                </div>
                <div class="set-row">
                  <div class="set-copy">
                    <div class="set-title">Bracket matching</div>
                  </div>
                  <div class="set-ctrl">
                    <div class="seg" role="group" aria-label="Bracket matching">
                      <button type="button" class:on={$editorPrefs.bracketMatching} aria-pressed={$editorPrefs.bracketMatching} onclick={() => setEditorPrefs({ bracketMatching: true })}>On</button>
                      <button type="button" class:on={!$editorPrefs.bracketMatching} aria-pressed={!$editorPrefs.bracketMatching} onclick={() => setEditorPrefs({ bracketMatching: false })}>Off</button>
                    </div>
                  </div>
                </div>
                <div class="set-row">
                  <div class="set-copy">
                    <div class="set-title">Scroll past end</div>
                    <div class="set-desc">Allow the last lines to sit mid-viewport.</div>
                  </div>
                  <div class="set-ctrl">
                    <div class="seg" role="group" aria-label="Scroll past end">
                      <button type="button" class:on={$editorPrefs.scrollPastEnd} aria-pressed={$editorPrefs.scrollPastEnd} onclick={() => setEditorPrefs({ scrollPastEnd: true })}>On</button>
                      <button type="button" class:on={!$editorPrefs.scrollPastEnd} aria-pressed={!$editorPrefs.scrollPastEnd} onclick={() => setEditorPrefs({ scrollPastEnd: false })}>Off</button>
                    </div>
                  </div>
                </div>
                <div class="set-row">
                  <div class="set-copy">
                    <div class="set-title">Cursor blink</div>
                  </div>
                  <div class="set-ctrl">
                    <div class="seg" role="group" aria-label="Cursor blink">
                      <button type="button" class:on={$editorPrefs.cursorBlink} aria-pressed={$editorPrefs.cursorBlink} onclick={() => setEditorPrefs({ cursorBlink: true })}>On</button>
                      <button type="button" class:on={!$editorPrefs.cursorBlink} aria-pressed={!$editorPrefs.cursorBlink} onclick={() => setEditorPrefs({ cursorBlink: false })}>Off</button>
                    </div>
                  </div>
                </div>
                <div class="set-row">
                  <div class="set-copy">
                    <div class="set-title">Thick caret</div>
                  </div>
                  <div class="set-ctrl">
                    <div class="seg" role="group" aria-label="Thick caret">
                      <button type="button" class:on={$editorPrefs.thickCaret} aria-pressed={$editorPrefs.thickCaret} onclick={() => setEditorPrefs({ thickCaret: true })}>On</button>
                      <button type="button" class:on={!$editorPrefs.thickCaret} aria-pressed={!$editorPrefs.thickCaret} onclick={() => setEditorPrefs({ thickCaret: false })}>Off</button>
                    </div>
                  </div>
                </div>
                <div class="set-row">
                  <div class="set-copy">
                    <div class="set-title">Rectangular selection</div>
                    <div class="set-desc">Option/Alt-drag to select columns.</div>
                  </div>
                  <div class="set-ctrl">
                    <div class="seg" role="group" aria-label="Rectangular selection">
                      <button type="button" class:on={$editorPrefs.rectangularSelect} aria-pressed={$editorPrefs.rectangularSelect} onclick={() => setEditorPrefs({ rectangularSelect: true })}>On</button>
                      <button type="button" class:on={!$editorPrefs.rectangularSelect} aria-pressed={!$editorPrefs.rectangularSelect} onclick={() => setEditorPrefs({ rectangularSelect: false })}>Off</button>
                    </div>
                  </div>
                </div>
                <div class="set-row">
                  <div class="set-copy">
                    <div class="set-title">Drop cursor</div>
                    <div class="set-desc">Show a caret while dragging text.</div>
                  </div>
                  <div class="set-ctrl">
                    <div class="seg" role="group" aria-label="Drop cursor">
                      <button type="button" class:on={$editorPrefs.dropCursor} aria-pressed={$editorPrefs.dropCursor} onclick={() => setEditorPrefs({ dropCursor: true })}>On</button>
                      <button type="button" class:on={!$editorPrefs.dropCursor} aria-pressed={!$editorPrefs.dropCursor} onclick={() => setEditorPrefs({ dropCursor: false })}>Off</button>
                    </div>
                  </div>
                </div>
                <div class="set-row">
                  <div class="set-copy">
                    <div class="set-title">Print margin</div>
                    <div class="set-desc">Vertical ruler at a column.</div>
                  </div>
                  <div class="set-ctrl">
                    <div class="seg" role="group" aria-label="Print margin">
                      {#each EDITOR_RULER_COLUMNS as n (n)}
                        <button
                          type="button"
                          class:on={$editorPrefs.rulerColumn === n}
                          aria-pressed={$editorPrefs.rulerColumn === n}
                          onclick={() => setEditorPrefs({ rulerColumn: n })}
                        >{n === 0 ? 'Off' : n}</button>
                      {/each}
                    </div>
                  </div>
                </div>
                <div class="set-row">
                  <div class="set-copy">
                    <div class="set-title">Keymap</div>
                    <div class="set-desc">Default, Vim, or Emacs bindings.</div>
                  </div>
                  <div class="set-ctrl">
                    <div class="seg" role="group" aria-label="Editor keymap">
                      {#each EDITOR_KEYMAP_OPTIONS as opt (opt.key)}
                        <button
                          type="button"
                          class:on={$editorPrefs.keymap === opt.key}
                          aria-pressed={$editorPrefs.keymap === opt.key}
                          onclick={() => setEditorPrefs({ keymap: opt.key })}
                        >{opt.label}</button>
                      {/each}
                    </div>
                  </div>
                </div>
                <div class="set-row">
                  <div class="set-copy">
                    <div class="set-title">Max open files</div>
                    <div class="set-desc">Tab strip limit. Use 0 for unlimited.</div>
                  </div>
                  <div class="set-ctrl">
                    <input
                      class="inp set-num"
                      type="number"
                      min="0"
                      max="200"
                      step="1"
                      value={$editorPrefs.maxOpenFiles}
                      aria-label="Max open files"
                      onchange={(e) => {
                        const n = Number(e.currentTarget.value)
                        setEditorPrefs({ maxOpenFiles: Number.isFinite(n) ? n : 32 })
                      }}
                    />
                  </div>
                </div>
              </div>
            </SettingsGroup>
            <SettingsGroup title="Terminal" blurb="Font for the terminal dock (⌃`). Defaults to the editor's.">
              <div class="set-rows code-panel-sides editor-prefs">
                  <div class="set-row">
                    <div class="set-copy">
                      <div class="set-title">Font</div>
                      <div class="set-desc">System monospace fonts{#if monoFontsLoading} · scanning…{/if}</div>
                    </div>
                    <div class="set-ctrl">
                      <Select
                        class="font-sel"
                        label="Terminal font"
                        value={$terminalPrefs.fontFace}
                        options={termFontOptions}
                        disabled={!monoFontOptions.length}
                        valueStyle={termFontValueStyle}
                        optionStyle={termFontOptionStyle}
                        onchange={(v) => setTerminalPrefs({ fontFace: v })}
                      />
                    </div>
                  </div>
                  <div class="set-row">
                    <div class="set-copy">
                      <div class="set-title">Font size</div>
                    </div>
                    <div class="set-ctrl">
                      <div class="seg" role="group" aria-label="Terminal font size">
                        <button
                          type="button"
                          class:on={$terminalPrefs.fontSize === 0}
                          aria-pressed={$terminalPrefs.fontSize === 0}
                          onclick={() => setTerminalPrefs({ fontSize: 0 })}
                        >Match</button>
                        {#each EDITOR_FONT_SIZES as n (n)}
                          <button
                            type="button"
                            class:on={$terminalPrefs.fontSize === n}
                            aria-pressed={$terminalPrefs.fontSize === n}
                            onclick={() => setTerminalPrefs({ fontSize: n })}
                          >{n}</button>
                        {/each}
                      </div>
                    </div>
                  </div>
                  <div class="set-row">
                    <div class="set-copy">
                      <div class="set-title">Line height</div>
                      <div class="set-desc">Tighter than the editor fits more rows in the dock.</div>
                    </div>
                    <div class="set-ctrl">
                      <div class="seg" role="group" aria-label="Terminal line height">
                        {#each TERMINAL_LINE_HEIGHTS as n (n)}
                          <button
                            type="button"
                            class:on={$terminalPrefs.lineHeight === n}
                            aria-pressed={$terminalPrefs.lineHeight === n}
                            onclick={() => setTerminalPrefs({ lineHeight: n })}
                          >{TERMINAL_LINE_HEIGHT_LABELS[n]}</button>
                        {/each}
                      </div>
                    </div>
                  </div>
                </div>
            </SettingsGroup>
            <SettingsGroup title="Code panels" blurb="Which side of the editor each Code tool opens on.">
              <div class="set-rows code-panel-sides">
                  {#each CODE_PANEL_KEYS as key (key)}
                    <div class="set-row">
                      <div class="set-copy">
                        <div class="set-title">{CODE_PANEL_LABELS[key]}</div>
                      </div>
                      <div class="set-ctrl">
                        <div class="seg" role="group" aria-label="{CODE_PANEL_LABELS[key]} panel side">
                          <button
                            type="button"
                            class:on={$codePanelSides[key] === 'left'}
                            aria-pressed={$codePanelSides[key] === 'left'}
                            onclick={() => setSide(key, 'left')}
                          >Left</button>
                          <button
                            type="button"
                            class:on={$codePanelSides[key] === 'right'}
                            aria-pressed={$codePanelSides[key] === 'right'}
                            onclick={() => setSide(key, 'right')}
                          >Right</button>
                        </div>
                      </div>
                    </div>
                  {/each}
                </div>
            </SettingsGroup>
            <SettingsGroup title="Files: exclude" blurb="Glob patterns hidden from the Code file explorer. Git-ignored files and folders still appear (dimmed) unless matched here.">
              <div class="exclude-list" role="list" aria-label="Files exclude patterns">
                  {#each $explorerPrefs.exclude as pat, i (i)}
                    <div class="exclude-row" role="listitem">
                      <input
                        class="inp exclude-inp"
                        type="text"
                        value={pat}
                        spellcheck="false"
                        placeholder="**/pattern"
                        aria-label="Exclude pattern {i + 1}"
                        oninput={(e) => updateExclude(i, e.currentTarget.value)}
                      />
                      <button type="button" class="exclude-rm" title="Remove pattern" aria-label="Remove pattern" onclick={() => removeExclude(i)}>
                        <Icon name="x" />
                      </button>
                    </div>
                  {/each}
                  <div class="exclude-actions">
                    <button type="button" class="tbtn" onclick={addExclude}>Add pattern</button>
                    <button type="button" class="tbtn" onclick={resetExplorerExclude}>Reset defaults</button>
                  </div>
                </div>
            </SettingsGroup>
          </SettingsGroups>
        {/if}

        {#if $settingsSection === 'skills'}
          <SettingsGroups>
            <p class="set-lead">Default for Recur and personal skills. Each project can override these (and manage repo skills) on its project page.</p>
            {#if skillsLoading}
              <div class="set-ctrl set-ctrl-text">Loading skills…</div>
            {:else if skillsErr}
              <div class="set-ctrl set-ctrl-text">{skillsErr}</div>
            {:else if !skillGroups.length}
              <div class="set-ctrl set-ctrl-text">No skills found yet.</div>
            {:else}
              {#each skillGroups as group (group.kind)}
                <SettingsGroup title={group.label} blurb={group.blurb}>
                  <div class="skill-list">
                      {#each group.skills as sk (sk.name + sk.path)}
                        {@const on = skillEnabled(sk.name)}
                        {@const loc = group.kind === 'user' ? skillLocation(sk.path, sk.scope) : ''}
                        <div class="skill-row">
                          <div class="skill-copy">
                            <div class="skill-name">/{sk.name}</div>
                            {#if sk.description}
                              <div class="skill-desc">{sk.description}</div>
                            {/if}
                            {#if loc}
                              <div class="skill-path" title={sk.path}>{loc}</div>
                            {/if}
                          </div>
                          <button
                            type="button"
                            class="tog"
                            class:on={on}
                            aria-label="{on ? 'Disable' : 'Enable'} {sk.name}"
                            aria-pressed={on}
                            disabled={!$settings}
                            onclick={() => setSkillEnabled(sk.name, !on)}
                          ></button>
                        </div>
                      {/each}
                    </div>
                </SettingsGroup>
              {/each}
            {/if}
          </SettingsGroups>
        {/if}

        {#if $settingsSection === 'bundle-commands'}
          <SettingsGroups>
            <p class="set-lead">Palette, keybindings, and format-on-save. Off commands leave ⌘⇧P, shortcuts, and willSave.</p>
            {#if cmdsLoading}
              <div class="set-ctrl set-ctrl-text">Loading commands…</div>
            {:else if cmdsErr}
              <div class="set-ctrl set-ctrl-text">{cmdsErr}</div>
            {:else if !bundleGroups.length && !bundleSkips.length}
              <div class="set-ctrl set-ctrl-text">No bundle commands discovered.</div>
            {:else}
              {#if bundleGroups.length}
                {#each bundleGroups as group (group.category)}
                  <SettingsGroup title={group.category}>
                    <div class="set-rows code-panel-sides">
                      {#each group.commands as cmd (cmd.id)}
                        {@const on = commandEnabled(cmd.id)}
                        <div class="set-row">
                          <div class="set-copy">
                            <div class="set-title">{cmd.label}</div>
                          </div>
                          <div class="set-ctrl bundle-cmd-ctrl">
                            {#if cmd.keyEquivalentDisplay}
                              <Kbd class="bundle-cmd-kbd" text={cmd.keyEquivalentDisplay} />
                            {/if}
                            <button
                              type="button"
                              class="tog"
                              class:on={on}
                              aria-label="{on ? 'Disable' : 'Enable'} {cmd.label}"
                              aria-pressed={on}
                              disabled={!$settings}
                              onclick={() => setCommandEnabled(cmd.id, !on)}
                            ></button>
                          </div>
                        </div>
                      {/each}
                    </div>
                  </SettingsGroup>
                {/each}
              {:else}
                <div class="set-ctrl set-ctrl-text">No loadable bundle commands.</div>
              {/if}
              {#if bundleSkips.length}
                <SettingsGroup title="Skipped packs">
                  {#snippet blurbContent()}
                    These directories were found but not loaded. Fix the manifest or run <code>recur bundles lint</code>.
                  {/snippet}
                  <div class="bundle-skips" role="list" aria-label="Skipped bundles">
                    {#each bundleSkips as skip (`${skip.source}:${skip.dir}`)}
                      <div class="bundle-skip" role="listitem" title={skip.dir}>
                        <div class="bundle-skip-name">{skip.name} <span class="bundle-skip-src">{skip.source}</span></div>
                        <div class="bundle-skip-reason">{skip.reason}</div>
                      </div>
                    {/each}
                  </div>
                </SettingsGroup>
              {/if}
            {/if}
          </SettingsGroups>
        {/if}

        {#if $settingsSection === 'bundle-prompts'}
          <SettingsGroups>
            <p class="set-lead">Chat slash verbs. Off prompts leave the composer / menu and titlebar actions that invoke them.</p>
            {#if promptsLoading}
              <div class="set-ctrl set-ctrl-text">Loading prompt actions…</div>
            {:else if promptsErr}
              <div class="set-ctrl set-ctrl-text">{promptsErr}</div>
            {:else if !promptGroups.length}
              <div class="set-ctrl set-ctrl-text">No prompt actions discovered.</div>
            {:else}
              {#each promptGroups as group (group.bundle)}
                <SettingsGroup title={group.bundle}>
                  <div class="skill-list">
                    {#each group.items as p (p.id)}
                      {@const on = promptOn(p.id)}
                      <div class="skill-row">
                        <div class="skill-copy">
                          <div class="skill-name">/{p.slash}</div>
                          {#if p.description || p.name}
                            <div class="skill-desc">{p.description || p.name}</div>
                          {/if}
                        </div>
                        <button
                          type="button"
                          class="tog"
                          class:on={on}
                          aria-label="{on ? 'Disable' : 'Enable'} /{p.slash}"
                          aria-pressed={on}
                          disabled={!$settings}
                          onclick={() => setPromptEnabled(p.id, !on)}
                        ></button>
                      </div>
                    {/each}
                  </div>
                </SettingsGroup>
              {/each}
            {/if}
          </SettingsGroups>
        {/if}

        {#if $settingsSection === 'bundle-grammars'}
          <SettingsGroups>
            <p class="set-lead">Syntax highlighting. Off grammars are not used for those file types.</p>
            {#if grammarsLoading}
              <div class="set-ctrl set-ctrl-text">Loading grammars…</div>
            {:else if grammarsErr}
              <div class="set-ctrl set-ctrl-text">{grammarsErr}</div>
            {:else if !bundleGrammars.length}
              <div class="set-ctrl set-ctrl-text">No grammars discovered.</div>
            {:else}
              <label class="set-search">
                <Icon name="search" />
                <input
                  type="search"
                  placeholder="Search grammars"
                  bind:value={grammarQuery}
                  autocomplete="off"
                  spellcheck="false"
                  aria-label="Search grammars"
                />
              </label>
              {#if !grammarGroups.length}
                <div class="set-ctrl set-ctrl-text">No grammars match.</div>
              {:else}
                {#each grammarGroups as group (group.bundle)}
                  <SettingsGroup title={group.bundle}>
                    <div class="set-rows code-panel-sides">
                      {#each group.items as g (g.id)}
                        {@const on = grammarOn(g.id)}
                        {@const title = grammarTitle(g)}
                        <div class="set-row">
                          <div class="set-copy">
                            <div class="set-title">{title}</div>
                            {#if grammarBlurb(g)}
                              <div class="set-desc">{grammarBlurb(g)}</div>
                            {/if}
                          </div>
                          <div class="set-ctrl">
                            <button
                              type="button"
                              class="tog"
                              class:on={on}
                              aria-label="{on ? 'Disable' : 'Enable'} {title}"
                              aria-pressed={on}
                              disabled={!$settings}
                              onclick={() => setGrammarEnabled(g.id, !on)}
                            ></button>
                          </div>
                        </div>
                      {/each}
                    </div>
                  </SettingsGroup>
                {/each}
              {/if}
            {/if}
          </SettingsGroups>
        {/if}

        {#if $settingsSection === 'bundle-themes'}
          <SettingsGroups>
            <p class="set-lead">Editor colour themes. Off themes leave Settings → Appearance and the View menu.</p>
            {#if themesLoading}
              <div class="set-ctrl set-ctrl-text">Loading themes…</div>
            {:else if themesErr}
              <div class="set-ctrl set-ctrl-text">{themesErr}</div>
            {:else if !bundleThemes.length}
              <div class="set-ctrl set-ctrl-text">No themes discovered.</div>
            {:else}
              <label class="set-search">
                <Icon name="search" />
                <input
                  type="search"
                  placeholder="Search themes"
                  bind:value={themeQuery}
                  autocomplete="off"
                  spellcheck="false"
                  aria-label="Search bundle themes"
                />
              </label>
              {#if !themeGroups.length}
                <div class="set-ctrl set-ctrl-text">No themes match.</div>
              {:else}
                {#each themeGroups as group (group.bundle)}
                  <SettingsGroup title={group.bundle}>
                    <div class="set-rows code-panel-sides">
                      {#each group.items as t (t.id)}
                        {@const on = themeOn(t.id)}
                        <div class="set-row">
                          <div class="set-copy">
                            <div class="set-title">{t.name}</div>
                            <div class="set-desc">{themeBlurb(t)}</div>
                          </div>
                          <div class="set-ctrl">
                            <button
                              type="button"
                              class="tog"
                              class:on={on}
                              aria-label="{on ? 'Disable' : 'Enable'} {t.name}"
                              aria-pressed={on}
                              disabled={!$settings}
                              onclick={() => setThemeEnabled(t.id, !on)}
                            ></button>
                          </div>
                        </div>
                      {/each}
                    </div>
                  </SettingsGroup>
                {/each}
              {/if}
            {/if}
          </SettingsGroups>
        {/if}

        {#if $settingsSection === 'bundle-snippets'}
          <SettingsGroups>
            <p class="set-lead">Tab triggers and snippet keybindings. Off snippets do not expand or appear in autocomplete.</p>
            {#if snippetsLoading}
              <div class="set-ctrl set-ctrl-text">Loading snippets…</div>
            {:else if snippetsErr}
              <div class="set-ctrl set-ctrl-text">{snippetsErr}</div>
            {:else if !bundleSnippets.length}
              <div class="set-ctrl set-ctrl-text">No snippets discovered.</div>
            {:else}
              <label class="set-search">
                <Icon name="search" />
                <input
                  type="search"
                  placeholder="Search snippets"
                  bind:value={snippetQuery}
                  autocomplete="off"
                  spellcheck="false"
                  aria-label="Search snippets"
                />
              </label>
              {#if !snippetGroups.length}
                <div class="set-ctrl set-ctrl-text">No snippets match.</div>
              {:else}
                {#each snippetGroups as group (group.bundle)}
                  <SettingsGroup title={group.bundle}>
                    <div class="set-rows code-panel-sides">
                      {#each group.items as s (s.id)}
                        {@const on = snippetOn(s.id)}
                        {@const blurb = snippetBlurb(s)}
                        <div class="set-row">
                          <div class="set-copy">
                            <div class="set-title">{s.label}</div>
                            {#if blurb}
                              <div class="set-desc">{blurb}</div>
                            {/if}
                          </div>
                          <div class="set-ctrl bundle-cmd-ctrl">
                            {#if s.keyEquivalentDisplay}
                              <Kbd class="bundle-cmd-kbd" text={s.keyEquivalentDisplay} />
                            {/if}
                            <button
                              type="button"
                              class="tog"
                              class:on={on}
                              aria-label="{on ? 'Disable' : 'Enable'} {s.label}"
                              aria-pressed={on}
                              disabled={!$settings}
                              onclick={() => setSnippetEnabled(s.id, !on)}
                            ></button>
                          </div>
                        </div>
                      {/each}
                    </div>
                  </SettingsGroup>
                {/each}
              {/if}
            {/if}
          </SettingsGroups>
        {/if}

        {#if $settingsSection === 'bundle-lsp'}
          <SettingsGroups>
            <p class="set-lead">Language servers for the Observatory editor. Managed servers download into Recur’s data dir on first use; others need the binary on PATH.</p>
            {#if lspLoading}
              <div class="set-ctrl set-ctrl-text">Loading language servers…</div>
            {:else if lspErr}
              <div class="set-ctrl set-ctrl-text">{lspErr}</div>
            {:else if !languageServers.length}
              <div class="set-ctrl set-ctrl-text">No language servers discovered.</div>
            {:else}
              <SettingsGroup title="Language servers">
                <div class="set-rows code-panel-sides">
                  {#each languageServers as ls (ls.id)}
                    {@const on = languageServerEnabled(ls.id)}
                    <div class="set-row">
                      <div class="set-copy">
                        <div class="set-title">
                          {ls.id}
                          {#if !ls.available && !ls.managed}
                            <span class="set-lsp-miss">Not installed</span>
                          {/if}
                        </div>
                        <div class="set-desc">
                          {ls.languages.join(', ')}
                          {#if ls.managed} · managed ({ls.managed}){/if}
                          {#if ls.bundleName} · {ls.bundleName}{/if}
                        </div>
                      </div>
                      <div class="set-ctrl">
                        <button
                          type="button"
                          class="tog"
                          class:on={on}
                          aria-label="{on ? 'Disable' : 'Enable'} {ls.id}"
                          aria-pressed={on}
                          disabled={!$settings}
                          onclick={() => setLanguageServerEnabled(ls.id, !on)}
                        ></button>
                      </div>
                    </div>
                  {/each}
                </div>
              </SettingsGroup>
            {/if}
          </SettingsGroups>
        {/if}

        {#if $settingsSection === 'integrations'}
          {#if bundleSettingsLoading && bundleSettingSchema.length === 0}
            <div class="set-ctrl set-ctrl-text">Loading…</div>
          {:else if bundleSettingsErr}
            <div class="set-ctrl set-ctrl-text">{bundleSettingsErr}</div>
          {:else if bundleSettingSchema.length === 0}
            <div class="set-ctrl set-ctrl-text">Bundles can declare <code>[[setting]]</code> in bundle.toml. Values show up here and are available to that pack’s shebang scripts.</div>
          {:else}
            <SettingsGroups>
              {#each bundleSettingGroups as g (g.bundle)}
                <SettingsGroup title={g.bundle}>
                  <div class="set-rows">
                    {#each g.items as s (s.id)}
                      <div class="set-row set-row-stack">
                        <div class="set-copy">
                          <div class="set-title">{s.label || s.name}</div>
                          {#if s.description}
                            <div class="set-desc">{s.description}</div>
                          {/if}
                        </div>
                        <div class="path-pick">
                          <input
                            class="inp"
                            type={s.secret && !bundleSettingVisible[s.id] ? 'password' : 'text'}
                            placeholder={s.placeholder || ''}
                            autocomplete="off"
                            spellcheck="false"
                            aria-label={s.label || s.name}
                            value={bundleSettingDrafts[s.id] ?? ''}
                            onfocus={() => { bundleSettingFocus = s.id }}
                            oninput={(e) => { bundleSettingDrafts = { ...bundleSettingDrafts, [s.id]: e.currentTarget.value } }}
                            onchange={(e) => commitBundleSetting(s.id, e.currentTarget.value)}
                            onblur={() => { bundleSettingFocus = '' }}
                          />
                          {#if s.secret}
                            <button
                              class="tbtn"
                              type="button"
                              aria-label={bundleSettingVisible[s.id] ? 'Hide value' : 'Show value'}
                              aria-pressed={!!bundleSettingVisible[s.id]}
                              title={bundleSettingVisible[s.id] ? 'Hide' : 'Show'}
                              disabled={!(bundleSettingDrafts[s.id] ?? '')}
                              onclick={() => { bundleSettingVisible = { ...bundleSettingVisible, [s.id]: !bundleSettingVisible[s.id] } }}
                            >
                              <Icon name={bundleSettingVisible[s.id] ? 'eye-off' : 'eye'} />
                            </button>
                          {/if}
                        </div>
                      </div>
                      {#if s.url}
                        <div class="set-row">
                          <div class="set-copy">
                            <div class="set-title">{s.urlLabel || 'Open'}</div>
                            <div class="set-desc">Opens in your browser.</div>
                          </div>
                          <div class="set-ctrl">
                            <button type="button" class="tbtn" onclick={() => void openExternalUrl(s.url)}>{s.urlLabel || 'Open'}</button>
                          </div>
                        </div>
                      {/if}
                    {/each}
                  </div>
                </SettingsGroup>
              {/each}
            </SettingsGroups>
          {/if}
        {/if}

        {#if $settingsSection === 'mcp'}
          <SettingsGroups>
            <p class="set-lead">Linear and other MCP tools chats can use. Each project can override these on its project page.</p>
            <SettingsGroup
              title="Linear"
              override={linearOn}
              overrideLabel="Enable Linear"
              overrideDisabled={!$settings}
              onOverrideChange={(on) => setLinearMCP(on)}
            >
              {#snippet blurbContent()}
                Search and update issues from chat. Uses the API key in Settings → Integrations. Ask, Plan, and Review can look up issues; Agent can change them.
                {#if linearOn && !linearKeySet}
                  <button type="button" class="lnk" style="display:inline;padding:0;border:none;background:none;font:inherit" onclick={() => settingsSection.set('integrations')}>Connect Linear…</button>
                {/if}
              {/snippet}
            </SettingsGroup>
            <SettingsGroup title="Your servers">
              {#if customMCPs.length}
                <div class="skill-list">
                  {#each customMCPs as s (s.id)}
                    <div class="skill-row">
                      <div class="skill-copy">
                        <div class="skill-name">{s.name || s.id}</div>
                        <div class="skill-desc">{s.transport === 'stdio' ? (s.command || 'stdio') : (s.url || 'http')}</div>
                      </div>
                      <button type="button" class="tbtn" onclick={() => startMCPEdit(s)}>Edit</button>
                      <button type="button" class="tbtn" onclick={() => deleteMCP(s.id || '')}>Remove</button>
                      <button
                        type="button"
                        class="tog"
                        class:on={!!s.enabled}
                        aria-label="{s.enabled ? 'Disable' : 'Enable'} {s.name || s.id}"
                        aria-pressed={!!s.enabled}
                        disabled={!$settings}
                        onclick={() => setMCPEnabled(s.id || '', !s.enabled)}
                      ></button>
                    </div>
                  {/each}
                </div>
              {:else if mcpAddKind === ''}
                <div class="set-ctrl set-ctrl-text">None yet.</div>
              {/if}
              {#if mcpAddKind === ''}
                <div class="exclude-actions" style="margin-top:12px">
                  <button type="button" class="tbtn" onclick={() => { resetMCPForm(); mcpAddKind = 'http' }}>Add HTTP</button>
                  <button type="button" class="tbtn" onclick={() => { resetMCPForm(); mcpAddKind = 'stdio' }}>Add stdio</button>
                </div>
              {:else}
                <div class="set-rows" style="margin-top:12px">
                  <div class="set-row set-row-stack">
                    <div class="set-copy"><div class="set-title">Name</div></div>
                    <input class="inp" type="text" placeholder={mcpAddKind === 'stdio' ? 'filesystem' : 'acme'} bind:value={mcpAddName} spellcheck="false" />
                  </div>
                  {#if mcpAddKind === 'http'}
                    <div class="set-row set-row-stack">
                      <div class="set-copy"><div class="set-title">URL</div></div>
                      <input class="inp" type="text" placeholder="https://example.com/mcp" bind:value={mcpAddURL} spellcheck="false" />
                    </div>
                    <div class="set-row set-row-stack">
                      <div class="set-copy"><div class="set-title">Authorization</div><div class="set-desc">{mcpEditID && mcpEditing?.headers?.Authorization ? 'Leave blank to keep the current token.' : 'Optional. A Bearer token if the server needs one.'}</div></div>
                      <input class="inp" type="password" placeholder="Bearer …" bind:value={mcpAddAuth} autocomplete="off" />
                    </div>
                  {:else}
                    <div class="set-row set-row-stack">
                      <div class="set-copy"><div class="set-title">Command</div></div>
                      <input class="inp" type="text" placeholder="npx" bind:value={mcpAddCommand} spellcheck="false" />
                    </div>
                    <div class="set-row set-row-stack">
                      <div class="set-copy"><div class="set-title">Args</div></div>
                      <input class="inp" type="text" placeholder="-y @example/mcp" bind:value={mcpAddArgs} spellcheck="false" />
                    </div>
                  {/if}
                  {#if mcpAddErr}
                    <div class="set-ctrl set-ctrl-text">{mcpAddErr}</div>
                  {/if}
                  <div class="exclude-actions">
                    <button type="button" class="tbtn" onclick={submitMCPAdd}>{mcpEditID ? 'Save' : 'Add'}</button>
                    <button type="button" class="tbtn" onclick={resetMCPForm}>Cancel</button>
                  </div>
                </div>
              {/if}
            </SettingsGroup>
          </SettingsGroups>
        {/if}

        {#if $settingsSection === 'prs'}
          <div class="set-rows">
            <div class="set-row">
              <div class="set-copy">
                <div class="set-title">Default merge method</div>
                <div class="set-desc">How PRs are merged by default. Projects can override this in their settings.</div>
              </div>
              <div class="set-ctrl">
                <div class="merge-method-radios">
                  {#each [['merge', 'Create a merge commit'], ['squash', 'Squash and merge'], ['rebase', 'Rebase and merge']] as [val, label]}
                    <button type="button" class="mm-radio" class:on={($settings?.defaultMergeMethod || 'merge') === val} onclick={() => patchSettings({ defaultMergeMethod: val })}>
                      <span class="mm-dot"></span>{label}
                    </button>
                  {/each}
                </div>
              </div>
            </div>
            <div class="set-row">
              <div class="set-copy">
                <div class="set-title">Default update-with-base method</div>
                <div class="set-desc">How a PR branch catches up when you click Update with base.</div>
              </div>
              <div class="set-ctrl">
                <div class="merge-method-radios">
                  {#each [['merge', 'Merge base into branch'], ['rebase', 'Rebase onto base']] as [val, label]}
                    <button type="button" class="mm-radio" class:on={($settings?.defaultUpdateBaseMethod || 'merge') === val} onclick={() => patchSettings({ defaultUpdateBaseMethod: val })}>
                      <span class="mm-dot"></span>{label}
                    </button>
                  {/each}
                </div>
              </div>
            </div>
            <div class="set-row">
              <div class="set-copy">
                <div class="set-title">Squash commit message</div>
                <div class="set-desc">Presented when squash-merging a pull request. Projects can override this.</div>
              </div>
              <div class="set-ctrl">
                <div class="merge-method-radios">
                  {#each [['title', 'Pull request title'], ['title-and-commits', 'Pull request title and commit details'], ['title-and-description', 'Pull request title and description']] as [val, label]}
                    <button type="button" class="mm-radio" class:on={($settings?.defaultSquashCommitMessage || 'title-and-commits') === val} onclick={() => patchSettings({ defaultSquashCommitMessage: val })}>
                      <span class="mm-dot"></span>{label}
                    </button>
                  {/each}
                </div>
              </div>
            </div>
            <div class="set-row">
              <div class="set-copy">
                <div class="set-title">Merge commit message</div>
                <div class="set-desc">Presented when merging a pull request with a merge commit. Projects can override this.</div>
              </div>
              <div class="set-ctrl">
                <div class="merge-method-radios">
                  {#each [['title', 'Pull request title'], ['title-and-description', 'Pull request title and description']] as [val, label]}
                    <button type="button" class="mm-radio" class:on={($settings?.defaultMergeCommitMessage || 'title') === val} onclick={() => patchSettings({ defaultMergeCommitMessage: val })}>
                      <span class="mm-dot"></span>{label}
                    </button>
                  {/each}
                </div>
              </div>
            </div>
          </div>
        {/if}

        {#if $settingsSection === 'data'}
          <div class="set-rows">
            <div class="set-row">
              <div class="set-copy">
                <div class="set-title">Logs</div>
                <div class="set-desc">{logsPath || 'Process log for crashes and agent failures.'}</div>
              </div>
              <div class="set-ctrl">
                <button type="button" class="tbtn" onclick={revealLogs}>Reveal in Finder</button>
              </div>
            </div>
            <div class="set-row set-row-danger">
              <div class="set-copy">
                <div class="set-title">Clear all data</div>
                <div class="set-desc">Deletes all projects, workspaces, and pull requests. Your settings are kept.</div>
              </div>
              <div class="set-ctrl">
                {#if !confirmingClear}
                  <button type="button" class="tbtn set-danger-btn" onclick={() => (confirmingClear = true)}>
                    <Icon name="trash" />
                    Clear all data
                  </button>
                {:else}
                  <div class="set-confirm">
                    <div class="set-confirm-msg">This can’t be undone.</div>
                    <div class="set-confirm-actions">
                      <button type="button" class="tbtn set-danger-btn solid" onclick={clearAll} disabled={clearing}>{clearing ? 'Clearing…' : 'Yes, clear all'}</button>
                      <button type="button" class="tbtn" onclick={() => (confirmingClear = false)} disabled={clearing}>Cancel</button>
                    </div>
                  </div>
                {/if}
              </div>
            </div>
          </div>
        {/if}

      </div>
    </div>
  </div>
</section>
