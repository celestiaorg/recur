<script module lang="ts">
  // Shared across Observatory instances so a parked shell's cleanup cannot
  // disarm the live workbench's watcher, command handlers, menu, or occluder.
  let watchOwner = ''
  let commandOwner = ''
  let menuOwner = ''
  let occluderOwner = ''
  // Shared so a present_artifact for a workspace that is not mounted still
  // badges when that shell is created.
  let unseenArtifactsByWorkspace = $state.raw<Record<string, readonly string[]>>({})
</script>

<script lang="ts">
  // Code observatory — tree, CodeMirror editor, search, outline, symbols.
  import { tick, onDestroy, untrack, type Snippet } from 'svelte'
  import { Dialogs, Events, Window } from '@wailsio/runtime'
  import {
    ArtifactsService, CodeService, ObservatoryService, WorkspacesService, call, callOk, onWorkspaceFs,
    onFindingsUpdated, onArtifactCreated, onArtifactPresented, bridgeErrorMessage, isDirectoryError, isWorktreeMissing,
  } from '../lib/api'
  import { copyText, readText } from '../lib/clipboard'
  import {
    pushToast, observatoryJump, planPanelTab, workspaces, projects,
    codePanelSides, panelsOnSide, CODE_PANEL_LABELS, isCodePanelKey,
    explorerPrefs, editorPrefs, resolveEditorIndent, theme,
    attention,
    view,
    prs,
    selectedPRId,
    prAgentDockHost,
    prAgentDockOpen,
    prDetailFocus,
    terminalDockOpenBy, terminalDockMaximizedBy, setTerminalDockOpen, isTerminalDockOpen,
    setTerminalDockMaximized,
    editorHiddenBy, setEditorHidden, isEditorHidden,
    terminalBesideBy, terminalBesidePctBy, setTerminalBeside, setTerminalBesidePct,
    isTerminalBeside, terminalBesidePct,
    paletteOpen, commandsOpen, shortcutsOpen, newTaskOpen, newProjectOpen,
    openCommandsPalette,
    openSyntaxModePicker, syntaxModeOpen,
    workspaceScanPending,
    settings,
    type CodePanelKey,
  } from '../lib/stores'
  import {
    bundlesEpoch, commandsForTrigger, listCommands, runCommand, scopeForPath, scratchLanguage,
    setEditorCommandHandler, setEditorFocusProvider,
    type RegisteredCommand,
  } from '../lib/commands'
  import { recordCommandUsage } from '../lib/command-usage'
  import { listSnippets, setEditorSnippetHandler, type RegisteredSnippet } from '../lib/snippets'
  import { outlineOpenFile, symbolAtLine, symbolLookupName } from '../lib/tm-symbols'
  import { tmDocIdFor } from '../lib/cm-tm-doc'
  import { chordFromKeyboardEvent, isReservedAppChord } from '../lib/keyequiv'
  import { isCmdOrCtrl, isLetterKey } from '../lib/shortcut'
  import { keybindingMenu, openKeybindingMenu } from '../lib/keybinding-menu'
  import { get } from 'svelte/store'
  import CommandPrompts from './CommandPrompts.svelte'
  import { isExcludedBy, compileExcludes } from '../lib/glob'
  import { isPathVisibleInExplorerTree } from '../lib/explorer-visible'
  import { scrollDeltaToReveal } from '../lib/workspace-nav'
  import { prAgentWorkspaceId } from '../lib/pr-workspace'
  import { endFileDrag, fileDragPaths, isFileDrag, startFileDrag, isAbsFilesystemPath } from '../lib/file-drag'
  import { fileIconClass } from '../lib/file-icons'
  import {
    loadEditorSession, saveEditorSession, flushEditorSessionNow,
    clearEditorSession, editorContentHash,
    loadExplorerScope, saveExplorerScope,
    maxScratchSeq, pruneScratchDocs, uniqueOpenFiles, remapOpenFiles,
    type ScratchDoc, type StashedBuffer, type Session,
  } from '../lib/editor-session'
  import {
    resolveRestoredViewMode, EDITOR_BODY_CAP, parseFileViews, pruneFileViews,
    peekHotShellEditorScrollTop, relocatePreservingOverflow, restoreHotShellGeometry,
    stashHotShellGeometry, type FileViewState,
  } from '../lib/editor-view-state'
  import {
    forgetEditorHistoryTree, remapEditorHistory,
  } from '../lib/cm-editor-history'
  import {
    getObservatorySnapshot, putObservatorySnapshot, pruneObservatorySnapshots,
    observatoryCacheKey,
    type ObservatorySnapshot,
    type ObservatoryFocusZone,
  } from '../lib/caches/observatory-cache'
  import { pruneSessionTabParks } from '../lib/caches/cm-session-tabs'
  import { captureStripScrollLeft, scrollChildIntoStrip } from '../lib/tab-strip-scroll'
  import { TabStripDrag } from '../lib/tab-strip-drag.svelte'
  import { languageIdForPath } from '../lib/cm-lang'
  import { editorTextEq } from '../lib/editor-dirty'
  import { workspaceFileURL, fileNeedsHTTPBody, fetchWorkspaceFileText, externalFileURL, readExternalEditorFile, isBinaryPreviewMime } from '../lib/workspace-file'
  import { grammarCatalogEpoch, languageLabelForPath } from '../lib/tm-registry'
  import {
    hydrateSyntaxMode, syntaxMapsNonEmpty, syntaxModeEpoch, syntaxModeSnapshot,
    setSyntaxModePersist,
  } from '../lib/syntax-mode'
  import {
    attachLsp, setLspDisplayFile, setLspFileIO, clearLspForWorkspace, lspOutline, lspReferences,
    applyLspQuickFix, fixProblemWithAI,
    notifyDidSave, lspHasDocumentSymbol, type LspAttach, uriToRelPath,
  } from '../lib/lsp'
  import {
    lspProblems,
    lspProblemsForWorkspace, countLspProblems, groupLspProblemsByPath,
    type LspProblem, type LspPathProblemFlag,
  } from '../lib/lsp-problems'
  import { CloseWorkspaceLSP } from '../../bindings/recur/internal/app/lspservice'
  import { onMenuAction, setMenuState, MENU_STATE_IDLE } from '../lib/menu'
  import { setChatOccluder } from '../lib/chat-occlusion'
  import { workspaceDevCmd, runSessionBy, isAnyRunRunning } from '../lib/run-dev'
  import CodeEditor from './CodeEditor.svelte'
  import MergeEditor from './MergeEditor.svelte'
  import FilePreview from './FilePreview.svelte'
  import PdfPreview from './PdfPreview.svelte'
  import AttachmentLightbox from './AttachmentLightbox.svelte'
  import QuickOpen from './QuickOpen.svelte'
  import ResizeHandle from './ResizeHandle.svelte'
  import TerminalDock from './TerminalDock.svelte'
  import TabStrip from './TabStrip.svelte'
  import TabChip from './TabChip.svelte'
  import FindingsPanel, { type FindingsStatusFilter } from './FindingsPanel.svelte'
  import ArtifactsPanel from './ArtifactsPanel.svelte'
  import ReviewSurface from './ReviewSurface.svelte'
  import Icon, { type IconName } from './Icon.svelte'
  import Kbd from './Kbd.svelte'
  import { addUnseenArtifactId, artifactFromEvent, artifactIdFromScratchPath, clearUnseenArtifactIds } from '../lib/artifacts'
  import { findingFlagForPath, findingMatchesRef, findingSevIcon, findingTitle, severityRank, type FindingFlag } from '../lib/findings-ui'
  import type { EditorFindingMark } from '../lib/cm-findings-gutter'
  import { filePreviewFindActive, previewKindForPath, PREVIEW_IFRAME_FIND_KEY, type PreviewKind } from '../lib/preview'
  import { createPreviewScrollStore } from '../lib/preview-scroll'
  import { editorViewModes, isBinaryImagePath, isMediaPath, isPdfPath, mergeViewCurrent, mergeViewReady, nextEditorViewMode, scmGutterBase, shouldHydrateRestoredCurrent, type EditorViewMode } from '../lib/editor-view'
  import { readReviewTab, writeReviewTab, readFileListMode, writeFileListMode, type ReviewTab, type FileListMode } from '../lib/review-diff'
  import { buildTreeFromPaths, capVisibleTree, flattenVisibleTree, pathsUnderDir } from '../lib/changed-tree'
  import { stickyFolderFades } from '../lib/tree-sticky'
  import { overlayScrollbar } from '../lib/overlay-scrollbar'
  import { autosizeTextarea } from '../lib/autosize-textarea'
  import { emojiShortcodes } from '../lib/emoji'
  import { SearchOpts } from '../../bindings/recur/internal/domain/models'
  import type {
    TreeEntry, FileContent, SearchHit, OutlineSymbol, SymbolRef,
    Finding, Commit,
  } from '../../bindings/recur/internal/domain/models'

  const PANEL_ICONS: Record<CodePanelKey, IconName> = {
    maestro: 'message',
    plan: 'list-todo',
    findings: 'hat-glasses',
    files: 'folder-simple',
    scm: 'git-pull-request',
    outline: 'list',
    problems: 'circle-alert',
    search: 'search',
    refs: 'refs',
    artifacts: 'grid',
  }

  let {
    projectId = '',
    workspaceId = '',
    initialPath = '',
    /** Workspace mode: start on changed files / findings instead of full tree. */
    loopScoped = false,
    /** Left dock chat content. When omitted, the chat panel is hidden. */
    leftPanel,
    /** Replaces the left dock's title bar when the Agent/Maestro panel is active
     *  (e.g. solo/pair chat tabs). Receives `lit` — true when this dock has
     *  keyboard focus (amber tab underline), matching editor/terminal tabs.
     *  Falls back to leftTitle / "Maestro". */
    leftHead,
    /** Plan panel content (workspace plan + agent todos). Hidden when omitted. */
    planPanel,
    /** Replaces the Plan pane title — a workspace has at most one plan, so the
     *  chrome holds Details/Todos instead of a redundant "Plan" label. */
    planHead,
    leftOpen = false,
    onToggleLeft,
    /** Overrides the left dock's title/label (default "Maestro"). */
    leftTitle = '',
    /** Solo/pair agent chat tabs — enables ⌘T / ⌘W / ⌥⌘←→ when the agent dock is focused. */
    agentTabCount = 0,
    onAgentNewTab,
    onAgentCloseTab,
    onAgentRotateTab,
    /** Kick off an Agent turn to close open findings (WorkspaceView). */
    onFixFindings,
    /** ⌘L — insert @file / @file:lines / @file:line:col-line:col for the active editor into the agent composer. */
    onAddEditorMention,
    /** Cover the editor + files while a worktree is still allocating. Chat stays visible. */
    mainBlocked = false,
    mainOverlay,
    /** Live workbench. Parked hot shells stay mounted but must not steal focus,
     *  watchers, keybindings, or the native menu. */
    shellActive = true,
  }: {
    projectId?: string
    workspaceId?: string
    initialPath?: string
    loopScoped?: boolean
    leftPanel?: Snippet<[boolean]>
    leftHead?: Snippet<[lit: boolean]>
    planPanel?: Snippet<[selectedArtifactId: string]>
    planHead?: Snippet
    leftOpen?: boolean
    onToggleLeft?: (open: boolean) => void
    leftTitle?: string
    agentTabCount?: number
    onAgentNewTab?: () => void
    onAgentCloseTab?: () => void
    onAgentRotateTab?: (dir: -1 | 1) => void
    onFixFindings?: () => void
    onAddEditorMention?: (path: string, startLine?: number, endLine?: number, startCol?: number, endCol?: number) => void
    mainBlocked?: boolean
    mainOverlay?: Snippet
    shellActive?: boolean
  } = $props()

  const previewScroll = createPreviewScrollStore()

  let tree = $state<TreeEntry[]>([])
  /** True when we skipped ListTree because the worktree isn't on disk yet. */
  let skippedTreeForCreate = false
  /** Recorded WorktreePath whose directory is gone — not the empty-path create case. */
  let worktreeMissing = $state(false)
  let changedPaths = $state<string[]>([])
  /** path → m|a|d|u versus base (loop) or HEAD (project) — explorer badges. */
  let gitStatus = $state<Record<string, string>>({})
  /**
   * False until this root's GitStatus (or the non-git empty stand-in) has
   * settled. Distinguishes "status not loaded yet" from "file has no diff".
   */
  let gitStatusReady = $state(false)
  /** Uncommitted porcelain versus HEAD — drives Commit / Discard. */
  let workingStatus = $state<Record<string, string>>({})
  let stagedFiles = $state<string[]>([])
  let commitMsg = $state('')
  let commitBusy = $state(false)
  let commitMsgBusy = $state(false)
  let browseAll = $state(false)
  /** The user picked a scope explicitly — don't override it automatically. */
  let scopeChosen = $state(false)
  let quickOpen = $state(false)
  let quickOpenMode = $state<'file' | 'symbol'>('file')
  let cursorLine = $state(1)
  let cursorCol = $state(1)
  let closedTabs = $state<string[]>([])
  /** Second editor group (VS Code–style split): independent path + buffer. */
  let splitOpen = $state(false)
  let splitPath = $state('')
  let splitDraft = $state('')
  let splitSaved = $state('')
  let splitWritable = $state(false)
  let focusedPane = $state<'main' | 'split'>('main')
  // Which chrome region last received keyboard focus. Drives the amber-vs-line
  // active-tab underline so you can tell at a glance whether the editor, the
  // terminal, or the agent dock is the focused surface. File explorer and
  // source control are part of the editor workbench, so they share that zone.
  let focusZone = $state<ObservatoryFocusZone>('editor')
  let splitEditor = $state<CodeEditor | undefined>(undefined)

  /** Snapshot the focused CodeEditor for bundle prompt actions (may be empty). */
  export function getFocusedEditorContext(): import('../lib/commands').EditorContext | null {
    const focused =
      focusedPane === 'split' && splitOpen && splitEditor ? splitEditor : editor
    if (!focused) return null
    try {
      return focused.getContext()
    } catch {
      return null
    }
  }
  /** Left pane width as % of the editor row (20–80). */
  let splitPct = $state((() => {
    try {
      const n = Number(localStorage.getItem('recur.editorSplitPct'))
      if (Number.isFinite(n) && n >= 20 && n <= 80) return Math.round(n)
    } catch { /* ignore */ }
    return 50
  })())
  let splitResizing = $state(false)
  let searchRegex = $state(false)
  let searchCase = $state(false)
  let searchGlob = $state('')
  let replaceText = $state('')
  let searchOffset = $state(0)
  let sessionRestored = $state(false)
  let file = $state<FileContent | null>(null)
  /** Selected path is a directory on disk (e.g. git submodule in Changed). */
  let selectedIsDir = $state(false)
  /** Contents at merge-base for Diff mode (CodeMirror unified merge). */
  let baseDoc = $state('')
  let baseDocPath = $state('')
  let outline = $state<OutlineSymbol[]>([])
  let hits = $state<SearchHit[]>([])
  let searchSkippedLarge = $state(0)
  let searchTruncated = $state(false)
  let refs = $state<SymbolRef[]>([])
  /** Active LSP attach for the focused editor path (null = heuristic fallback). */
  let lspAttach = $state<LspAttach>(null)
  let lspAttachGen = 0
  let lspOutlineTimer = 0
  let problemFixBusy = $state('')
  const problemsList = $derived(lspProblemsForWorkspace($lspProblems, workspaceId))
  const problemCounts = $derived(countLspProblems(problemsList))
  const problemsByPath = $derived(groupLspProblemsByPath(problemsList))
  let findings = $state<Finding[]>([])
  /** Finding id currently being status-updated (disables its action buttons). */
  let findingBusyId = $state('')
  let findingsFilter = $state<FindingsStatusFilter>('open')
  /** Deep-link: expand these finding rows (cleared after applied). */
  let findingsExpandIds = $state<string[]>([])
  /** Deep-link: focus note composer for this finding id. */
  let findingsFocusNoteId = $state('')
  let query = $state('')
  let draft = $state('')
  let savedContent = $state('')
  let busy = $state(false)
  /** File body is streaming in; the rest of the workbench stays interactive. */
  let fileLoading = $state(false)
  /**
   * Whether to actually draw the "Loading …" text. Same delayed-reveal rule as
   * the tree skeleton: a local read usually answers in a few ms, and an
   * indicator drawn for two frames reads as a glitch rather than as feedback.
   * Once shown it also stays for a minimum span, so a load that crosses the
   * delay by a hair cannot flash on the way out either.
   */
  let showFileLoading = $state(false)
  const FILE_LOAD_MIN_VISIBLE_MS = 400
  let fileLoadingTimer: ReturnType<typeof setTimeout> | undefined
  let fileLoadingHideTimer: ReturnType<typeof setTimeout> | undefined
  let fileLoadingShownAt = 0
  let fileLoadGen = 0
  let fileLoadAbort: AbortController | null = null
  /** Explorer data for the current root is still loading. */
  let treeLoading = $state(false)
  /**
   * Whether to actually draw placeholder rows. Gated behind a short delay: a
   * local worktree usually answers well inside it, and a skeleton that appears
   * for a couple of frames reads as a flicker — worse than the pane simply being
   * empty for that long. Only a load that genuinely drags gets one.
   */
  let showTreeSkeleton = $state(false)
  const SKELETON_DELAY_MS = 250
  /** Matches Go maxEditBytes — above this the buffer is view-only. */
  const MAX_EDIT_BYTES = 16 * 1024 * 1024
  const utf8 = new TextEncoder()
  function utf8Bytes(text: string): number {
    return utf8.encode(text).byteLength
  }
  function fileWithBody(src: FileContent, body: string): FileContent {
    return { ...src, content: body, size: utf8Bytes(body) }
  }
  let skeletonTimer: ReturnType<typeof setTimeout> | undefined
  /** Bumped on every root change so in-flight loads can tell they're obsolete. */
  let rootToken = 0
  let showSide = $state(true)
  /** Editor pane fills a native full screen window (artifacts, previews). */
  let editorFullscreen = $state(false)
  /** Window was already full screen before the editor went full screen; leave it that way on exit. */
  let windowWasFullscreen = false
  async function setEditorFullscreen(on: boolean) {
    if (on === editorFullscreen) return
    editorFullscreen = on
    if (on) {
      revealFsExit()
      void tick().then(focusFullscreenContent)
    }
    try {
      if (on) {
        windowWasFullscreen = !!(await Window.IsFullscreen())
        if (!windowWasFullscreen) await Window.Fullscreen()
      } else if (!windowWasFullscreen) {
        await Window.UnFullscreen()
      }
    } catch (err: unknown) {
      pushToast('error', `Full screen failed: ${bridgeErrorMessage(err)}`)
    }
  }
  $effect(() => Events.On(Events.Types.Common.WindowUnFullscreen, () => { editorFullscreen = false }))
  $effect(() => {
    void [$view, shellActive]
    untrack(() => void setEditorFullscreen(false))
  })
  /** Keys scroll the preview right away; Esc bubbles from here to .obs-main. */
  function focusFullscreenContent() {
    const pane = obsEl?.querySelector('.obs-main .obs-editor-pane')
    const el = pane?.querySelector<HTMLElement>('iframe')
      ?? pane?.querySelector<HTMLElement>('.file-preview, .file-preview-slot, .obs-pdf-viewport, .obs-image')
    if (!el) return
    if (!el.hasAttribute('tabindex') && !(el instanceof HTMLIFrameElement)) el.tabIndex = -1
    el.focus({ preventScroll: true })
  }
  /** HTML previews forward Esc over postMessage; the open find bar takes it first. */
  function onPreviewIframeMessage(e: MessageEvent) {
    if (!editorFullscreen || e.data?.type !== PREVIEW_IFRAME_FIND_KEY || e.data.key !== 'Escape') return
    const main = obsEl?.querySelector('.obs-main')
    if (!main || main.querySelector('.file-preview-find')) return
    if (![...main.querySelectorAll('iframe')].some((f) => f.contentWindow === e.source)) return
    void setEditorFullscreen(false)
  }
  let fsExitVisible = $state(false)
  let fsExitTimer: ReturnType<typeof setTimeout> | undefined
  function revealFsExit() {
    fsExitVisible = true
    clearTimeout(fsExitTimer)
    fsExitTimer = setTimeout(() => { fsExitVisible = false }, 1500)
  }
  /** Local left-gutter open when parent doesn't own the preference (e.g. Project). */
  let localLeftOpen = $state(false)

  const leftGutterOpen = $derived(onToggleLeft ? leftOpen : localLeftOpen)

  // Terminal dock state belongs to the workspace this Observatory is showing, not
  // to the app — switching workspaces must not carry the drawer along with it.
  const termOpen = $derived(isTerminalDockOpen($terminalDockOpenBy, workspaceId))
  const termMaximized = $derived(termOpen && !!$terminalDockMaximizedBy[workspaceId])
  // Editor hidden = terminal owns the middle panel; toolbar + side docks stay.
  const editorHidden = $derived(isEditorHidden($editorHiddenBy, workspaceId))
  const termBeside = $derived(isTerminalBeside($terminalBesideBy, workspaceId))
  const termBesidePct = $derived(terminalBesidePct($terminalBesidePctBy, workspaceId))
  let termBesideResizing = $state(false)

  // Maximize is an overlay that animates from the dock's slot to the whole
  // Observatory (and back). The store flips immediately; this phase lags so we
  // can FLIP the dock's box and blur the chrome underneath instead of hiding it.
  let termMaxPhase = $state<'idle' | 'enter' | 'max' | 'exit'>('idle')
  let termMaxAnim = $state(false)
  let termSlotH = $state(0)
  let overlayKeepEditorHidden = $state(false)
  let obsEl = $state<HTMLElement | undefined>(undefined)
  const termOverlaying = $derived(termMaxPhase !== 'idle')
  const editorTucked = $derived(termOverlaying ? overlayKeepEditorHidden : editorHidden)
  const termBesideLayout = $derived(termOpen && termBeside && !editorTucked)
  const TERM_MAX_MS = 320
  let termMaxGen = 0
  let termMaxForWs = ''
  let termOpenForWs = ''
  let termWasOpen = false

  function reduceTermMotion(): boolean {
    return typeof matchMedia === 'function' && matchMedia('(prefers-reduced-motion: reduce)').matches
  }

  type OverlayRect = { top: number; left: number; width: number; height: number }

  function overlayRect(el: Element): OverlayRect {
    const r = el.getBoundingClientRect()
    return { top: r.top, left: r.left, width: r.width, height: r.height }
  }

  function applyOverlayRect(r: OverlayRect) {
    if (!obsEl) return
    obsEl.style.setProperty('--term-overlay-top', `${r.top}px`)
    obsEl.style.setProperty('--term-overlay-left', `${r.left}px`)
    obsEl.style.setProperty('--term-overlay-width', `${r.width}px`)
    obsEl.style.setProperty('--term-overlay-height', `${r.height}px`)
  }

  function clearOverlayRect() {
    if (!obsEl) return
    obsEl.style.removeProperty('--term-overlay-top')
    obsEl.style.removeProperty('--term-overlay-left')
    obsEl.style.removeProperty('--term-overlay-width')
    obsEl.style.removeProperty('--term-overlay-height')
  }

  function termDockEl(): HTMLElement | undefined {
    return obsEl?.querySelector('.term-dock') ?? undefined
  }

  function afterPaint(): Promise<void> {
    return new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(() => resolve())))
  }

  function waitDockTransition(dock: HTMLElement): Promise<void> {
    if (reduceTermMotion()) return Promise.resolve()
    return new Promise((resolve) => {
      let settled = false
      const finish = () => {
        if (settled) return
        settled = true
        dock.removeEventListener('transitionend', onEnd)
        clearTimeout(timer)
        resolve()
      }
      const onEnd = (e: TransitionEvent) => {
        if (e.target !== dock) return
        if (e.propertyName !== 'height' && e.propertyName !== 'top' && e.propertyName !== 'width' && e.propertyName !== 'left') return
        finish()
      }
      dock.addEventListener('transitionend', onEnd)
      const timer = setTimeout(finish, TERM_MAX_MS + 80)
    })
  }

  function restoreTargetRect(): OverlayRect | null {
    if (!obsEl) return null
    // Editor-hidden: the dock owned the whole centre column, so shrink back to
    // .obs-main rather than a bottom slot (hidden siblings would pin a slot to the top).
    if (overlayKeepEditorHidden) {
      const main = obsEl.querySelector('.obs-main')
      return main ? overlayRect(main) : overlayRect(obsEl)
    }
    const slot = obsEl.querySelector('.term-dock-slot')
    return slot ? overlayRect(slot) : overlayRect(obsEl)
  }

  async function animateTermFill(gen: number) {
    if (termMaxPhase === 'enter' || termMaxPhase === 'max') return
    const reversing = termMaxPhase === 'exit'
    if (!reversing) overlayKeepEditorHidden = editorHidden
    await tick()
    if (gen !== termMaxGen) return
    const dock = termDockEl()
    if (!dock || !obsEl) {
      termMaxPhase = 'max'
      return
    }
    const from = overlayRect(dock)
    if (!reversing) termSlotH = from.height
    applyOverlayRect(from)
    termMaxAnim = false
    termMaxPhase = 'enter'
    await tick()
    if (gen !== termMaxGen) return
    void focusTerminalPanel()
    const to = overlayRect(obsEl)
    if (reduceTermMotion()) {
      applyOverlayRect(to)
      termMaxPhase = 'max'
      return
    }
    await afterPaint()
    if (gen !== termMaxGen) return
    termMaxAnim = true
    applyOverlayRect(overlayRect(obsEl))
    await waitDockTransition(dock)
    if (gen !== termMaxGen) return
    termMaxAnim = false
    termMaxPhase = 'max'
    applyOverlayRect(overlayRect(obsEl))
  }

  async function animateTermShrink(gen: number) {
    if (termMaxPhase === 'idle' || termMaxPhase === 'exit') return
    const dock = termDockEl()
    if (!dock || !obsEl) {
      termMaxPhase = 'idle'
      clearOverlayRect()
      return
    }
    applyOverlayRect(overlayRect(dock))
    termMaxAnim = false
    termMaxPhase = 'exit'
    await tick()
    if (gen !== termMaxGen) return
    const target = restoreTargetRect()
    if (!target || reduceTermMotion()) {
      termMaxAnim = false
      termMaxPhase = 'idle'
      clearOverlayRect()
      return
    }
    await afterPaint()
    if (gen !== termMaxGen) return
    termMaxAnim = true
    applyOverlayRect(target)
    await waitDockTransition(dock)
    if (gen !== termMaxGen) return
    termMaxAnim = false
    termMaxPhase = 'idle'
    clearOverlayRect()
  }

  $effect(() => {
    const id = workspaceId
    const open = termOpen
    const max = termMaximized
    untrack(() => {
      if (id !== termMaxForWs) {
        termMaxForWs = id
        termMaxGen += 1
        termMaxPhase = 'idle'
        termMaxAnim = false
        clearOverlayRect()
      }
      if (open && max) {
        if (termMaxPhase === 'enter' || termMaxPhase === 'max') return
        const gen = ++termMaxGen
        void animateTermFill(gen)
        return
      }
      if (!open) {
        termMaxGen += 1
        termMaxPhase = 'idle'
        termMaxAnim = false
        clearOverlayRect()
        return
      }
      if (termMaxPhase === 'idle' || termMaxPhase === 'exit') return
      const gen = ++termMaxGen
      void animateTermShrink(gen)
    })
  })

  $effect(() => {
    if (!shellActive) return
    if (!termOverlaying || !obsEl) return
    const el = obsEl
    const sync = () => {
      if (termMaxPhase === 'idle') return
      // RO fires as soon as we observe (and again when the slot lands). Applying
      // the destination rect then would skip the FLIP "from" frame — grow vanished
      // after this observer was added; shrink survived because exit doesn't
      // re-observe. Hold the starting box until the transition class is on.
      if (!termMaxAnim && (termMaxPhase === 'enter' || termMaxPhase === 'exit')) return
      if (termMaxPhase === 'exit') {
        const target = restoreTargetRect()
        if (target) applyOverlayRect(target)
        return
      }
      applyOverlayRect(overlayRect(el))
    }
    const ro = new ResizeObserver(sync)
    ro.observe(el)
    window.addEventListener('resize', sync)
    return () => {
      ro.disconnect()
      window.removeEventListener('resize', sync)
    }
  })

  onDestroy(() => { termMaxGen += 1 })

  function setLeftGutterOpen(open: boolean) {
    if (onToggleLeft) onToggleLeft(open)
    else localLeftOpen = open
  }

  function setShowSide(open: boolean) {
    showSide = open
  }

  /** Observatory left gutter, or the portaled PR dock when that's the visible chat. */
  function setPRAgentDockOpen(open: boolean) {
    const active = document.activeElement
    // Only park focus when the current target is the one being hidden / unmounted.
    // ⌥⌘B must not yank the caret out of a PR comment field.
    const restore = active instanceof Element && (
      open
        ? active.matches('#v-prdetail .prtabs .prd-chat-toggle')
        : !!active.closest('#prd-agent')
    )
    prAgentDockOpen.set(open)
    if (!open) prDetailFocus.set('pr')
    if (!restore) return
    void tick().then(() => {
      const sel = open
        ? '#prd-agent button.prd-chat-toggle'
        : '#v-prdetail .prtabs .prd-chat-toggle'
      document.querySelector<HTMLElement>(sel)?.focus()
    })
  }

  function toggleVisibleLeftPanel() {
    if (portalAgentToPR) setPRAgentDockOpen(!get(prAgentDockOpen))
    else if (hasLeft) setLeftGutterOpen(!leftGutterOpen)
  }

  type PanelMeta = { key: CodePanelKey; label: string; title: string }
  const allPanelMeta: PanelMeta[] = (
    ['maestro', 'plan', 'findings', 'files', 'scm', 'outline', 'problems', 'search', 'refs', 'artifacts'] as CodePanelKey[]
  ).map((key) => ({
    key,
    label: key === 'maestro' && leftTitle ? leftTitle : CODE_PANEL_LABELS[key],
    title: key === 'maestro' && leftTitle ? leftTitle : key === 'refs' ? 'References' : CODE_PANEL_LABELS[key],
  }))

  const availableKeys = $derived(
    allPanelMeta
      .map((p) => p.key)
      .filter((k) =>
        (k !== 'maestro' || !!leftPanel) &&
        (k !== 'plan' || !!planPanel) &&
        (k !== 'findings' || !!workspaceId) &&
        (k !== 'artifacts' || !!workspaceId)),
  )

  const leftPanels = $derived(
    panelsOnSide($codePanelSides, 'left')
      .filter((k) => availableKeys.includes(k))
      .map((key) => allPanelMeta.find((p) => p.key === key)!),
  )
  const rightPanels = $derived(
    panelsOnSide($codePanelSides, 'right')
      .filter((k) => availableKeys.includes(k))
      .map((key) => allPanelMeta.find((p) => p.key === key)!),
  )

  const hasLeft = $derived(leftPanels.length > 0)
  const hasRight = $derived(rightPanels.length > 0)

  let leftKind = $state<CodePanelKey>('maestro')
  let rightKind = $state<CodePanelKey>('files')

  let selectedPath = $state('')
  /** Explorer selection. Folders update this without changing the open editor. */
  let treePath = $state('')
  let highlightLine = $state(0)
  let expanded = $state<Set<string>>(new Set())
  let loadedDirs = $state<Set<string>>(new Set())
  let loadingDirs = $state<Set<string>>(new Set())
  /** Right-click menu on a tree entry (path '' = empty/root area). */
  let ctxMenu = $state<null | { x: number; y: number; path: string; dir: boolean; name: string; tab?: boolean }>(null)
  /** Inline rename / new-file / new-folder name field in the tree. */
  let draftEdit = $state<null | {
    mode: 'rename' | 'new-file' | 'new-folder'
    parent: string
    path: string
    name: string
    dir: boolean
  }>(null)
  /** In-app clipboard for copy/cut/paste of tree entries (not OS clipboard). */
  let fileClip = $state<null | { mode: 'copy' | 'cut'; path: string; dir: boolean }>(null)
  let draftInput = $state<HTMLInputElement | undefined>(undefined)
  let filesPaneEl = $state<HTMLDivElement | undefined>(undefined)
  let leftAsideEl = $state<HTMLElement | undefined>(undefined)
  let termDock = $state<{
    focus: () => void
    newTab: () => void
    closeActiveTab: () => void
    rotateActiveTab: (dir: -1 | 1) => void
  } | undefined>(undefined)
  /** Live terminal tab count — drives Previous/Next Tab menu enablement. */
  let termTabCount = $state(0)
  /** In-session open-file strip. Cap from editorPrefs.maxOpenFiles (0 = unlimited). */
  let openFiles = $state<string[]>([])
  /** Unsaved text for tabs the user left without saving (or closing). */
  let stashedBuffers = $state<Record<string, StashedBuffer>>({})
  /** Scratch/virtual documents opened by command output (`scratch://…`). */
  let scratchDocs = $state<Record<string, ScratchDoc>>({})
  let scratchSeq = 0
  let editor = $state<CodeEditor | undefined>(undefined)
  let mergeEditor = $state<MergeEditor | undefined>(undefined)
  let filePreview = $state<FilePreview | undefined>(undefined)
  /** Find/replace panel open on the focused editor (crumb-bar search toggle). */
  let findPanelOpen = $state(false)
  /** Bundle command prompt dialog state. */
  let promptDialog = $state<{
    open: boolean
    title: string
    prompts: RegisteredCommand['prompts']
    resolve: ((v: Record<string, string> | null) => void) | null
  }>({ open: false, title: '', prompts: [], resolve: null })
  let searchInput = $state<HTMLInputElement | undefined>(undefined)
  /** Code vs Preview vs Diff for the open path. */
  let viewMode = $state<EditorViewMode>('code')
  /** Editor vs PR-style branch review occupying the main column.
   *  Pref is per-workspace (keep-alive snapshot + editor-session file).
   *  Display clamps to editor when the project is not git — isGit can lag a remount. */
  let mainSurfacePref = $state<'editor' | 'review'>('editor')
  /** Workspace whose editor-hidden layout we temporarily lifted to show review. */
  let reviewUnhidFor = ''
  let reviewTab = $state<ReviewTab>(readReviewTab())
  let reviewRefresh = $state(0)
  let reviewCommitCount = $state(0)
  let reviewScopeHash = $state('')
  let reviewWorkingOpen = $state(false)
  let reviewDiffTotals = $state({ add: 0, del: 0 })
  let reviewFileCount = $state(0)
  let reviewRestore = $state<{ selectedPath: string; scrollTop: number } | null>(null)
  let reviewSurface = $state<{
    getViewState: () => { tab: ReviewTab; selectedPath: string; scrollTop: number }
    clearCommitScope: () => void
    openWorkingScope: () => void
    openCommitScope: (c: Commit) => void
  } | undefined>()
  function setMainSurface(next: 'editor' | 'review') {
    const pref = !isGit && next === 'review' ? 'editor' : next
    if (pref === mainSurfacePref) return
    mainSurfacePref = pref
    persistOrClearEditorSession()
  }
  function closeReviewSurface() {
    const restoreId = reviewUnhidFor
    reviewUnhidFor = ''
    setMainSurface('editor')
    if (restoreId) setEditorHidden(restoreId, true)
  }
  function toggleReview() {
    if (mainBlocked) return
    if (mainSurface === 'review') {
      closeReviewSurface()
      return
    }
    setMainSurface('review')
    // Review lives in the same column the editor-hidden rule hides. Un-hide so
    // the toolbar click and ⌥⌘R cannot land on a display:none surface; restore
    // on close unless ⌥⌘E / opening a file changes the layout in between.
    if (workspaceId && editorHidden) {
      reviewUnhidFor = workspaceId
      setEditorHidden(workspaceId, false)
    } else {
      reviewUnhidFor = ''
    }
  }
  function showReviewSurface() {
    if (mainBlocked || !isGit) return
    if (mainSurface !== 'review') {
      setMainSurface('review')
      if (workspaceId && editorHidden) {
        reviewUnhidFor = workspaceId
        setEditorHidden(workspaceId, false)
      } else {
        reviewUnhidFor = ''
      }
    }
  }
  function openWorkingReview() {
    if (mainBlocked || !isGit) return
    showReviewSurface()
    setReviewTab('commits')
    reviewSurface?.openWorkingScope()
  }
  /** Show a chat turn's checkpoint diff in the review surface as a commit scope. */
  export function openTurnReview(c: Commit) {
    if (mainBlocked || !isGit) return
    showReviewSurface()
    setReviewTab('commits')
    reviewSurface?.openCommitScope(c)
  }
  /** Keyboard path only: the inert flip orphans CM/review focus into Files.
   *  Click handlers keep the button they just pressed. */
  function parkAfterReviewToggle(opening: boolean) {
    const token = rootToken
    void tick().then(() => {
      if (rootToken !== token) return
      const stuck = (el: Element | null) => {
        if (!(el instanceof HTMLElement)) return false
        el.focus({ preventScroll: true })
        return document.activeElement === el
      }
      if (opening) {
        // Visible Changes scroller only — Commits keeps a display:none+inert
        // .diffscroll in the DOM, and querySelector lists are document-order
        // (`.obs-main` would otherwise beat its descendant `.obs-review-host`).
        if (stuck(document.querySelector('#v-detail .obs-review-host .obs-review-slot:not(.off) .diffscroll'))) return
        if (stuck(document.querySelector('#v-detail .obs-review-host'))) return
        stuck(document.querySelector('#v-detail .obs-main, .obs-main'))
        return
      }
      const wantSplit = splitOpen && focusedPane === 'split' && !showDiff
      if (wantSplit) splitEditor?.focus()
      else if (showDiff) mergeEditor?.focus()
      else editor?.focus()
      const ae = document.activeElement as HTMLElement | null
      if (ae?.closest('.obs-editors') && !ae.closest('.obs-surface.off')) return
      stuck(document.querySelector('#v-detail .obs-main, .obs-main'))
    })
  }
  function setReviewTab(next: ReviewTab) {
    if (next === 'changes') reviewSurface?.clearCommitScope()
    reviewTab = next
    writeReviewTab(next)
  }
  function cycleReviewTab(dir: -1 | 1) {
    if (!reviewHasCommits) return
    const tabs: ReviewTab[] = ['changes', 'commits']
    const i = Math.max(0, tabs.indexOf(reviewStripTab))
    setReviewTab(tabs[(i + dir + tabs.length) % tabs.length])
  }
  function showEditorSurface() {
    reviewUnhidFor = ''
    setMainSurface('editor')
    if (workspaceId && editorHidden) setEditorHidden(workspaceId, false)
  }
  function toggleEditorSurface() {
    if (!workspaceId || mainBlocked) return
    reviewUnhidFor = ''
    if (mainSurface !== 'editor' || editorHidden) {
      showEditorSurface()
      return
    }
    setEditorHidden(workspaceId, true)
  }

  function toggleTermBeside() {
    if (!workspaceId || mainBlocked || termOverlaying) return
    const next = !termBeside
    setTerminalBeside(workspaceId, next)
    if (next && editorHidden) setEditorHidden(workspaceId, false)
  }
  /** Per-file caret / scroll / view-mode, keyed by path. */
  let fileViews = $state<Record<string, FileViewState>>({})
  /**
   * Cold session restore is applying saved caret/chip. Persist and
   * captureSnapshot must not snapshot the pre-restore editor (line 1) over
   * those coordinates. Held until the restore scroll has actually landed
   * (double-rAF), and scoped to the rootToken that started the restore.
   */
  let restoringSessionViews = false
  let restoringSessionViewsToken = 0
  /** Skip restoring caret/scroll — this open is an explicit go-to-line. */
  let skipViewRestore = $state(false)
  let fileTabStripEl = $state<HTMLDivElement | undefined>(undefined)
  let fileTabFloatEl = $state<HTMLDivElement | undefined>(undefined)
  const fileTabDrag = new TabStripDrag<string>({
    getStrip: () => fileTabStripEl,
    getFloat: () => fileTabFloatEl,
    getItems: () => openFiles,
    setItems: (next) => { openFiles = uniqueOpenFiles(next) },
    keyOf: (p) => p,
    canStart: () => !mainBlocked && openFiles.length > 1,
    bindOwner: () => ({
      getItems: () => openFiles,
      setItems: (next) => { openFiles = uniqueOpenFiles(next) },
    }),
  })
  $effect(() => {
    workspaceId
    fileTabDrag.cancel()
  })
  onDestroy(() => fileTabDrag.cancel())
  /** Keep-alive: restore the file-tab strip offset instead of chasing the active chip. */
  let fileTabScrollRestore = $state<{ left: number; id: number } | undefined>(undefined)
  let fileTabScrollRestoreId = 0
  /** Bump so TabStrip reveals the active chip even when selectedPath is unchanged. */
  let fileTabRevealNonce = $state(0)
  function revealActiveFileTab() {
    fileTabRevealNonce += 1
  }
  /** Path currently in CodeEditor — lags selectedPath until that file's body is ready. */
  let editorPath = $state('')
  /** Last path whose ReadFile failed (binary / not-found / fetch). Diff can still mount. */
  let hydrateFailedPath = $state('')
  /** Per-tab buffers so switching back does not refetch / remount empty. */
  let tabBuffers = $state<Record<string, { draft: string; saved: string; file: FileContent }>>({})
  /** Paths with unsaved edits, including background tabs. */
  let dirtyTabs = $state<Record<string, boolean>>({})

  function markTabDirty(path: string, d: boolean) {
    if (!path) return
    if (!!dirtyTabs[path] === d) return
    if (d) {
      dirtyTabs = { ...dirtyTabs, [path]: true }
      return
    }
    const { [path]: _drop, ...rest } = dirtyTabs
    dirtyTabs = rest
  }

  function tabIsDirty(path: string): boolean {
    if (!path) return false
    if (dirtyTabs[path]) return true
    const buf = tabBuffers[path]
    if (buf && !editorTextEq(buf.draft, buf.saved)) return true
    return pathIsDirty(path)
  }

  function putTabBuffer(path: string, buf: { draft: string; saved: string; file: FileContent }) {
    const clean = editorTextEq(buf.draft, buf.saved)
    if (clean) markTabDirty(path, false)
    if (clean && buf.draft.length > EDITOR_BODY_CAP) {
      if (!tabBuffers[path]) return
      const next = { ...tabBuffers }
      delete next[path]
      tabBuffers = next
      return
    }
    tabBuffers = { ...tabBuffers, [path]: buf }
  }

  const leftMeta = $derived(leftPanels.find((p) => p.key === leftKind) ?? leftPanels[0])
  const rightMeta = $derived(rightPanels.find((p) => p.key === rightKind) ?? rightPanels[0])

  /** Paths changed since base (git status ∪ PR files). */
  const scopePaths = $derived.by(() => {
    const set = new Set<string>([...changedPaths, ...Object.keys(gitStatus)])
    return [...set].sort((a, b) => a.localeCompare(b))
  })
  /**
   * Membership indexes over scopePaths, rebuilt only when it changes. The tree
   * asks "is this node in scope?" once per rendered row, and answering it with
   * a linear scan (plus a second scan over every scope path for directories)
   * made filtered rendering quadratic in a busy worktree.
   */
  const scopeSet = $derived(new Set(scopePaths))
  const scopeDirSet = $derived.by(() => {
    const dirs = new Set<string>()
    for (const p of scopePaths) {
      for (let i = p.indexOf('/'); i >= 0; i = p.indexOf('/', i + 1)) {
        dirs.add(p.slice(0, i))
      }
    }
    return dirs
  })
  /** Compiled once per exclude-list change instead of per node per pattern. */
  const excludeMatchers = $derived(compileExcludes($explorerPrefs.exclude))
  /** Changed-since-base membership, one filter, one number for every surface. */
  const reviewPaths = $derived(scopePaths.filter((p) => !isExcludedBy(p, excludeMatchers)))
  const reviewCount = $derived(reviewPaths.length)
  /**
   * Directory → worst status among its descendants. Previously recomputed by
   * allocating Object.entries(gitStatus) and scanning it in full for every
   * directory row on every render.
   */
  const dirStatusIndex = $derived.by(() => {
    const rank: Record<string, number> = { d: 4, a: 3, u: 2, m: 1 }
    const out = new Map<string, string>()
    for (const [p, st] of Object.entries(gitStatus)) {
      if (!st) continue
      for (let i = p.indexOf('/'); i >= 0; i = p.indexOf('/', i + 1)) {
        const d = p.slice(0, i)
        const cur = out.get(d)
        if ((rank[st] ?? 0) > (rank[cur ?? ''] ?? 0)) out.set(d, st)
      }
    }
    return out
  })
  const showChangedToggle = $derived(scopePaths.length > 0)
  /** Changed tab: same tree, unchanged files filtered out. */
  const filterChanged = $derived(!browseAll && scopePaths.length > 0)
  /** Live-refresh (Changed tab default) vs. a human having explicitly browsed
   * the full tree — governs whether background polling should keep running. */
  const showChangedFirst = $derived(loopScoped && !browseAll && changedPaths.length > 0)
  const selectedGitStatus = $derived(selectedPath ? (gitStatus[selectedPath] || '') : '')
  const workingDirtyCount = $derived(Object.keys(workingStatus).length)
  const openFindingsCount = $derived(findings.filter((f) => (f.status || 'open') === 'open').length)
  const planOpenCount = $derived.by(() => {
    const p = $workspaces.find((w) => w.id === workspaceId)?.plan
    const steps = p?.steps ?? []
    if (!steps.length) return 0
    return steps.filter((s) => s.status !== 'done' && s.status !== 'skipped').length
  })
  /** Unseen finished chat tabs for this workspace (D-attention) — Agent pane badge. */
  const unseenChatCount = $derived(workspaceId ? ($attention.byWorkspace[workspaceId] ?? 0) : 0)
  /**
   * Unseen artifact ids since last visit to the Artifacts panel, keyed by workspace.
   * Shared across hot shells (module $state) so a badge for a background
   * workspace survives until that shell is created. Ids (not a count) so
   * create+present of the same artifact badges as one.
   */
  const unseenArtifactCount = $derived((unseenArtifactsByWorkspace[workspaceId] ?? []).length)
  /** Artifacts list is actually on screen — not merely the selected tab while
   *  Inbox/PRs hide `#v-detail` (visibility, not display:none) or a maximized terminal covers the panes.
   *  Same visibility gates as `maestroPanelVisible` below. */
  const artifactsPanelFocused = $derived(
    shellActive &&
    $view === 'detail' &&
    !termOverlaying &&
    (
      $codePanelSides.artifacts === 'left'
        ? hasLeft && leftGutterOpen && leftKind === 'artifacts'
        : hasRight && showSide && rightKind === 'artifacts'
    ),
  )
  /** This observatory's agent dock should occupy the PR-detail slot. */
  const portalAgentToPR = $derived(
    $view === 'prdetail' &&
    !!workspaceId &&
    prAgentWorkspaceId($prs.find((p) => p.id === $selectedPRId), $workspaces) === workspaceId,
  )

  // The dock is portaled out of .obs, so .obs onfocusin never sees chat / PR
  // chrome. Mirror Observatory's zone mapping onto the PR view.
  $effect(() => {
    if (!shellActive) return
    if (!portalAgentToPR) return
    const focusZoneBeforePR = untrack(() => focusZone)
    untrack(() => {
      const active = document.activeElement
      const agent = active instanceof Element && !!active.closest('#prd-agent, .obs-left')
      focusZone = agent ? 'agent' : 'other'
      prDetailFocus.set(agent ? 'agent' : 'pr')
    })
    const onFocus = (e: FocusEvent) => {
      const el = e.target
      if (!(el instanceof Element) || !el.closest('#v-prdetail')) return
      const agent = !!el.closest('#prd-agent, .obs-left')
      focusZone = agent ? 'agent' : 'other'
      prDetailFocus.set(agent ? 'agent' : 'pr')
    }
    window.addEventListener('focusin', onFocus)
    return () => {
      window.removeEventListener('focusin', onFocus)
      prDetailFocus.set('pr')
      focusZone = focusZoneBeforePR
    }
  })

  /** Agent transcript is on screen — a finish on the selected tab skips unseen. */
  const maestroPanelVisible = $derived(
    shellActive &&
    !!leftPanel &&
    ($view === 'detail' || portalAgentToPR) &&
    (portalAgentToPR || !termOverlaying) &&
    (
      portalAgentToPR
        ? $prAgentDockOpen
        : $codePanelSides.maestro === 'left'
          ? hasLeft && leftGutterOpen && leftKind === 'maestro'
          : hasRight && showSide && rightKind === 'maestro'
    ),
  )

  function attachAgentPortal(node: HTMLElement) {
    const placeholder = document.createComment('agent-dock')
    node.before(placeholder)
    // Reparenting .obs-left resets overflow. Restore in $effect.pre (same as
    // hot-shell park) so the first paint already has scrollLeft. A post-paint
    // write on a visible strip flashes the overlay scrollbar.
    let layoutRo: ResizeObserver | undefined
    let dockWasOpen = false
    const stopLayout = () => {
      layoutRo?.disconnect()
      layoutRo = undefined
    }
    const revealWhenLaidOut = () => {
      stopLayout()
      const tryReveal = () => {
        const strip = node.querySelector<HTMLElement>('.wsv-chat-tabs .tab-strip-scroll')
        const chip = strip?.querySelector<HTMLElement>('.tab-chip.on')
        if (!strip || strip.clientWidth < 1) return false
        restoreHotShellGeometry(node, { defer: false })
        if (chip) scrollChildIntoStrip(strip, chip, 'auto')
        return true
      }
      if (tryReveal()) return
      layoutRo = new ResizeObserver(() => {
        if (tryReveal()) stopLayout()
      })
      layoutRo.observe(node)
      const strip = node.querySelector<HTMLElement>('.wsv-chat-tabs .tab-strip-scroll')
      if (strip) layoutRo.observe(strip)
    }
    $effect.pre(() => {
      const host = $prAgentDockHost
      const open = $prAgentDockOpen
      if (host) {
        const moved = node.parentNode !== host
        if (moved) {
          relocatePreservingOverflow(node, (n) => { host.appendChild(n) })
        } else if (!open) {
          stashHotShellGeometry(node)
        }
        if (open && (moved || !dockWasOpen)) revealWhenLaidOut()
        else stopLayout()
      } else if (placeholder.parentNode && node.parentNode !== placeholder.parentNode) {
        stopLayout()
        relocatePreservingOverflow(node, (n) => { placeholder.after(n) })
      }
      dockWasOpen = !!(host && open)
    })
    return () => {
      // Hide-left unmounts this aside; Svelte runs this *after* the node is
      // detached. Putting it back after the comment left a stray .obs-left in
      // the 1-column observatory grid and stacked the editor under the chat.
      // Leaving the PR tears the attachment down while the aside is still live
      // (in the PR host) — only then do we move it home.
      stopLayout()
      if (node.isConnected && placeholder.parentNode) {
        relocatePreservingOverflow(node, (n) => { placeholder.after(n) })
      }
      placeholder.remove()
    }
  }
  $effect(() => {
    if (!shellActive) return
    const id = workspaceId
    occluderOwner = id
    setChatOccluder('agent-pane', !!leftPanel && !maestroPanelVisible)
    return () => {
      if (occluderOwner === id) {
        occluderOwner = ''
        setChatOccluder('agent-pane', false)
      }
    }
  })
  function paneBadgeCount(key: CodePanelKey): number {
    if (key === 'maestro') return unseenChatCount
    if (key === 'scm') return workingDirtyCount
    if (key === 'findings') return openFindingsCount
    if (key === 'plan') return planOpenCount
    if (key === 'artifacts') return unseenArtifactCount
    if (key === 'problems') return problemCounts.total
    return 0
  }
  function paneBadgeTitle(p: PanelMeta): string {
    const n = paneBadgeCount(p.key)
    if (!n) return p.title
    if (p.key === 'maestro') return `${p.title} (${n} unread)`
    if (p.key === 'scm') return `${p.title} (${n})`
    if (p.key === 'findings') return `${p.title} (${n} open)`
    if (p.key === 'plan') return `${p.title} (${n} remaining)`
    if (p.key === 'artifacts') return `${p.title} (${n} new)`
    if (p.key === 'problems') return `${p.title} (${problemCounts.errors}E ${problemCounts.warnings}W)`
    return p.title
  }
  function paneBadgeLabel(n: number): string {
    return n > 9 ? '9+' : String(n)
  }
  const findingsFilterCounts = $derived({
    open: openFindingsCount,
    ignored: findings.filter((f) => f.status === 'ignored').length,
    invalid: findings.filter((f) => f.status === 'invalid').length,
    resolved: findings.filter((f) => f.status === 'resolved').length,
    all: findings.length,
  })
  const FINDINGS_FILTERS: { key: FindingsStatusFilter; label: string }[] = [
    { key: 'open', label: 'Open' },
    { key: 'ignored', label: 'Ignored' },
    { key: 'invalid', label: 'Invalid' },
    { key: 'resolved', label: 'Resolved' },
    { key: 'all', label: 'All' },
  ]
  /** Normalize a finding file path to the same rel-path the editor tabs use. */
  function findingRelPath(file: string): string {
    const wt = $workspaces.find((w) => w.id === workspaceId)?.worktreePath?.replace(/\/$/, '') ?? ''
    let p = file.replace(/\\/g, '/').replace(/^\.\//, '')
    if (wt && (p === wt || p.startsWith(wt + '/'))) p = p.slice(wt.length + 1)
    return p
  }
  // Rel-path → open finding count + worst severity, for explorer flags.
  const openFindingsByPath = $derived.by(() => {
    const map = new Map<string, FindingFlag>()
    for (const f of findings) {
      if ((f.status || 'open') !== 'open' || !f.file) continue
      const p = findingRelPath(f.file)
      if (!p || p.startsWith('/')) continue
      const cur = map.get(p)
      const sev = String(f.severity || 'info')
      if (!cur) map.set(p, { count: 1, severity: sev })
      else {
        cur.count++
        if (severityRank(sev) > severityRank(cur.severity)) cur.severity = sev
      }
    }
    return map
  })
  const reviewFindingsByPath = $derived.by(() => {
    const m = new Map<string, Finding[]>()
    for (const f of findings) {
      if ((f.status || 'open') !== 'open' || !f.file) continue
      const p = findingRelPath(f.file)
      if (!p || p.startsWith('/')) continue
      m.set(p, [...(m.get(p) ?? []), f])
    }
    return m
  })
  const reviewDirtyPaths = $derived(new Set(Object.keys(workingStatus)))
  async function openReviewFinding(f: Finding) {
    if (!f.id) return
    findingsExpandIds = [f.id]
    await selectCodePanel('findings')
  }
  /** Open findings for an editor path → gutter marks (severity dots). */
  function findingMarksForPath(path: string): EditorFindingMark[] {
    if (!path) return []
    const out: EditorFindingMark[] = []
    for (const f of findings) {
      if ((f.status || 'open') !== 'open' || !f.id || !f.file) continue
      if (findingRelPath(f.file) !== path) continue
      if (!f.line || f.line < 1) continue
      out.push({
        id: f.id,
        line: f.line,
        endLine: f.endLine && f.endLine > f.line ? f.endLine : undefined,
        severity: String(f.severity || 'info'),
        title: findingTitle(f.message || ''),
      })
    }
    return out
  }
  const mainFindingMarks = $derived(findingMarksForPath(editorPath || selectedPath))
  /**
   * Workspace identity for the CodeEditors. Deliberately NOT derived from the
   * projectId/workspaceId props: the editor's other props (editorPath, draft,
   * fileViews) only swap when the root-switch $effect applies the new
   * workspace's snapshot, and effect ordering does not guarantee that runs
   * before the editor's own $effect. A derived key could reach the editor
   * ahead of the snapshot swap — it would rebuild for the "new" session out
   * of the old workspace's buffer, then the doc-drift path carries the old
   * worktree's scroll into the new one (same relative path converging both
   * workspaces on one position). Written by the switch $effect instead, in
   * the same synchronous batch as the snapshot swap, so the editor can never
   * observe a torn prop set.
   */
  let editorSessionKey = $state('')
  const splitFindingMarks = $derived(findingMarksForPath(splitPath))
  async function openFindingFromEditor(id: string) {
    if (!id) return
    await selectCodePanel('findings')
    findingsFilter = 'open'
    findingsExpandIds = [id]
    await tick()
    findingsExpandIds = []
  }
  const stagedSet = $derived(new Set(stagedFiles))
  const workingPaths = $derived(Object.keys(workingStatus).sort((a, b) => a.localeCompare(b)))
  const stagedPaths = $derived(workingPaths.filter((p) => stagedSet.has(p)))
  const unstagedPaths = $derived(workingPaths.filter((p) => !stagedSet.has(p)))
  let scmFileListMode = $state<FileListMode>(readFileListMode())
  function setScmFileListMode(next: FileListMode) {
    scmFileListMode = next
    writeFileListMode(next)
  }
  let scmCollapsed = $state<Set<string>>(new Set())
  function toggleScmDir(path: string) {
    const next = new Set(scmCollapsed)
    if (next.has(path)) next.delete(path)
    else next.add(path)
    scmCollapsed = next
  }
  const stagedTree = $derived(buildTreeFromPaths(stagedPaths))
  const unstagedTree = $derived(buildTreeFromPaths(unstagedPaths))
  /** Focused pane drives chrome (breadcrumb, status) and where opens land. */
  const activePath = $derived(
    focusedPane === 'split' && splitOpen && splitPath ? splitPath : selectedPath,
  )
  const selectedArtifactId = $derived(artifactIdFromScratchPath(activePath))
  const activeFileProblems = $derived(
    activePath ? (problemsByPath.get(activePath) ?? null) : null,
  )

  /** Map LSP DiagnosticSeverity names onto Findings sev-* icons. */
  function problemSevIcon(severity: LspProblem['severity']): IconName {
    switch (severity) {
      case 'error': return findingSevIcon('high')
      case 'warning': return findingSevIcon('medium')
      case 'info': return findingSevIcon('info')
      default: return findingSevIcon('low')
    }
  }

  function problemLocLabel(p: LspProblem): string {
    const name = p.path.includes('/') ? p.path.slice(p.path.lastIndexOf('/') + 1) : p.path
    return p.line > 0 ? `${name}:${p.line}` : (name || '—')
  }

  function problemFlagFor(path: string, isDir: boolean): LspPathProblemFlag | null {
    if (!isDir) return problemsByPath.get(path) ?? null
    const prefix = path + '/'
    let errors = 0
    let warnings = 0
    let count = 0
    let severity: LspPathProblemFlag['severity'] = 'info'
    for (const [p, f] of problemsByPath) {
      if (p !== path && !p.startsWith(prefix)) continue
      errors += f.errors
      warnings += f.warnings
      count += f.count
      if (f.severity === 'error') severity = 'error'
      else if (f.severity === 'warning' && severity !== 'error') severity = 'warning'
    }
    return count ? { errors, warnings, count, severity } : null
  }

  function detachLsp() {
    lspAttachGen++
    lspAttach = null
    editor?.setLspExtension([])
  }

  // CodeEditor now reports edits synchronously but hands over the full text on a
  // short debounce, so `draft`/`splitDraft` can trail the buffer by a frame or
  // two. Anything that must not race a keystroke — refreshOpenFileIfStale in
  // particular, which overwrites the buffer from disk whenever it sees a clean
  // pane — reads these flags. The editor clears the flag when the buffer matches
  // the last clean snapshot again (undo / delete-to-original). The text
  // comparison stays as a backstop: it also covers the case where one file is
  // open in both panes and the mirrored text arrives at the other editor as an
  // external change (clearing its flag) while the file is genuinely still
  // modified. Line endings are compared the way CodeMirror stores them (LF).
  let mainEditorDirty = $state(false)
  let splitEditorDirty = $state(false)

  const splitDirty = $derived(!!splitPath && (splitEditorDirty || !editorTextEq(splitDraft, splitSaved)))
  const activeGitStatus = $derived(activePath ? (gitStatus[activePath] || '') : '')
  // Status bar language = TextMate grammar label (not indent-prefs EditorLangId).
  const editorLang = $derived.by(() => {
    void $grammarCatalogEpoch
    void $syntaxModeEpoch
    return activePath ? languageLabelForPath(activePath) : ''
  })
  const currentSymbol = $derived(symbolAtLine(outline, cursorLine))
  const pathCrumbs = $derived(activePath ? activePath.split('/').filter(Boolean) : [])
  const canShowDiff = $derived(!!selectedGitStatus && !isBinaryImagePath(selectedPath) && !isMediaPath(selectedPath))
  const canShowImageDiff = $derived(
    isBinaryImagePath(selectedPath) && (selectedGitStatus === 'm' || selectedGitStatus === 'd'),
  )
  // Prefer `draft` once this path is hydrated — including '' (user cleared the
  // buffer). `draft || file.content` treated empty as missing and fed the
  // on-disk text back into MergeEditor's live doc sync, undoing delete-all.
  // Wait for editorPath too: collapseUnchanged only inits on first mount.
  const mergeMissingCurrent = $derived(
    selectedGitStatus === 'd' || selectedIsDir || hydrateFailedPath === selectedPath,
  )
  const mergeCurrent = $derived(mergeViewCurrent({
    missingCurrent: mergeMissingCurrent,
    editorPath,
    selectedPath,
    draft,
  }))
  const mergeReady = $derived(mergeViewReady({
    selectedPath,
    baseDocPath,
    canShowDiff,
    missingCurrent: mergeMissingCurrent,
    editorPath,
  }))
  const showDiff = $derived(viewMode === 'diff' && mergeReady)
  /** Diff selected but base or current still in flight — show a load state, not the code view. */
  const diffPending = $derived(
    viewMode === 'diff' && !!selectedPath && !mergeReady && !isBinaryImagePath(selectedPath) && !isMediaPath(selectedPath),
  )
  // A file can stop being diffable while Diff is up (agent reverts it, status
  // refresh clears it) — fall back to Code instead of a blank/loading pane.
  // Do not treat "status not loaded yet" as "no diff": cold restore may put
  // Diff up before GitStatus returns, and flipping now would both hide the
  // chip and persist 'code' over the saved view.
  $effect(() => {
    if (viewMode !== 'diff' || !selectedPath) return
    if (!gitStatusReady) return
    if (isBinaryImagePath(selectedPath) || isMediaPath(selectedPath)) {
      if (!canShowImageDiff) viewMode = 'preview'
      return
    }
    if (!canShowDiff && selectedGitStatus !== 'd') viewMode = 'code'
  })
  // Rasters have no Code surface — map a restored/stale 'code' chip to Preview.
  $effect(() => {
    if (!isBinaryImagePath(selectedPath) && !isMediaPath(selectedPath)) return
    if (viewMode === 'code') viewMode = 'preview'
  })
  // Keep active tabs valid when Settings moves panels between sides.
  $effect(() => {
    const leftKeys = leftPanels.map((p) => p.key)
    const rightKeys = rightPanels.map((p) => p.key)
    if (leftKeys.length && !leftKeys.includes(leftKind)) leftKind = leftKeys[0]
    if (rightKeys.length && !rightKeys.includes(rightKind)) rightKind = rightKeys[0]
  })

  export async function selectCodePanel(next: CodePanelKey) {
    const side = $codePanelSides[next]
    if (mainBlocked && side !== 'left') return
    if (side === 'left') {
      leftKind = next
      setLeftGutterOpen(true)
    } else {
      rightKind = next
      setShowSide(true)
    }
    if (next === 'findings') {
      void loadFindings()
    }
    if (next === 'search') {
      await tick()
      searchInput?.focus()
      searchInput?.select()
    }
  }

  const isImage = $derived(!!file?.mime?.startsWith('image/'))
  const isPdf = $derived(file?.mime === 'application/pdf' || isPdfPath(selectedPath))
  const isVideo = $derived(!!file?.mime?.startsWith('video/'))
  const imageSrc = $derived.by(() => {
    if (!isImage || !file) return ''
    if (file.path.startsWith('scratch://') && file.content) {
      return `data:${file.mime};base64,${file.content}`
    }
    if (isAbsFilesystemPath(file.path)) {
      return externalFileURL(file.path, file.mtime ?? 0)
    }
    return workspaceFileURL(projectId, workspaceId, file.path, file.mtime ?? 0)
  })
  const mediaSrc = $derived.by(() => {
    if ((!isPdf && !isVideo) || !file) return ''
    if (file.path.startsWith('scratch://') && file.content) {
      return `data:${file.mime};base64,${file.content}`
    }
    if (isAbsFilesystemPath(file.path)) {
      return externalFileURL(file.path, file.mtime ?? 0)
    }
    return workspaceFileURL(projectId, workspaceId, file.path, file.mtime ?? 0)
  })
  /** Bumped when the FileAtBase cache mutates; also cache-busts image `at=base` URLs. */
  let baseDocEpoch = $state(0)
  /** Previous revision for a modified/deleted raster — skip added/untracked. */
  const imageBaseSrc = $derived.by(() => {
    if (!selectedPath || isScratchPath(selectedPath) || isAbsFilesystemPath(selectedPath)) return ''
    if (selectedGitStatus !== 'm' && selectedGitStatus !== 'd') return ''
    if (!isImage && !isBinaryImagePath(selectedPath)) return ''
    // `t` folds the base-doc epoch so a rebase/refetch cannot keep a stale
    // previous pane; 0 is omitted by workspaceFileURL.
    return workspaceFileURL(projectId, workspaceId, selectedPath, baseDocEpoch + 1, 'base')
  })
  let imageBaseFailedSrc = $state('')
  const imageCompare = $derived(
    viewMode === 'diff'
    && !!imageBaseSrc
    && imageBaseFailedSrc !== imageBaseSrc
    && (selectedGitStatus === 'd' || (!!file && file.path === selectedPath)),
  )
  const showImagePane = $derived(
    (isImage && !!file && file.path === selectedPath && selectedGitStatus !== 'd')
    || (isBinaryImagePath(selectedPath) && selectedGitStatus === 'd'),
  )
  const showPdfPane = $derived(
    isPdf && !!file && file.path === selectedPath && selectedGitStatus !== 'd',
  )
  const showVideoPane = $derived(
    isVideo && !!file && file.path === selectedPath && selectedGitStatus !== 'd',
  )
  let mediaZoom = $state<{ src: string; name: string; video?: boolean; startTime?: number } | null>(null)
  let paneVideo = $state<HTMLVideoElement>()
  function zoomVideo() {
    if (!paneVideo || !file) return
    paneVideo.pause()
    mediaZoom = { src: mediaSrc, name: file.path, video: true, startTime: paneVideo.currentTime }
  }
  const showSplitLayout = $derived(splitOpen && !showDiff && !showImagePane && !showPdfPane && !showVideoPane)
  const previewKind = $derived<PreviewKind | null>(
    file && !isBinaryPreviewMime(file.mime) ? previewKindForPath(file.path) : null,
  )
  /** Enabled crumb-bar chips — ⌘⇧V cycles this list. */
  const availableViewModes = $derived(editorViewModes({
    hasFile: !!file || ((isBinaryImagePath(selectedPath) || isMediaPath(selectedPath)) && selectedGitStatus !== 'd'),
    deleted: selectedGitStatus === 'd',
    canShowDiff: canShowDiff || canShowImageDiff,
    hasPreview: isBinaryImagePath(selectedPath) || isMediaPath(selectedPath)
      ? selectedGitStatus !== 'd'
      : !!previewKind,
    isImage: isImage || isBinaryImagePath(selectedPath) || isPdf || isVideo || isMediaPath(selectedPath),
  }))
  const showPreview = $derived(!!previewKind && viewMode === 'preview')
  const dirty = $derived(!!file && !isBinaryPreviewMime(file.mime) && (mainEditorDirty || !editorTextEq(draft, savedContent)))
  const activeDirty = $derived(
    focusedPane === 'split' && splitOpen ? splitDirty : dirty,
  )
  /** Paths whose editor buffer is unsaved — same signal as the file-tab dirty dot. */
  const unsavedPaths = $derived.by(() => {
    const s = new Set<string>()
    if (dirty && selectedPath) s.add(selectedPath)
    if (splitDirty && splitOpen && splitPath) s.add(splitPath)
    return s
  })
  // Worktree edit bit from ReadFile — not gated on viewMode (Diff may show the
  // plain editor while base content loads; that must stay editable). Files
  // over MAX_EDIT_BYTES stay view-only so save cannot freeze the Wails bridge.
  const writable = $derived(
    !!file?.writable && !isBinaryPreviewMime(file.mime) && file.size <= MAX_EDIT_BYTES,
  )
  /** Mutating tree ops (new/rename/delete/…) require a loop worktree. */
  const treeWritable = $derived(!!workspaceId)
  /** Recorded worktree folder for the on-screen workspace. Empty during eager
   *  create and after teardown blanks the path — polls must not fall through
   *  to the project checkout. */
  const worktreePath = $derived(
    workspaceId ? ($workspaces.find((w) => w.id === workspaceId)?.worktreePath ?? '') : '',
  )
  /** Source Control + git status polls only apply to git checkouts. Plain-folder
   *  projects keep the SCM panel as a disabled "Not a git repository" state. */
  const isGit = $derived(!!$projects.find((p) => p.id === projectId)?.isGit)
  const mainSurface = $derived(isGit && mainSurfacePref === 'review' ? 'review' : 'editor')
  const reviewBase = $derived.by(() => {
    const ws = workspaceId ? $workspaces.find((w) => w.id === workspaceId) : undefined
    if (!ws || ws.isMain) return ''
    return (ws.baseBranch || $projects.find((p) => p.id === projectId)?.baseBranch || '').trim()
  })
  // Any git checkout can list commits: unique-to-branch when there is a base,
  // recent HEAD history on the main working copy (no merge-base).
  const reviewHasCommits = $derived(isGit)
  const reviewStripTab = $derived(!reviewHasCommits && reviewTab === 'commits' ? 'changes' : reviewTab)
  /** Directory used as the target for toolbar New File / New Folder. */
  const actionDir = $derived.by(() => {
    const path = treePath || selectedPath
    if (!path) return ''
    const entry = findEntry(activeTree, path)
    if (entry?.dir) return path
    return parentPath(path)
  })

  function formatBytes(n: number): string {
    if (n < 1024) return `${n} B`
    if (n < 1024 * 1024) return `${(n / 1024).toFixed(n < 10 * 1024 ? 1 : 0)} KB`
    if (n < 1024 * 1024 * 1024) return `${(n / (1024 * 1024)).toFixed(1)} MB`
    return `${(n / (1024 * 1024 * 1024)).toFixed(2)} GB`
  }

  /** Patch a single subdir's children in-place. Merges (via mergeChildren)
   * rather than replacing wholesale, so if that dir itself has an
   * already-loaded grandchild, a live-refresh cycle doesn't momentarily wipe
   * it before the grandchild's own refresh repopulates it a tick later. */
  function patchChildren(nodes: TreeEntry[], path: string, kids: TreeEntry[]): TreeEntry[] {
    return nodes.map((n) => {
      if (n.path === path) return { ...n, children: mergeChildren(n.children ?? [], kids) }
      if (n.children?.length) return { ...n, children: patchChildren(n.children, path, kids) }
      return n
    })
  }

  function findEntry(nodes: TreeEntry[], path: string): TreeEntry | null {
    for (const n of nodes) {
      if (n.path === path) return n
      if (n.children?.length) {
        const hit = findEntry(n.children, path)
        if (hit) return hit
      }
    }
    return null
  }

  function filesScrollEl(): HTMLElement | null {
    return filesPaneEl ?? null
  }

  async function withFilesScroll<T>(fn: () => Promise<T>): Promise<T> {
    const el = filesScrollEl()
    const top = el?.scrollTop ?? 0
    try {
      return await fn()
    } finally {
      await tick()
      if (el) el.scrollTop = top
    }
  }

  function parentPath(path: string): string {
    const i = path.lastIndexOf('/')
    return i < 0 ? '' : path.slice(0, i)
  }

  function joinPath(parent: string, name: string): string {
    const n = name.trim()
    if (!n || n.includes('/') || n === '.' || n === '..') return ''
    return parent ? `${parent}/${n}` : n
  }

  function uniqueSiblingName(parent: string, base: string, dir: boolean): string {
    const kids = parent === '' ? tree : (findEntry(tree, parent)?.children ?? [])
    const taken = new Set((kids ?? []).map((k) => k.name))
    if (!taken.has(base)) return base
    if (dir) {
      for (let i = 2; i < 200; i++) {
        const n = `${base} ${i}`
        if (!taken.has(n)) return n
      }
      return `${base} copy`
    }
    const dot = base.lastIndexOf('.')
    const stem = dot > 0 ? base.slice(0, dot) : base
    const ext = dot > 0 ? base.slice(dot) : ''
    for (let i = 2; i < 200; i++) {
      const n = `${stem} ${i}${ext}`
      if (!taken.has(n)) return n
    }
    return `${stem} copy${ext}`
  }

  function closeCtx() {
    ctxMenu = null
  }

  function openCtx(e: MouseEvent, path: string, dir: boolean, name: string, tab = false) {
    e.preventDefault()
    e.stopPropagation()
    const pad = 8
    const mw = 220
    const mh = tab ? 110 : 360
    let x = e.clientX
    let y = e.clientY
    if (x + mw > window.innerWidth - pad) x = window.innerWidth - mw - pad
    if (y + mh > window.innerHeight - pad) y = window.innerHeight - mh - pad
    ctxMenu = { x: Math.max(pad, x), y: Math.max(pad, y), path, dir, name, tab }
  }

  async function softRefresh(parents: string[]) {
    await withFilesScroll(async () => {
      const uniq = [...new Set(parents.map((p) => p || ''))]
      for (const p of uniq) {
        if (p === '') {
          const t = await call(CodeService.ListTree(projectId, workspaceId, ''), 'Tree failed')
          if (t) tree = mergeChildren(tree, t)
          continue
        }
        if (!expanded.has(p) && !loadedDirs.has(p)) continue
        await ensureDirLoaded(p, true)
      }
      await refreshGitStatus()
      if (workspaceId) {
        const ch = await call(CodeService.ListChangedFiles(workspaceId), 'Changed files failed')
        if (ch) changedPaths = ch
      }
    })
  }

  function remapKeyedRecord<T>(rec: Record<string, T>, from: string, to: string): Record<string, T> {
    const wasFrom = (p: string) => p === from || p.startsWith(from + '/')
    const rewrite = (p: string) => (p === from ? to : to + p.slice(from.length))
    const claimed = new Set(Object.keys(rec).filter(wasFrom).map(rewrite))
    const next: Record<string, T> = {}
    for (const [p, v] of Object.entries(rec)) {
      if (wasFrom(p) || claimed.has(p)) continue
      next[p] = v
    }
    for (const [p, v] of Object.entries(rec)) {
      if (!wasFrom(p)) continue
      next[rewrite(p)] = v
    }
    return next
  }

  /** True when remap would drop an unsaved dest tab (deleted file still open). */
  function remapWouldDiscardUnsaved(from: string, to: string): boolean {
    if (!from || !to || from === to) return false
    const destHit = (p: string) => p === to || p.startsWith(to + '/')
    const fromHit = (p: string) => p === from || p.startsWith(from + '/')
    for (const p of Object.keys(tabBuffers)) {
      if (destHit(p) && !fromHit(p) && tabIsDirty(p)) return true
    }
    for (const p of Object.keys(stashedBuffers)) {
      if (destHit(p) && !fromHit(p) && tabIsDirty(p)) return true
    }
    for (const p of Object.keys(dirtyTabs)) {
      if (destHit(p) && !fromHit(p) && tabIsDirty(p)) return true
    }
    return false
  }

  async function confirmDiscardRemapDest(from: string, to: string): Promise<boolean> {
    if (!remapWouldDiscardUnsaved(from, to)) return true
    return askConfirm('Unsaved edits', 'Discard unsaved edits?', 'Discard')
  }

  function remapOpenPath(from: string, to: string) {
    // Capture live undo stacks first: remap only moves what's already in the
    // serialized cache, and later captures still key off the old mountedPath.
    editor?.captureHistory()
    splitEditor?.captureHistory()
    mergeEditor?.captureHistory()
    remapEditorHistory(editorSessionKey, from, to)
    openFiles = remapOpenFiles(openFiles, from, to)
    tabBuffers = remapKeyedRecord(tabBuffers, from, to)
    dirtyTabs = remapKeyedRecord(dirtyTabs, from, to)
    fileViews = remapKeyedRecord(fileViews, from, to)
    stashedBuffers = remapKeyedRecord(stashedBuffers, from, to)
    if (selectedPath === from || selectedPath.startsWith(from + '/')) {
      selectedPath = selectedPath === from ? to : to + selectedPath.slice(from.length)
    }
    if (treePath === from || treePath.startsWith(from + '/')) {
      treePath = treePath === from ? to : to + treePath.slice(from.length)
    }
    if (editorPath === from || editorPath.startsWith(from + '/')) {
      editorPath = editorPath === from ? to : to + editorPath.slice(from.length)
    }
    if (splitPath === from || splitPath.startsWith(from + '/')) {
      splitPath = splitPath === from ? to : to + splitPath.slice(from.length)
    }
    if (file && (file.path === from || file.path.startsWith(from + '/'))) {
      const next = file.path === from ? to : to + file.path.slice(from.length)
      file = { ...file, path: next }
    }
    if (baseDocPath === from || baseDocPath.startsWith(from + '/')) {
      baseDocPath = baseDocPath === from ? to : to + baseDocPath.slice(from.length)
    }
    const nextExp = new Set<string>()
    for (const d of expanded) {
      if (d === from) nextExp.add(to)
      else if (d.startsWith(from + '/')) nextExp.add(to + d.slice(from.length))
      else nextExp.add(d)
    }
    expanded = nextExp
    const nextLoaded = new Set<string>()
    for (const d of loadedDirs) {
      if (d === from) nextLoaded.add(to)
      else if (d.startsWith(from + '/')) nextLoaded.add(to + d.slice(from.length))
      else nextLoaded.add(d)
    }
    loadedDirs = nextLoaded
  }

  function dropOpenPath(path: string) {
    openFiles = openFiles.filter((p) => p !== path && !p.startsWith(path + '/'))
    const nextStash = { ...stashedBuffers }
    let stashChanged = false
    for (const p of Object.keys(nextStash)) {
      if (p === path || p.startsWith(path + '/')) {
        delete nextStash[p]
        stashChanged = true
      }
    }
    if (stashChanged) stashedBuffers = nextStash
    if (treePath === path || treePath.startsWith(path + '/')) {
      treePath = ''
    }
    if (editorPath === path || editorPath.startsWith(path + '/')) editorPath = ''
    if (selectedPath === path || selectedPath.startsWith(path + '/')) {
      file = null
      selectedIsDir = false
      baseDoc = ''
      baseDocPath = ''
      selectedPath = ''
      draft = ''
      savedContent = ''
      const next = openFiles[0]
      if (next) void openPath(next)
      else detachLsp()
    }
    if (!openFiles.length) persistOrClearEditorSession()
    // After the flush, so an editor unmount's capture can't resurrect a
    // deleted file's undo stack.
    const scope = editorSessionKey
    void tick().then(() => forgetEditorHistoryTree(scope, path))
  }

  async function startNew(kind: 'file' | 'folder', parent: string) {
    closeCtx()
    if (!treeWritable) {
      pushToast('info', 'Create files in a loop worktree')
      return
    }
    if (parent) await toggleDir(parent, true)
    draftEdit = {
      mode: kind === 'file' ? 'new-file' : 'new-folder',
      parent,
      path: '',
      name: kind === 'file' ? 'untitled' : 'new-folder',
      dir: kind === 'folder',
    }
    await tick()
    draftInput?.focus()
    draftInput?.select()
  }

  async function startRename(path: string, name: string, dir = false) {
    closeCtx()
    if (!treeWritable || !path) return
    draftEdit = { mode: 'rename', parent: parentPath(path), path, name, dir }
    await tick()
    draftInput?.focus()
    draftInput?.select()
  }

  async function commitDraft() {
    const ed = draftEdit
    if (!ed) return
    const name = ed.name.trim()
    draftEdit = null
    if (!name || name.includes('/') || name === '.' || name === '..') {
      pushToast('info', 'Invalid name')
      return
    }
    if (ed.mode === 'rename') {
      const to = joinPath(ed.parent, name)
      if (!to || to === ed.path) return
      if (!(await confirmDiscardRemapDest(ed.path, to))) return
      if (!(await callOk(CodeService.Rename(projectId, workspaceId, ed.path, to), 'Rename failed'))) return
      remapOpenPath(ed.path, to)
      await softRefresh([ed.parent, parentPath(to)])
      pushToast('success', 'Renamed')
      return
    }
    const dest = joinPath(ed.parent, name)
    if (!dest) {
      pushToast('info', 'Invalid name')
      return
    }
    const ok = ed.mode === 'new-file'
      ? await callOk(CodeService.CreateFile(projectId, workspaceId, dest), 'Create failed')
      : await callOk(CodeService.Mkdir(projectId, workspaceId, dest), 'Create folder failed')
    if (!ok) return
    await softRefresh([ed.parent])
    if (ed.parent) await toggleDir(ed.parent, true)
    if (ed.mode === 'new-file') void openPath(dest)
    else await toggleDir(dest, true)
  }

  function cancelDraft() {
    draftEdit = null
  }

  /** Native confirm — window.confirm is a no-op in WKWebView (Wails/macOS). */
  async function askConfirm(title: string, message: string, confirmLabel = 'OK'): Promise<boolean> {
    try {
      const choice = await Dialogs.Question({
        Title: title,
        Message: message,
        Buttons: [
          { Label: 'Cancel', IsCancel: true },
          { Label: confirmLabel, IsDefault: true },
        ],
      })
      return choice === confirmLabel
    } catch {
      // Dialog unavailable (browser e2e) — treat as cancelled.
      return false
    }
  }

  async function deletePath(path: string, name: string) {
    closeCtx()
    if (!treeWritable || !path) return
    if (!(await askConfirm('Delete', `Delete “${name}”? This cannot be undone.`, 'Delete'))) return
    if (!(await callOk(CodeService.Delete(projectId, workspaceId, path), 'Delete failed'))) return
    dropOpenPath(path)
    await softRefresh([parentPath(path)])
    pushToast('success', 'Deleted')
  }

  function clipCopy(path: string, dir: boolean) {
    closeCtx()
    if (!path) return
    fileClip = { mode: 'copy', path, dir }
    pushToast('info', 'Copied')
  }

  function clipCut(path: string, dir: boolean) {
    closeCtx()
    if (!treeWritable || !path) return
    fileClip = { mode: 'cut', path, dir }
    pushToast('info', 'Cut')
  }

  async function movePath(src: string, dest: string): Promise<boolean> {
    if (!(await confirmDiscardRemapDest(src, dest))) return false
    if (!(await callOk(CodeService.Rename(projectId, workspaceId, src, dest), 'Move failed'))) return false
    remapOpenPath(src, dest)
    await softRefresh([parentPath(src), parentPath(dest)])
    pushToast('success', 'Moved')
    return true
  }

  async function clipPaste(intoDir: string) {
    closeCtx()
    if (!treeWritable || !fileClip) return
    if (intoDir) await ensureDirLoaded(intoDir, true)
    else await softRefresh([''])
    const src = fileClip.path
    const base = src.split('/').pop() || 'copy'
    const destName = uniqueSiblingName(intoDir, base, fileClip.dir)
    const dest = joinPath(intoDir, destName)
    if (!dest) return
    if (fileClip.mode === 'cut') {
      if (dest === src) return
      if (await movePath(src, dest)) fileClip = null
      return
    }
    if (!(await callOk(CodeService.Copy(projectId, workspaceId, src, dest), 'Paste failed'))) return
    await softRefresh([intoDir])
    if (intoDir) await toggleDir(intoDir, true)
    pushToast('success', 'Pasted')
  }

  async function duplicatePath(path: string, dir: boolean) {
    closeCtx()
    if (!treeWritable || !path) return
    const parent = parentPath(path)
    if (parent) await ensureDirLoaded(parent, true)
    else await softRefresh([''])
    const base = path.split('/').pop() || 'copy'
    const destName = uniqueSiblingName(parent, base, dir)
    const dest = joinPath(parent, destName)
    if (!dest) return
    if (!(await callOk(CodeService.Copy(projectId, workspaceId, path, dest), 'Duplicate failed'))) return
    await softRefresh([parent])
    pushToast('success', 'Duplicated')
  }

  async function copyRelPath(path: string) {
    closeCtx()
    if (!path) return
    try {
      await copyText(path)
      pushToast('info', 'Copied path')
    } catch (err: any) {
      pushToast('error', `Copy failed: ${err?.message ?? String(err)}`)
    }
  }

  async function copyAbsPath(path: string) {
    closeCtx()
    if (!path) return
    const abs = await call(CodeService.AbsPath(projectId, workspaceId, path), 'Path failed')
    if (!abs) return
    try {
      await copyText(abs)
      pushToast('info', 'Copied absolute path')
    } catch (err: any) {
      pushToast('error', `Copy failed: ${err?.message ?? String(err)}`)
    }
  }

  async function revealPath(path: string) {
    closeCtx()
    if (!path) return
    await callOk(CodeService.RevealInFinder(projectId, workspaceId, path), 'Reveal failed')
  }

  async function openExternal(path: string) {
    closeCtx()
    if (!path) return
    await callOk(CodeService.OpenInEditor(projectId, workspaceId, path, 0), 'Open failed')
  }

  function isInChangedScope(path: string, dir: boolean): boolean {
    if (scopeSet.has(path)) return true
    return dir && scopeDirSet.has(path)
  }

  function visibleEntries(entries: TreeEntry[] | undefined | null, changedOnly = false): TreeEntry[] {
    const matchers = excludeMatchers
    return (entries ?? []).filter((e) => {
      if (isExcludedBy(e.path, matchers)) return false
      if (changedOnly && !isInChangedScope(e.path, e.dir)) return false
      return true
    })
  }

  const changedTree = $derived.by(() => {
    if (!filterChanged) return []
    return buildTreeFromPaths(reviewPaths)
  })

  /**
   * Changed scope starts fully expanded (the point is to see the files) and
   * collapsing is local state — no directory listing is involved either way.
   */
  let changedCollapsed = $state<Set<string>>(new Set())

  const visibleTree = $derived(filterChanged ? changedTree : visibleEntries(tree, false))

  /**
   * The side panels render straight into a scroller with no windowing, so a big
   * outline, a wide blast radius or a 500-file working tree used to emit
   * thousands of nodes at once. Cap the rows and let the user ask for the rest,
   * the same way DiffFile handles long diffs.
   */
  const LIST_CAP = 300
  let expandedLists = $state<Set<string>>(new Set())
  function capped<T>(key: string, items: T[]): T[] {
    if (items.length <= LIST_CAP || expandedLists.has(key)) return items
    return items.slice(0, LIST_CAP)
  }
  function expandList(key: string) {
    expandedLists = new Set([...expandedLists, key])
  }

  const statusTitle: Record<string, string> = {
    m: 'Modified',
    a: 'Added',
    d: 'Deleted',
    u: 'Untracked',
  }

  /** Prefer live GitStatus map; fall back to ListTree annotation; dirs inherit dirty child color. */
  function entryStatus(e: TreeEntry): string {
    const direct = gitStatus[e.path] || e.status || ''
    if (direct) return direct
    if (!e.dir) return ''
    return dirStatusIndex.get(e.path) ?? ''
  }

  /** Drop empty entries so an all-blank map doesn't read as "has changes". */
  function compactStatus(raw: Record<string, string | undefined>): Record<string, string> {
    const out: Record<string, string> = {}
    for (const [p, st] of Object.entries(raw)) {
      if (st) out[p] = st
    }
    return out
  }

  let gitStatusGen = 0

  async function refreshGitStatus() {
    if (!projectId && !workspaceId) return
    const gen = ++gitStatusGen
    // Eager create / teardown / mid-switch: no worktree on disk. refreshTree
    // already no-ops here; don't fall through to the project checkout.
    // Bump workingStatusGen so an in-flight refreshWorkingStatus cannot
    // repopulate working/staged after this clear (same workspace id, path gone).
    if ((workspaceId && !worktreePath) || !isGit) {
      workingStatusGen += 1
      gitStatus = {}
      workingStatus = {}
      stagedFiles = []
      return
    }
    const st = await call(CodeService.GitStatus(projectId, workspaceId), 'Git status failed')
    if (gen !== gitStatusGen) return
    if (!st) return
    const next: Record<string, string> = {}
    for (const [p, s] of Object.entries(st)) {
      if (s) next[p] = s
    }
    // Avoid reactive churn (and Changed-filter refilters) when nothing changed.
    const prev = gitStatus
    const nextKeys = Object.keys(next)
    if (
      nextKeys.length === Object.keys(prev).length
      && nextKeys.every((k) => prev[k] === next[k])
    ) {
      // still refresh working/staged
    } else {
      gitStatus = next
      maybeHydrateRestoredCurrent(prev, next)
    }
    await refreshWorkingStatus()
  }

  function maybeHydrateRestoredCurrent(prev: Record<string, string>, next: Record<string, string>) {
    const path = selectedPath
    if (!path) return
    if (!shouldHydrateRestoredCurrent({
      selectedPath: path,
      editorPath,
      selectedIsDir,
      fileLoading,
      hydrateFailedPath,
      prevStatus: prev[path] || '',
      nextStatus: next[path] || '',
    })) return
    void hydrateFile(path)
  }

  let workingStatusGen = 0

  async function refreshWorkingStatus() {
    if (!treeWritable || !isGit || !worktreePath) {
      workingStatusGen += 1
      workingStatus = {}
      stagedFiles = []
      return
    }
    const gen = ++workingStatusGen
    // StagedFiles is the poll that used to 422 mid-switch; quiet it like
    // refreshTree. WorkingStatus only fails on a genuine resolve break.
    const [st, staged] = await Promise.all([
      call(CodeService.WorkingStatus(projectId, workspaceId), 'Working status failed'),
      quiet(CodeService.StagedFiles(projectId, workspaceId)),
    ])
    if (gen !== workingStatusGen) return
    if (st) {
      const next: Record<string, string> = {}
      for (const [p, s] of Object.entries(st)) {
        if (s) next[p] = s
      }
      workingStatus = next
    }
    if (staged) stagedFiles = staged
  }

  function trimOpenFiles(paths: string[], keep: string, max: number): string[] {
    const next = uniqueOpenFiles(paths)
    if (max <= 0) return next
    while (next.length > max) {
      const drop = next.findIndex((p) => p !== keep)
      if (drop < 0) break
      const dropped = next[drop]
      next.splice(drop, 1)
      if (dropped) forgetEditorHistoryTree(editorSessionKey, dropped)
    }
    return next
  }

  /** Add path to the strip if new; never reorder existing tabs. */
  function ensureOpenTab(path: string) {
    if (openFiles.includes(path)) return
    const i = openFiles.indexOf(selectedPath)
    const next = i >= 0
      ? [...openFiles.slice(0, i + 1), path, ...openFiles.slice(i + 1)]
      : [...openFiles, path]
    openFiles = trimOpenFiles(next, path, $editorPrefs.maxOpenFiles)
  }

  // If the user lowers the max, trim excess tabs (keep the active one).
  $effect(() => {
    const max = $editorPrefs.maxOpenFiles
    if (max > 0 && openFiles.length > max) {
      openFiles = trimOpenFiles(openFiles, selectedPath, max)
    }
  })

  $effect(() => {
    const keep = splitPath ? [...openFiles, splitPath] : openFiles
    const live = new Set(keep)
    let dropped = false
    const next = { ...tabBuffers }
    for (const p of Object.keys(next)) {
      if (live.has(p)) continue
      delete next[p]
      dropped = true
    }
    if (dropped) tabBuffers = next
    let dirtyDropped = false
    const nextDirty = { ...dirtyTabs }
    for (const p of Object.keys(nextDirty)) {
      if (live.has(p)) continue
      delete nextDirty[p]
      dirtyDropped = true
    }
    if (dirtyDropped) dirtyTabs = nextDirty
    editor?.retainPaths(keep)
    splitEditor?.retainPaths(keep)
  })

  /** Explorer scope: changed-only vs full tree. Switching to the full tree can
   * land on a view that's only ever been populated at mount (the loop may have
   * stopped emitting feed events by now — converged, paused, awaiting review —
   * so scheduleLiveRefresh's ticks have long since stopped firing), so force a
   * one-off refresh of whatever's on screen here instead of waiting on the
   * next live-refresh tick that may never come. */
  async function setBrowse(all: boolean) {
    browseAll = all
    scopeChosen = true
    saveExplorerScope(projectId, workspaceId, all)
    void selectCodePanel('files')
    if (all) {
      // Switching to the real tree: pull in the directories the Changed view was
      // showing so the user lands where they were rather than back at the root.
      // One batched call, and only when they actually ask for it.
      await expandChangedAncestors()
      void refreshLoadedDirs()
      void refreshChangedFiles()
      void refreshOpenFileIfStale()
    }
    // Changed scope needs nothing loaded — its tree is derived from paths we
    // already hold (see buildTreeFromPaths).
  }

  /**
   * The tree the explorer is actually showing. Changed scope renders a structure
   * derived from changedPaths rather than the lazily-loaded `tree`, so anything
   * resolving the row the user selected has to look here — `tree` may have no
   * entry for that path at all now that entering Changed scope loads nothing.
   * (Filesystem questions — sibling name collisions — still go to `tree`, whose
   * callers force-load the directory first.)
   */
  const activeTree = $derived(filterChanged ? changedTree : tree)

  /** Every directory on the way to one of `paths`, shallowest first. */
  function ancestorDirsOf(paths: Iterable<string>): string[] {
    const dirs = new Set<string>()
    for (const p of paths) {
      for (let i = p.indexOf('/'); i >= 0; i = p.indexOf('/', i + 1)) {
        dirs.add(p.slice(0, i))
      }
    }
    return [...dirs].sort((a, b) => a.split('/').length - b.split('/').length)
  }

  /**
   * Reveal every changed file by loading and expanding its ancestor directories.
   *
   * This is the whole cost of entering the Changed scope, and it used to await
   * one ListTree per ancestor — each of which recomputed the repo's git status
   * (three subprocesses) before returning a single directory. Fifteen changed
   * files across fifteen directories meant 21 serial round trips to draw 35
   * rows. One ListTrees covers the lot, and the tree/expanded/loadedDirs sets
   * are each assigned once instead of once per directory (every reassignment
   * deep-clones the tree and re-runs the filtered render).
   */
  async function expandChangedAncestors() {
    const dirs = ancestorDirsOf(scopePaths)
    if (!dirs.length) return
    const missing = dirs.filter((d) => !loadedDirs.has(d))
    if (missing.length) {
      let batch: Record<string, TreeEntry[] | undefined>
      try {
        batch = await CodeService.ListTrees(projectId, workspaceId, missing)
      } catch {
        // Leave the tree alone and let the next refresh retry rather than
        // half-expanding into a misleading view.
        return
      }
      let next = tree
      const loaded = new Set(loadedDirs)
      // `missing` is shallowest-first, so a parent's children are in place
      // before a child gets patched into them.
      for (const d of missing) {
        const kids = batch?.[d]
        if (!kids) continue
        next = d === '' ? mergeChildren(next, kids) : patchChildren(next, d, kids)
        loaded.add(d)
      }
      tree = next
      loadedDirs = loaded
    }
    expanded = new Set([...expanded, ...dirs])
  }

  function beginTreeLoad() {
    treeLoading = true
    // Keep any already-armed timer: re-arming would push the skeleton further
    // out on every call and a slow load would never show one.
    if (skeletonTimer) return
    skeletonTimer = setTimeout(() => {
      skeletonTimer = undefined
      if (treeLoading) showTreeSkeleton = true
    }, SKELETON_DELAY_MS)
  }

  // Dependency-free, so it runs once on mount and its cleanup runs on destroy —
  // a pending skeleton timer must not outlive the component.
  $effect(() => () => {
    if (skeletonTimer) clearTimeout(skeletonTimer)
    skeletonTimer = undefined
    if (fileLoadingTimer) clearTimeout(fileLoadingTimer)
    fileLoadingTimer = undefined
    if (fileLoadingHideTimer) clearTimeout(fileLoadingHideTimer)
    fileLoadingHideTimer = undefined
  })

  function endTreeLoad() {
    treeLoading = false
    showTreeSkeleton = false
    if (skeletonTimer) {
      clearTimeout(skeletonTimer)
      skeletonTimer = undefined
    }
  }

  /**
   * Single writer for the file-load indicator: `fileLoading` stays the honest
   * state (other logic branches on it), `showFileLoading` is the debounced
   * display of it.
   */
  function setFileLoading(next: boolean) {
    fileLoading = next
    if (next) {
      // A new load supersedes a pending hide — keep the indicator up.
      if (fileLoadingHideTimer) {
        clearTimeout(fileLoadingHideTimer)
        fileLoadingHideTimer = undefined
      }
      if (fileLoadingTimer || showFileLoading) return
      fileLoadingTimer = setTimeout(() => {
        fileLoadingTimer = undefined
        if (!fileLoading) return
        showFileLoading = true
        fileLoadingShownAt = Date.now()
      }, SKELETON_DELAY_MS)
      return
    }
    if (fileLoadingTimer) {
      clearTimeout(fileLoadingTimer)
      fileLoadingTimer = undefined
    }
    if (!showFileLoading || fileLoadingHideTimer) return
    const shownFor = Date.now() - fileLoadingShownAt
    if (shownFor >= FILE_LOAD_MIN_VISIBLE_MS) {
      showFileLoading = false
      return
    }
    fileLoadingHideTimer = setTimeout(() => {
      fileLoadingHideTimer = undefined
      showFileLoading = false
    }, FILE_LOAD_MIN_VISIBLE_MS - shownFor)
  }

  /** Everything scoped to one project/worktree, cleared together. */
  function resetExplorerData() {
    beginTreeLoad()
    tree = []
    expanded = new Set()
    loadedDirs = new Set()
    loadingDirs = new Set()
    dirNotFoundSince.clear()
    maybeClearGoneConfirm()
    changedPaths = []
    gitStatus = {}
    gitStatusReady = false
    workingStatus = {}
    stagedFiles = []
    worktreeMissing = false
    outline = []
    hits = []
    refs = []
    searchSkippedLarge = 0
    searchTruncated = false
    findings = []
    expandedLists = new Set()
    setFileLoading(false)
    fileLoadAbort?.abort()
    hydrateFailedPath = ''
  }

  /**
   * Load everything the explorer needs for the current root.
   *
   * Tree listing + git-status + working/staged go out together. ListTree still
   * annotates with vs-base status (deleted files missing on disk), and GitStatus
   * supplies the full map for the Changed view — Go coalesces concurrent
   * ChangeStatus/StatusMap so this is one git pass, not three. Changed paths are
   * the GitStatus keys (ListChangedFiles is the same set plus a PR snapshot used
   * after teardown, which this path never hits: no worktree → we return above).
   *
   * They are also applied in a single batch. `scopePaths` is the union of
   * `changedPaths` and `gitStatus`, so letting those land separately briefly
   * rendered one worktree's files alongside another's.
   */
  /**
   * @param opts.soft When true, keep the current tree visible (no skeleton) and
   *   merge the fresh root listing without collapsing expanded dirs — used for
   *   stale-while-revalidate after a keep-alive restore.
   */
  async function refreshTree(opts: { soft?: boolean } = {}) {
    if (!projectId && !workspaceId) {
      endTreeLoad()
      return
    }
    if (workspaceId && !worktreePath) {
      skippedTreeForCreate = true
      if (!opts.soft) endTreeLoad()
      return
    }
    skippedTreeForCreate = false
    const token = rootToken
    const soft = !!opts.soft
    if (!soft) beginTreeLoad()
    try {
      const treeP = CodeService.ListTree(projectId, workspaceId, '').then(
        (rows) => ({ rows, missing: false, err: null as unknown }),
        (err: unknown) => ({ rows: null as TreeEntry[] | null, missing: isWorktreeMissing(err), err }),
      )
      const [treeRes, st, ws, staged] = await Promise.all([
        treeP,
        isGit ? call(CodeService.GitStatus(projectId, workspaceId), 'Git status failed') : Promise.resolve({}),
        treeWritable && isGit ? quiet(CodeService.WorkingStatus(projectId, workspaceId)) : Promise.resolve(null),
        treeWritable && isGit ? quiet(CodeService.StagedFiles(projectId, workspaceId)) : Promise.resolve(null),
      ])
      // The user moved to another project/worktree while this was in flight —
      // applying it now would repopulate the explorer with the wrong repo.
      if (token !== rootToken) return
      if (treeRes.missing) worktreeMissing = true
      else if (treeRes.err) pushToast('error', `Tree failed: ${bridgeErrorMessage(treeRes.err)}`)
      else worktreeMissing = false
      const t = treeRes.rows
      if (t) {
        if (soft) {
          // Keep expand/load state; merge root children so already-fetched
          // subtrees aren't wiped for a background revalidate.
          tree = mergeChildren(tree, t)
          loadedDirs = new Set([...loadedDirs, ''])
        } else {
          tree = t
          expanded = new Set()
          loadedDirs = new Set([''])
          // loadedDirs is being rebuilt from scratch, so any leftover missing
          // timestamps (from dirs no longer tracked) must not carry over —
          // otherwise a later not-found could combine with a stale first-miss
          // and evict a dir that just came back.
          dirNotFoundSince.clear()
          maybeClearGoneConfirm()
        }
      }
      if (st) {
        const prev = gitStatus
        gitStatus = compactStatus(st)
        changedPaths = Object.keys(gitStatus)
        maybeHydrateRestoredCurrent(prev, gitStatus)
      }
      if (ws) workingStatus = compactStatus(ws)
      if (staged) stagedFiles = staged
      // Soft restore: refresh any dirs the user already had open so children
      // match disk without a full collapse.
      if (soft && loadedDirs.size > 1) void refreshLoadedDirs()
    } finally {
      if (token === rootToken) {
        gitStatusReady = true
        if (!soft) endTreeLoad()
      }
    }
  }

  // Eager create leaves WorktreePath empty until git finishes. refreshTree
  // no-ops until then; load the explorer once the path lands on this workspace.
  $effect(() => {
    if (!workspaceId || !worktreePath || !skippedTreeForCreate) return
    skippedTreeForCreate = false
    void refreshTree()
  })

  /**
   * Base-revision cache: FileAtBase is stable for a worktree's lifetime
   * except when the merge-base moves (Update with base / retarget). Then
   * `invalidateBaseDocs` drops the cache so gutter and Diff refetch.
   * Cached entries are shown immediately and revalidated in the background —
   * a moved base swaps in place (MergeEditor holds the live scroll across
   * that rebuild). Cleared on workspace switch.
   */
  const baseDocCache = new Map<string, string>()
  const BASE_DOC_CACHE_MAX = 32
  const baseDocInflight = new Map<string, Promise<string | null>>()
  // baseDocEpoch is declared with the image-pane URL (cache-busts `at=base`).

  function rememberBaseDoc(path: string, content: string) {
    if (content.length > EDITOR_BODY_CAP) return
    const prev = baseDocCache.get(path)
    baseDocCache.delete(path)
    baseDocCache.set(path, content)
    let evicted = false
    while (baseDocCache.size > BASE_DOC_CACHE_MAX) {
      const oldest = baseDocCache.keys().next().value
      if (oldest === undefined) break
      baseDocCache.delete(oldest)
      evicted = true
    }
    if (prev !== content || evicted) baseDocEpoch++
  }

  function gutterBaseFor(path: string): string | null {
    void baseDocEpoch
    if (!path || !isGit || isScratchPath(path) || isAbsFilesystemPath(path)) return null
    if (!baseDocCache.has(path)) return null
    return scmGutterBase(baseDocCache.get(path)!, gitStatus[path] || '')
  }

  function fetchBaseDoc(path: string): Promise<string | null> {
    const cached = baseDocCache.get(path)
    if (cached != null) return Promise.resolve(cached)
    const pending = baseDocInflight.get(path)
    if (pending) return pending
    const token = rootToken
    const req = quiet(CodeService.FileAtBase(projectId, workspaceId, path)).then((b) => {
      if (baseDocInflight.get(path) === req) baseDocInflight.delete(path)
      if (b == null || rootToken !== token) return null
      rememberBaseDoc(path, b)
      return b
    })
    baseDocInflight.set(path, req)
    return req
  }

  /** Warm FileAtBase for the code-view gutter without blocking paint or toasting. */
  function prefetchBaseDoc(path: string) {
    if (!isGit || !path || isScratchPath(path) || isAbsFilesystemPath(path) || isBinaryImagePath(path) || isMediaPath(path)) return
    void fetchBaseDoc(path)
  }

  async function loadBaseDoc(path: string) {
    if (isBinaryImagePath(path) || isMediaPath(path)) return
    const cached = baseDocCache.get(path)
    if (cached != null) {
      baseDoc = cached
      baseDocPath = path
      const token = rootToken
      void quiet(CodeService.FileAtBase(projectId, workspaceId, path)).then((b) => {
        if (b == null || rootToken !== token) return
        rememberBaseDoc(path, b)
        if (path === selectedPath && baseDocPath === path && b !== baseDoc) baseDoc = b
      })
      return
    }
    const pending = baseDocInflight.get(path)
    if (pending) {
      const b = await pending
      if (path !== selectedPath) return
      if (b != null) {
        baseDoc = b
        baseDocPath = path
        return
      }
    }
    const token = rootToken
    const b = await call(CodeService.FileAtBase(projectId, workspaceId, path), 'Base revision failed')
    if (rootToken !== token) return
    if (b != null) rememberBaseDoc(path, b)
    if (path !== selectedPath) return
    baseDoc = b ?? ''
    baseDocPath = path
  }

  function invalidateBaseDocs() {
    if (baseDocCache.size === 0 && baseDocInflight.size === 0 && !baseDocPath) return
    baseDocCache.clear()
    baseDocInflight.clear()
    baseDocEpoch++
    baseDoc = ''
    baseDocPath = ''
    if (viewMode === 'diff' && selectedPath) void loadBaseDoc(selectedPath)
    else if (selectedPath) prefetchBaseDoc(selectedPath)
    if (splitPath && splitPath !== selectedPath) prefetchBaseDoc(splitPath)
  }

  let trackedBaseWs = ''
  let trackedBaseBranch = ''
  let trackedBehindBase = 0
  $effect(() => {
    const id = workspaceId
    const w = $workspaces.find((x) => x.id === id)
    const branch = (w?.baseBranch || '').trim()
    const behind = w?.behindBase ?? 0
    if (id !== trackedBaseWs) {
      trackedBaseWs = id
      trackedBaseBranch = branch
      trackedBehindBase = behind
      return
    }
    const retargeted = branch !== trackedBaseBranch
    const caughtUp = behind < trackedBehindBase
    trackedBaseBranch = branch
    trackedBehindBase = behind
    if (retargeted || caughtUp) invalidateBaseDocs()
  })

  async function ensureDirLoaded(path: string, force = false) {
    if (!force && (loadedDirs.has(path) || loadingDirs.has(path))) return
    if (!force && loadingDirs.has(path)) return
    // Quiet refresh: keep current children visible — no spinner / collapse flicker.
    const quiet = force && loadedDirs.has(path)
    if (!quiet) loadingDirs = new Set([...loadingDirs, path])
    try {
      const kids = await call(CodeService.ListTree(projectId, workspaceId, path), 'Tree failed')
      if (kids) {
        tree = patchChildren(tree, path, kids)
        loadedDirs = new Set([...loadedDirs, path])
        // Freshly (re-)loaded, so a leftover first-miss timestamp from before
        // this dir was dropped from loadedDirs no longer applies.
        dirNotFoundSince.delete(path)
      }
    } finally {
      if (!quiet) {
        const next = new Set(loadingDirs)
        next.delete(path)
        loadingDirs = next
      }
    }
  }

  /** Silence toasts for background live-refresh (see scheduleLiveRefresh
   * below): a rejection that repeats every cycle (e.g. a dir/file the agent
   * just deleted) must not spam an error toast on each tick. */
  async function quiet<T>(p: Promise<T>): Promise<T | null> {
    try { return await p } catch { return null }
  }

  /** The backend marks a genuine "this path doesn't exist" rejection with a
   * stable "not found: …" prefix (internal/code.ErrNotFound), distinct from a
   * transient failure (a momentary I/O error, a race with the engine
   * rewriting the worktree, a resolveRoot error). Live refresh is event-driven
   * (FSEvents → short debounce), so treating every rejection as "gone" would
   * spontaneously close open tabs / drop tree dirs on a single blip. */
  function isNotFound(err: unknown): boolean {
    const msg = (err as { message?: unknown } | null | undefined)?.message
    return typeof msg === 'string' && msg.includes('not found:')
  }

  /** Merge a freshly-listed dir's entries into the existing tree, carrying
   * over any children already fetched for a subdir so it doesn't render
   * momentarily empty. Used both for a full listing (the root) and, via
   * patchChildren, for a single subdir. */
  function mergeChildren(existing: TreeEntry[], fresh: TreeEntry[]): TreeEntry[] {
    const byPath = new Map(existing.map((e) => [e.path, e]))
    return fresh.map((e) => {
      const prev = byPath.get(e.path)
      return prev?.children ? { ...e, children: prev.children } : e
    })
  }

  /** Debounce for coalescing same-tick FSEvents into one RPC. Kernel NoDefer
   * already delivers idle creates immediately; bursts coalesce at 400ms. */
  const LIVE_REFRESH_MS = 50
  /** Working-tree diffs skip chroma but still marshal every file; don't rebuild
   * the review stack on every 50ms FS coalescing tick while a loop is writing. */
  const REVIEW_REFRESH_MS = 1600
  let reviewRefreshTimer: ReturnType<typeof setTimeout> | undefined
  function scheduleReviewRefresh() {
    if (mainSurface !== 'review') return
    if (reviewRefreshTimer) return
    reviewRefreshTimer = setTimeout(() => {
      reviewRefreshTimer = undefined
      if (mainSurface !== 'review') return
      // A live DiffVsBase swap while the line-note composer is open re-anchors
      // the draft onto a different row. Defer until the composer dismisses.
      if (document.querySelector('.obs-review-host .icmt-compose')) return
      reviewRefresh++
    }, REVIEW_REFRESH_MS)
  }
  /** A path must stay missing this long across refreshes before we treat it as
   * gone. Git checkout/rework can unlink and recreate faster than one ListTrees
   * round-trip; counting consecutive misses without a time floor would collapse
   * an expanded dir (or close a tab) during that window. A first miss arms
   * `goneConfirmTimer` so a confirming refresh actually runs after this delay
   * even when the disk goes quiet. */
  const GONE_CONFIRM_MS = 600
  /** Stop re-arming the confirm timer after this many ms from the first miss.
   * Covers a couple of transient ListTrees failures without turning a dirty
   * buffer (or a stuck in-flight listing) into a permanent 600ms git poll. */
  const GONE_CONFIRM_MAX_MS = GONE_CONFIRM_MS * 3

  let liveRefreshAlive = true
  let goneConfirmTimer: ReturnType<typeof setTimeout> | undefined

  function clearLiveRefreshTimers() {
    refreshLoadedDirsPending = false
    refreshOpenFileIfStalePending = false
    if (liveRefreshTimer) {
      clearTimeout(liveRefreshTimer)
      liveRefreshTimer = undefined
    }
    if (reviewRefreshTimer) {
      clearTimeout(reviewRefreshTimer)
      reviewRefreshTimer = undefined
    }
    if (goneConfirmTimer) {
      clearTimeout(goneConfirmTimer)
      goneConfirmTimer = undefined
    }
  }

  function stopLiveRefresh() {
    liveRefreshAlive = false
    clearLiveRefreshTimers()
  }

  function armGoneConfirm() {
    if (!liveRefreshAlive || goneConfirmTimer) return
    goneConfirmTimer = setTimeout(() => {
      goneConfirmTimer = undefined
      if (!liveRefreshAlive) return
      void confirmGone()
    }, GONE_CONFIRM_MS)
  }

  async function confirmGone() {
    try {
      await Promise.all([refreshOpenFileIfStale(), refreshLoadedDirs()])
    } catch {
      // A rejected refresh must not skip the re-arm (fetchWorkspaceFileText
      // used to throw past this point and strand the miss).
    }
    if (!liveRefreshAlive) return
    if (goneConfirmStillDue()) armGoneConfirm()
  }

  function goneConfirmStillDue(): boolean {
    const now = Date.now()
    if (openFileNotFoundSince > 0 && now - openFileNotFoundSince < GONE_CONFIRM_MAX_MS) return true
    for (const t of dirNotFoundSince.values()) {
      if (now - t < GONE_CONFIRM_MAX_MS) return true
    }
    return false
  }

  function maybeClearGoneConfirm() {
    if (dirNotFoundSince.size > 0 || openFileNotFoundSince > 0) return
    if (!goneConfirmTimer) return
    clearTimeout(goneConfirmTimer)
    goneConfirmTimer = undefined
  }

  /** First-miss timestamps per dir. Evict only once a later miss is at least
   * GONE_CONFIRM_MS after the first; any success resets via deletion. */
  const dirNotFoundSince = new Map<string, number>()

  /** Guards refreshLoadedDirs against overlapping invocations: setBrowse fires
   * it directly (see above) while scheduleLiveRefresh's tick can already have
   * an identical call in flight. A trailing pending bit retries through the
   * debounce (not synchronously) so a create that landed mid-ListTrees is not
   * dropped, without two ListTrees landing milliseconds apart in one
   * unlink→recreate window. */
  let refreshLoadedDirsInFlight = false
  let refreshLoadedDirsPending = false

  /** Re-fetch just the dirs already on screen (root + expanded) so newly
   * created/removed files surface without collapsing the tree the user has open.
   * Fails silently per-dir. The root can't have been deleted by the agent (it
   * IS the worktree root), so it's never evicted — any failure there is
   * transient by definition. A non-root dir is only dropped from
   * loadedDirs/expanded after it has been missing for GONE_CONFIRM_MS across
   * refreshes: the engine's own git operations (rework's revert, `git checkout`,
   * patch application) can genuinely unlink and immediately recreate a path,
   * and a live-refresh can land in that window and see a real, but transient,
   * ENOENT. Any other error leaves it in place so the next cycle retries it,
   * instead of permanently disabling live-refresh for that subtree after one
   * blip — and it DOES reset that dir's missing timestamp, since a transient
   * error in between two not-founds breaks the "still gone" window. */
  async function refreshLoadedDirs() {
    if (!liveRefreshAlive) return
    if (!projectId && !workspaceId) return
    if (refreshLoadedDirsInFlight) {
      refreshLoadedDirsPending = true
      return
    }
    refreshLoadedDirsInFlight = true
    refreshLoadedDirsPending = false
    const token = rootToken
    try {
      // Shallowest first so the root's merge is applied before an already-loaded
      // subdir's patch — explicit ordering instead of relying on Set insertion
      // order (which happens to be parent-before-child today).
      const dirs = [...loadedDirs].sort((a, b) => a.split('/').length - b.split('/').length)
      const gone: string[] = []
      // One round-trip for the whole batch. Listing dirs one at a time meant a
      // separate whole-repo `git status` per directory (three subprocesses
      // each), serially, on every live-refresh — ListTrees resolves the status
      // once and answers for all of them. A dir the agent deleted is simply
      // absent from the map rather than failing the batch.
      let batch: Record<string, TreeEntry[] | undefined> | null = null
      try {
        batch = await CodeService.ListTrees(projectId, workspaceId, dirs)
        if (token !== rootToken) return
        worktreeMissing = false
      } catch (err) {
        // Whole-batch failure (not a per-dir miss): leave the tree as-is and
        // let the next tick retry rather than evicting anything. A vanished
        // worktree is this case — an empty map would stamp every dir gone.
        if (token !== rootToken) return
        if (isWorktreeMissing(err)) worktreeMissing = true
        return
      }
      let next = tree
      for (const dir of dirs) {
        const kids = batch?.[dir]
        if (kids) {
          next = dir === '' ? mergeChildren(next, kids) : patchChildren(next, dir, kids)
          dirNotFoundSince.delete(dir)
          continue
        }
        if (dir === '') continue
        const now = Date.now()
        const first = dirNotFoundSince.get(dir)
        if (first === undefined) {
          dirNotFoundSince.set(dir, now)
          armGoneConfirm()
        } else if (now - first >= GONE_CONFIRM_MS) {
          gone.push(dir)
        } else {
          armGoneConfirm()
        }
      }
      // Assign once: each reassignment deep-clones the tree and invalidates
      // every derived that reads it, so doing it per directory re-ran the whole
      // filtered render N times per tick.
      tree = next
      if (gone.length) {
        const goneSet = new Set(gone)
        for (const dir of gone) dirNotFoundSince.delete(dir)
        loadedDirs = new Set([...loadedDirs].filter((d) => !goneSet.has(d)))
        expanded = new Set([...expanded].filter((d) => !goneSet.has(d)))
      }
      maybeClearGoneConfirm()
    } finally {
      refreshLoadedDirsInFlight = false
      if (refreshLoadedDirsPending) scheduleLiveRefresh()
    }
  }

  /** First-miss timestamp for whichever file refreshOpenFileIfStale last
   * checked. Keyed implicitly by openFileNotFoundPath — a path change or a
   * successful read resets it. */
  let openFileNotFoundPath = ''
  let openFileNotFoundSince = 0

  /** Guards refreshOpenFileIfStale against overlapping invocations: setBrowse
   * fires it directly (see above) while scheduleLiveRefresh's tick can already
   * have an identical call in flight for the same path. A trailing pending bit
   * retries through the debounce so the last event of a write burst is not
   * swallowed by an in-flight ReadFile+Outline. */
  let refreshOpenFileIfStaleInFlight = false
  let refreshOpenFileIfStalePending = false

  /** Re-read the open file from disk when it hasn't been locally edited, so a
   * coding agent's writes show up in the pane it's already open in. Re-checks
   * dirty/selectedPath after the await (not just before) so a keystroke or a
   * navigation that happened while the read was in flight can't be clobbered
   * by a stale response. Fails silently; a not-found only closes the tab once
   * the path has stayed missing for GONE_CONFIRM_MS — a single miss is treated
   * as a possible transient blip (the engine's revert/checkout/patch-apply
   * machinery can genuinely unlink and immediately recreate a file, and a
   * live-refresh can land in that window), so it's ignored and retried next
   * cycle. Any other error (transient I/O, a race with the engine rewriting
   * the worktree) leaves the tab open, resets the missing timestamp (a
   * transient error in between two not-founds breaks the "still gone" window),
   * and just skips this cycle's refresh. */
  function isScratchPath(path: string): boolean {
    return path.startsWith('scratch://')
  }

  function diskOutline(path: string): Promise<OutlineSymbol[] | null> {
    return isAbsFilesystemPath(path) || isScratchPath(path)
      ? Promise.resolve(null)
      : call(CodeService.Outline(projectId, workspaceId, path), 'Outline failed')
  }

  /** Human tab label; disambiguate when several scratches share a title. */
  function scratchTabLabel(path: string): string {
    const base = scratchDocs[path]?.title?.trim() || path.replace(/^scratch:\/\/\d+-/, '').replace(/\.[^.]+$/, '') || 'Scratch'
    const peers = openFiles.filter(
      (p) => isScratchPath(p) && (scratchDocs[p]?.title?.trim() || '') === (scratchDocs[path]?.title?.trim() || ''),
    )
    if (peers.length <= 1) return base
    const n = peers.indexOf(path) + 1
    return n > 1 ? `${base} (${n})` : base
  }

  /**
   * Always create a brand-new scratch buffer + tab. Does not reuse an existing
   * scratch, route into the split pane, or ask to discard another dirty buffer.
   */
  function openScratchTab(title: string, contents: string, lang = 'txt', openInPreview = false) {
    scratchSeq += 1
    const ext = scratchLanguage(lang)
    const slug = title.replace(/[^\w.-]+/g, '_').replace(/^_+|_+$/g, '') || 'Scratch'
    const path = `scratch://${scratchSeq}-${slug}.${ext}`
    scratchDocs = { ...scratchDocs, [path]: { title, content: contents, lang: ext } }
    ensureOpenTab(path)
    stashCurrentBuffer()
    void activateScratchTab(path, openInPreview)
  }

  /** Ext / preview lang for an artifact kind (domain ArtifactKind). */
  function artifactScratchLang(kind: string, path: string): string {
    switch (String(kind || '').toLowerCase()) {
      case 'mermaid': return 'mmd'
      case 'html': return 'html'
      case 'svg': return 'svg'
      case 'png': return 'png'
      case 'jpeg':
      case 'jpg': return 'jpg'
      case 'gif': return 'gif'
      case 'webp': return 'webp'
      case 'csv': return 'csv'
      case 'json': return 'json'
      case 'md':
      case 'markdown': return 'md'
    }
    const base = path.split(/[/\\]/).pop() ?? ''
    const i = base.lastIndexOf('.')
    return i >= 0 ? scratchLanguage(base.slice(i + 1)) : 'txt'
  }

  /** Raster artifacts open as an image tab. SVG stays text so Code+Preview still work. */
  function artifactImageMIME(kind: string): string {
    switch (String(kind || '').toLowerCase()) {
      case 'png': return 'image/png'
      case 'jpeg':
      case 'jpg': return 'image/jpeg'
      case 'gif': return 'image/gif'
      case 'webp': return 'image/webp'
      default: return ''
    }
  }

  function bytesToBase64(buf: ArrayBuffer): string {
    const bytes = new Uint8Array(buf)
    const chunk = 0x8000
    let binary = ''
    for (let i = 0; i < bytes.length; i += chunk) {
      binary += String.fromCharCode(...bytes.subarray(i, i + chunk))
    }
    return btoa(binary)
  }

  /**
   * Artifacts live under the managed data dir (HTTP /workspaces/…), not the
   * git worktree — open them as a scratch tab (stable path per artifact id).
   */
  export async function openArtifact(a: {
    id?: string
    title?: string
    kind?: string
    path?: string
  }): Promise<boolean> {
    const id = (a.id || '').trim()
    const rel = (a.path || '').replace(/^\//, '')
    if (!id || !rel.startsWith('workspaces/')) {
      pushToast('error', 'Invalid artifact')
      return false
    }
    showEditorSurface()
    const url = '/' + rel
    const kind = String(a.kind || '').toLowerCase()
    const lang = artifactScratchLang(kind, rel)
    const path = `scratch://artifact-${id}.${lang}`
    const title = (a.title || '').trim() || id
    try {
      const res = await fetch(url)
      if (!res.ok) throw new Error(`not found (${res.status})`)
      const mime = artifactImageMIME(kind) || artifactImageMIME(lang)
      const isImage = !!mime
      const content = isImage ? bytesToBase64(await res.arrayBuffer()) : await res.text()
      scratchDocs = {
        ...scratchDocs,
        [path]: { title, content, lang, ...(mime ? { mime } : {}) },
      }
      ensureOpenTab(path)
      void activateScratchTab(path, !isImage && !!previewKindForPath(path))
      return true
    } catch (err: unknown) {
      pushToast('error', `Could not open artifact: ${bridgeErrorMessage(err)}`)
      return false
    }
  }

  function artifactTabPath(id: string): string {
    return openFiles.find((p) => artifactIdFromScratchPath(p) === id) ?? ''
  }

  function renameArtifactTab(a: { id: string; title?: string }) {
    const path = artifactTabPath(a.id)
    const doc = scratchDocs[path]
    if (doc) scratchDocs = { ...scratchDocs, [path]: { ...doc, title: a.title || a.id } }
  }

  function dropArtifactTab(id: string) {
    const path = artifactTabPath(id)
    if (!path) return
    dropOpenPath(path)
    const next = { ...scratchDocs }
    delete next[path]
    scratchDocs = next
  }

  async function activateScratchTab(path: string, openInPreview = false) {
    const gen = ++navGen
    focusedPane = 'main'
    if (path !== selectedPath) rememberLeavingEditors()
    selectedPath = path
    revealActiveFileTab()
    treePath = path
    highlightLine = 0
    openFileNotFoundPath = ''
    openFileNotFoundSince = 0
    maybeClearGoneConfirm()
    selectedIsDir = false
    baseDoc = ''
    baseDocPath = ''
    skipViewRestore = false
    // Commit the mode before the await — a rapid tab flip must never observe
    // (or a stale continuation stamp) the previous file's mode.
    const firstOpen = openInPreview && !fileViews[path]
    viewMode = firstOpen
      ? (previewKindForPath(path) ? 'preview' : 'code')
      : viewModeForPath(path)
    await hydrateFile(path)
    if (gen !== navGen) return
    await tick()
    if (gen !== navGen) return
    if (viewMode === 'code') restoreCodeView(path)
  }

  function collectPrompts(cmd: RegisteredCommand): Promise<Record<string, string> | null> {
    if (!cmd.prompts.length) return Promise.resolve({})
    return new Promise((resolve) => {
      promptDialog = {
        open: true,
        title: cmd.label,
        prompts: cmd.prompts,
        resolve: (v) => {
          promptDialog = { open: false, title: '', prompts: [], resolve: null }
          resolve(v)
        },
      }
    })
  }

  function workspaceRoot(): string {
    return $workspaces.find((w) => w.id === workspaceId)?.worktreePath?.replace(/\/$/, '') ?? ''
  }

  async function runEditorCommand(id: string) {
    const focused = focusedPane === 'split' && splitOpen && splitEditor
      ? splitEditor
      : editor
    if (!focused) {
      pushToast('error', 'Open a file in the editor first')
      return
    }
    const path = focusedPane === 'split' && splitOpen && splitPath ? splitPath : selectedPath
    const cmds = await listCommands(scopeForPath(path), path)
    const cmd = cmds.find((c) => c.id === id)
    if (!cmd) {
      pushToast('error', `Unknown command`)
      return
    }
    const inputs = await collectPrompts(cmd)
    if (inputs === null) return // cancelled

    const ctx = focused.getContext()
    ctx.clipboard = await readText()
    const root = workspaceRoot()
    ctx.projectDirectory = root
    ctx.workDir = root
    ctx.directory = ctx.path.includes('/') ? ctx.path.slice(0, ctx.path.lastIndexOf('/')) : ''

    try {
      const res = await runCommand(id, ctx, inputs)
      if (res.exitCode !== 0) {
        pushToast('error', res.stderr.trim() || `Command exited ${res.exitCode}`)
      }
      await applyCommandResult(
        focused,
        res.output,
        res.stdout,
        res.stderr,
        cmd.label,
        res.inputMode,
        res.language,
        res.openInPreview,
        res.exitCode,
      )
      // Palette / prompts stole focus — return it so the selection stays usable.
      if (res.output !== 'newDocument' || !res.openInPreview) focused.focus()
    } catch (err) {
      pushToast('error', bridgeErrorMessage(err) || 'Command failed')
      focused.focus()
    }
  }

  async function applyCommandResult(
    ed: CodeEditor,
    output: string,
    stdout: string,
    stderr: string,
    label: string,
    inputMode?: string,
    language?: string,
    openInPreview?: boolean,
    exitCode = 0,
  ) {
    const ok = exitCode === 0
    switch (output) {
      case 'replaceSelection':
      case 'replaceDocument':
      case 'replaceInput':
      case 'insertText':
      case 'insertSnippet': {
        // Failed macros often return empty stdout with the manifest routing —
        // never rewrite the buffer on a non-zero exit.
        if (!ok) break
        const next = ed.applyOutput(output, stdout, inputMode)
        if (next != null) {
          if (focusedPane === 'split' && splitOpen) {
            splitDraft = next
            if (splitPath === selectedPath) draft = next
          } else {
            draft = next
          }
        }
        break
      }
      case 'newDocument':
        if (!ok) break
        openScratchTab(label || 'Command Output', stdout, language || 'txt', !!openInPreview)
        break
      case 'copyToClipboard':
        // Allowed on failure when stdout carries a message (error toast already shown).
        if (!ok && !stdout) break
        await copyText(stdout)
        pushToast('success', 'Copied to clipboard')
        break
      case 'showAsToast':
        if (stdout.trim()) pushToast('info', stdout.trim())
        else if (ok) pushToast('info', label)
        break
      case 'showAsTooltip':
        if (stdout.trim()) pushToast('info', stdout.trim())
        else if (ok) pushToast('info', label)
        break
      case 'discard':
        // Error toast already shown by the caller when !ok.
        if (!ok) break
        if (stderr.trim()) pushToast('error', stderr.trim())
        break
      default:
        if (ok) {
          pushToast(
            'error',
            stderr.trim() || `Unknown output routing ${JSON.stringify(output)}`,
          )
        }
        break
    }
  }

  async function refreshOpenFileIfStale() {
    if (!liveRefreshAlive) return
    const path = selectedPath
    if (!path || isScratchPath(path) || isAbsFilesystemPath(path)) return
    if (dirty) {
      // Can't confirm (or close) while the user is editing — dropping the
      // tracker stops confirmGone from polling ListTrees every 600ms for as
      // long as the buffer stays dirty.
      if (openFileNotFoundSince > 0) {
        openFileNotFoundPath = ''
        openFileNotFoundSince = 0
        maybeClearGoneConfirm()
      }
      return
    }
    // editorPath lags selectedPath until hydrate lands. Applying a refresh for
    // B while the visible editor is still A would pair B's FileContent with A's
    // text (Cmd+S writes A into B). hydrateFile owns that window.
    if (editorPath !== path) return
    if (refreshOpenFileIfStaleInFlight) {
      refreshOpenFileIfStalePending = true
      return
    }
    refreshOpenFileIfStaleInFlight = true
    refreshOpenFileIfStalePending = false
    try {
      let f: FileContent
      try {
        f = await CodeService.ReadFile(projectId, workspaceId, path)
      } catch (err) {
        if (dirty || selectedPath !== path || editorPath !== path) return
        if (isWorktreeMissing(err)) {
          // Whole-tree outage — not a per-file delete. Keep the tab/session;
          // the banner explains the state until the folder is back.
          worktreeMissing = true
          return
        }
        if (isNotFound(err)) {
          const now = Date.now()
          if (openFileNotFoundPath !== path) {
            openFileNotFoundPath = path
            openFileNotFoundSince = now
            armGoneConfirm()
          } else if (openFileNotFoundSince > 0 && now - openFileNotFoundSince >= GONE_CONFIRM_MS) {
            openFiles = openFiles.filter((p) => p !== path)
            file = null
            selectedPath = ''
            editorPath = ''
            draft = ''
            savedContent = ''
            openFileNotFoundPath = ''
            openFileNotFoundSince = 0
            maybeClearGoneConfirm()
            if (!openFiles.length) persistOrClearEditorSession()
          } else {
            armGoneConfirm()
          }
        } else if (openFileNotFoundPath === path) {
          openFileNotFoundPath = ''
          openFileNotFoundSince = 0
          maybeClearGoneConfirm()
        }
        return
      }
      if (openFileNotFoundPath === path) {
        openFileNotFoundPath = ''
        openFileNotFoundSince = 0
        maybeClearGoneConfirm()
      }
      if (dirty || selectedPath !== path || editorPath !== path) return
      // ReadFile talks to the worktree (unlike GitStatus, which returns empty
      // while the folder is gone). Clear before the unchanged-content returns
      // so a live-refresh tick can drop the banner without waiting on ListTrees.
      worktreeMissing = false
      if (f.content) {
        if (f.content === file?.content) return
      } else if (f.size === file?.size && f.mtime === file?.mtime) {
        return
      }
      if (fileNeedsHTTPBody(f)) {
        try {
          f.content = await fetchWorkspaceFileText(projectId, workspaceId, path)
        } catch {
          return
        }
        if (dirty || selectedPath !== path || editorPath !== path) return
      }
      file = f
      draft = f.content
      savedContent = f.content
      // Content actually moved (not just a no-op refresh), so the Symbols/Outline
      // panel and any highlightLine-based anchors would otherwise silently
      // drift out of sync with the refreshed body for as long as the file
      // stays open — openPath fetches Outline on every open; mirror that here.
      // This branch only runs when size/mtime moved, not on every live-refresh.
      const o = await resolveFileOutline(
        path,
        'main',
        quiet(CodeService.Outline(projectId, workspaceId, path)),
      )
      if (!dirty && selectedPath === path && editorPath === path && o) outline = o
    } finally {
      refreshOpenFileIfStaleInFlight = false
      if (refreshOpenFileIfStalePending) scheduleLiveRefresh()
    }
  }

  let changedFilesGen = 0

  async function refreshChangedFiles() {
    if (!workspaceId || !isGit) {
      if (!isGit) changedPaths = []
      return
    }
    const gen = ++changedFilesGen
    const ch = await quiet(CodeService.ListChangedFiles(workspaceId))
    if (gen !== changedFilesGen) return
    if (!ch) return
    // Unlike the initial fetch in refreshTree (which decides the starting
    // scope once, at mount), this runs on every live-refresh tick. If changed
    // files just appeared for the first time — going from empty to
    // non-empty — while the user was already looking at the full tree
    // (showChangedFirst was false precisely because changedPaths was empty),
    // keep them there instead of yanking the view over to "Changed in loop"
    // the instant the agent's first edit lands.
    if (!scopeChosen && loopScoped && !browseAll && changedPaths.length === 0 && ch.length > 0) {
      browseAll = true
    }
    changedPaths = ch
  }

  // Live repo-state sync: the engine watches the ON-SCREEN workspace's worktree
  // (fswatch.go — one recursive FSEvents stream) and publishes workspace.fs_changed
  // whenever the disk changes — no matter who changed it: an agent turn, the
  // user's editor, a terminal, a git command. That single trigger drives this
  // short-debounce refresh, so the changed-files list, source-control status,
  // on-screen tree, and (if unedited) the currently open file stay in near-
  // real-time sync with the worktree. Kernel coalescing (400ms, NoDefer) already
  // batches bursts; this timer only collapses same-tick events into one RPC.
  let liveRefreshTimer: ReturnType<typeof setTimeout> | undefined

  function scheduleLiveRefresh() {
    if (!liveRefreshAlive) return
    if (liveRefreshTimer) return
    liveRefreshTimer = setTimeout(() => {
      liveRefreshTimer = undefined
      if (!liveRefreshAlive) return
      void refreshChangedFiles()
      void refreshGitStatus()
      void refreshOpenFileIfStale()
      scheduleReviewRefresh()
      // Refresh whichever tree is actually on screen: the full tree when
      // browsing all files, but ALSO the full tree in the default loop-scoped
      // state before showChangedFirst has anything to show (loopScoped with
      // an empty changedPaths renders the full tree, per the `files` panel
      // body below) — gating on browseAll alone left that view frozen.
      // A pending ListTrees retry (create landed mid-flight) must run even
      // in changed-first mode so the dropped refresh is not discarded.
      // worktreeMissing too: Changed view with a PR keeps showChangedFirst
      // true (ListChangedFiles still returns PR paths while the folder is
      // gone), so without this the banner never clears after the tree returns.
      if (browseAll || !showChangedFirst || refreshLoadedDirsPending || worktreeMissing) void refreshLoadedDirs()
    }, LIVE_REFRESH_MS)
  }

  // IMPORTANT: this effect must not synchronously read `workspaceId` (or anything
  // else reactive) at its top level. Handling a live event flows through the
  // `workspaces` store (see api.ts's wireEvents), which the parent view derives
  // its `loop` object from — and that derivation hands this component a
  // fresh-but-equal `workspaceId` prop on every single event. An effect that reads
  // `workspaceId` up front gets re-scheduled by Svelte on every such parent
  // re-render, and Svelte always runs the PREVIOUS cleanup before re-running the
  // body — so the just-armed scheduleLiveRefresh() timer below was being cancelled
  // by clearTimeout on almost every cycle, silently starving the live refresh.
  // Reading `workspaceId` only inside the callback (invoked later, outside tracked
  // execution) keeps this effect dependency-free: it subscribes exactly once per
  // mount and always sees the current workspaceId when the event arrives.
  $effect(() => {
    const unsub = onWorkspaceFs((changedId) => {
      if (!workspaceId || changedId !== workspaceId) return
      if (!shellActive) return
      scheduleLiveRefresh()
    })
    return () => {
      unsub()
      clearLiveRefreshTimers()
    }
  })

  // A reviewer (loop iteration or Review-mode chat) filed a finding — refetch
  // so the findings panel tracks it live.
  $effect(() => onFindingsUpdated((wid) => {
    if (!workspaceId || wid !== workspaceId) return
    void loadFindings()
  }))

  function bumpUnseenArtifacts(wid: string, artifactId: string) {
    unseenArtifactsByWorkspace = addUnseenArtifactId(unseenArtifactsByWorkspace, wid, artifactId)
  }
  function clearUnseenArtifacts(wid: string) {
    unseenArtifactsByWorkspace = clearUnseenArtifactIds(unseenArtifactsByWorkspace, wid)
  }

  // Badge the Artifacts pane when something new lands while that panel isn't open.
  // Do not read workspaceId / artifactsPanelFocused in the effect body — that
  // resubscribes on every parent re-render (same trap as scheduleLiveRefresh).
  $effect(() => {
    return onArtifactCreated((wid, art) => {
      if (!wid) return
      if (wid === workspaceId && artifactsPanelFocused) return
      const a = artifactFromEvent(art)
      if (!a) return
      bumpUnseenArtifacts(wid, a.id)
    })
  })
  $effect(() => {
    if (artifactsPanelFocused && workspaceId) clearUnseenArtifacts(workspaceId)
  })
  // present_artifact must be handled here: ArtifactsPanel is only mounted while
  // the pane is already selected, so a listener there never fires for a closed pane.
  $effect(() => {
    return onArtifactPresented((wid, art) => {
      if (!wid) return
      const a = artifactFromEvent(art)
      if (!a) return
      // Another workspace's Observatory is not mounted — badge it so the
      // human sees a signal when they switch over. Same id as a prior create
      // does not increment again.
      if (wid !== workspaceId) {
        bumpUnseenArtifacts(wid, a.id)
        return
      }
      if (!shellActive) {
        bumpUnseenArtifacts(wid, a.id)
        return
      }
      void (async () => {
        await selectCodePanel('artifacts')
        await openArtifact(a)
      })()
    })
  })

  // Arm the engine's worktree watcher for the workspace on screen. Eager create
  // publishes worktreePath after `git worktree add` while phase is still
  // creating — WatchWorkspace no-ops on an empty path, so we re-arm when the
  // path lands (not only when workspaceId changes). Pass the id even before
  // the path exists: WatchWorkspace('') clears chat focus as if the Observatory
  // were left. Track id:path so the empty-path call does not skip the later
  // re-arm. The guard makes equal-value re-runs no-ops; a cleanup would restart
  // the watcher on every parent re-render (see the starvation note above).
  // Unmount disarms once.
  let watchArmedFor = ''
  $effect(() => {
    if (!shellActive) return
    if ($workspaceScanPending) return
    const id = workspaceId
    const path = worktreePath
    const key = id ? id + ':' + path : ''
    if (key === watchArmedFor) return
    watchArmedFor = key
    watchOwner = key
    WorkspacesService.WatchWorkspace(id || '')
  })
  $effect(() => {
    if (shellActive) return
    if (!watchArmedFor) return
    if (watchOwner === watchArmedFor) {
      watchOwner = ''
      WorkspacesService.WatchWorkspace('')
    }
    watchArmedFor = ''
  })
  onDestroy(() => {
    fileLoadAbort?.abort()
    stopLiveRefresh()
    if (watchArmedFor && watchOwner === watchArmedFor) {
      watchOwner = ''
      WorkspacesService.WatchWorkspace('')
    }
    if (workspaceId) {
      clearLspForWorkspace(workspaceId)
      void CloseWorkspaceLSP(workspaceId)
    }
    detachLsp()
  })

  // Register command/snippet runners + focus provider for ⌘⇧P / Bundles.
  $effect(() => {
    if (!shellActive) return
    const id = workspaceId
    commandOwner = id
    setEditorCommandHandler((cmdId) => runEditorCommand(cmdId))
    setEditorSnippetHandler((sn) => insertBundleSnippet(sn))
    setEditorFocusProvider(() => {
      const path = focusedPane === 'split' && splitOpen && splitPath ? splitPath : selectedPath
      return { scope: scopeForPath(path), path }
    })
    return () => {
      if (commandOwner !== id) return
      commandOwner = ''
      setEditorCommandHandler(null)
      setEditorSnippetHandler(null)
      setEditorFocusProvider(null)
    }
  })

  // Moving a parked shell into the live slot changes the box (sidebar, view
  // chrome). Resync CodeMirror's measure; xterm's ResizeObserver covers the dock.
  let wasShellActive = true
  /** Bumped when openPath reveals a line so deferred parked-scroll restore cannot clobber it. */
  let editorRevealGen = 0
  $effect(() => {
    const on = shellActive
    if (on && !wasShellActive) {
      const zone = untrack(() => focusZone)
      const token = rootToken
      const revealAt = untrack(() => editorRevealGen)
      const jump = untrack(() => get(observatoryJump))
      const skipParkedScroll = !!(
        jump?.path && (!jump.workspaceId || jump.workspaceId === workspaceId)
      )
      const shell = obsEl?.closest('.wsv-shell')
      const parkedTop = skipParkedScroll
        ? undefined
        : (shell instanceof HTMLElement
          ? peekHotShellEditorScrollTop(shell)
          : undefined)
      const applyParkedScroll = () => {
        if (rootToken !== token || editorRevealGen !== revealAt) return
        if (parkedTop != null) editor?.restoreScrollTop(parkedTop)
      }
      editor?.resync()
      splitEditor?.resync()
      applyParkedScroll()
      scheduleLiveRefresh()
      if (termOverlaying && obsEl) applyOverlayRect(overlayRect(obsEl))
      void tick().then(() => {
        if (rootToken !== token) return
        editor?.resync()
        splitEditor?.resync()
        applyParkedScroll()
        if (termOverlaying && obsEl) applyOverlayRect(overlayRect(obsEl))
        requestAnimationFrame(() => {
          requestAnimationFrame(() => {
            if (rootToken !== token) return
            restoreChromeFocus(zone, token)
            requestAnimationFrame(() => {
              requestAnimationFrame(() => {
                applyParkedScroll()
              })
            })
          })
        })
      })
    }
    wasShellActive = on
  })

  $effect(() => {
    const id = workspaceId
    if (!shellActive || !id) return
    setLspDisplayFile(id, async (relPath) => {
      await openPath(relPath)
      await tick()
      return editor?.getView() ?? null
    })
    setLspFileIO(id, {
      unsaved(relPath) {
        if (relPath === selectedPath) return editor?.getDoc() ?? draft
        if (relPath === splitPath) return splitEditor?.getDoc() ?? splitDraft
        return tabBuffers[relPath]?.draft ?? null
      },
      async read(relPath) {
        try {
          const f = await readEditorFile(relPath)
          return f.content
        } catch {
          return null
        }
      },
      applyIfOpen(relPath, text) {
        if (relPath === selectedPath && editor?.getView()) {
          const v = editor.getView()!
          v.dispatch({ changes: { from: 0, to: v.state.doc.length, insert: text }, userEvent: 'lsp.fix' })
          draft = text
          return true
        }
        if (relPath === splitPath && splitEditor?.getView()) {
          const v = splitEditor.getView()!
          v.dispatch({ changes: { from: 0, to: v.state.doc.length, insert: text }, userEvent: 'lsp.fix' })
          splitDraft = text
          return true
        }
        return false
      },
      async write(relPath, text) {
        const buf = tabBuffers[relPath]
        if (buf) {
          putTabBuffer(relPath, { ...buf, draft: text, file: fileWithBody(buf.file, text) })
          markTabDirty(relPath, !editorTextEq(text, buf.saved))
          return
        }
        await callOk(CodeService.WriteFile(projectId, workspaceId, relPath, text), 'Apply edit failed')
      },
    })
    return () => {
      setLspDisplayFile(id, null)
      setLspFileIO(id, null)
    }
  })

  async function toggleDir(path: string, forceOpen = false) {
    if (filterChanged) {
      // Derived subtree: collapse state is local and there is nothing to fetch.
      const next = new Set(changedCollapsed)
      if (forceOpen || next.has(path)) next.delete(path)
      else next.add(path)
      changedCollapsed = next
      return
    }
    const isOpen = expanded.has(path)
    if (isOpen && !forceOpen) {
      const next = new Set(expanded)
      next.delete(path)
      expanded = next
      return
    }
    await ensureDirLoaded(path)
    expanded = new Set([...expanded, path])
  }

  /**
   * Expand ancestors of `filePath` so the explorer selection is visible.
   * Uses one ListTrees round-trip for any unloaded dirs (same pattern as
   * expandChangedAncestors) instead of N serial ListTree calls.
   */
  async function revealInTree(filePath: string) {
    if (!filePath) return
    const parts = filePath.split('/').filter(Boolean)
    if (parts.length <= 1) return
    const dirs: string[] = []
    let acc = ''
    for (let i = 0; i < parts.length - 1; i++) {
      acc = acc ? `${acc}/${parts[i]}` : parts[i]
      dirs.push(acc)
    }
    if (filterChanged) {
      const next = new Set(changedCollapsed)
      for (const d of dirs) next.delete(d)
      changedCollapsed = next
      return
    }
    const token = rootToken
    const missing = dirs.filter((d) => !loadedDirs.has(d))
    if (missing.length) {
      let batch: Record<string, TreeEntry[] | undefined>
      try {
        batch = await CodeService.ListTrees(projectId, workspaceId, missing)
      } catch {
        return
      }
      if (token !== rootToken) return
      let next = tree
      const loaded = new Set(loadedDirs)
      for (const d of missing) {
        const kids = batch?.[d]
        if (!kids) continue
        next = d === '' ? mergeChildren(next, kids) : patchChildren(next, d, kids)
        loaded.add(d)
      }
      tree = next
      loadedDirs = loaded
    }
    if (token !== rootToken) return
    expanded = new Set([...expanded, ...dirs])
  }

  /** Files explorer is the active panel on its dock and that dock is showing. */
  function filesExplorerIsShowing(): boolean {
    const side = $codePanelSides.files
    if (side === 'left') return hasLeft && leftGutterOpen && leftKind === 'files'
    return hasRight && showSide && rightKind === 'files'
  }

  /**
   * `reveal` — select in the tree and expand ancestors (tree click / tab).
   * `scroll` — same as reveal, then scroll the Files list to the row when
   * Files is showing and the row is actually rendered (Quick Open).
   * `if-visible` — chat chips: only select/scroll if Files is open and the row
   * is already in the current filtered listing. Never steal the side panel.
   */
  type ExplorerReveal = 'reveal' | 'scroll' | 'if-visible'

  function shouldSelectInExplorer(path: string, explorer: ExplorerReveal): boolean {
    if (explorer === 'reveal' || explorer === 'scroll') return true
    if (!filesExplorerIsShowing()) return false
    return isPathVisibleInExplorerTree(
      visibleTree,
      path,
      (dir) => (filterChanged ? !changedCollapsed.has(dir) : expanded.has(dir)),
      (n) => (filterChanged ? (n.children ?? []) : visibleEntries(n.children as TreeEntry[] | undefined, false)),
    )
  }

  async function scrollExplorerRow(path: string) {
    if (treePath !== path || !filesExplorerIsShowing()) return
    await tick()
    const pane = filesScrollEl()
    if (!pane) return
    const row = [...pane.querySelectorAll<HTMLElement>('.obs-node')]
      .find((el) => el.dataset.treePath === path)
    if (!row) return
    // Scroll the list itself — Element.scrollIntoView also moves ancestor
    // overflow containers (#v-detail / the editor).
    const view = pane.getBoundingClientRect()
    const rowRect = row.getBoundingClientRect()
    const rowH = parseFloat(getComputedStyle(pane).getPropertyValue('--tree-row-h')) || 24
    const ancestors = Math.max(0, path.split('/').filter(Boolean).length - 1)
    const delta = scrollDeltaToReveal(rowRect.top, rowRect.bottom, view.top, view.bottom, ancestors * rowH, 0)
    if (delta === 0) return
    const max = Math.max(0, pane.scrollHeight - pane.clientHeight)
    pane.scrollTop = Math.min(max, Math.max(0, pane.scrollTop + delta))
  }

  /** Expand ancestors for reveal/scroll; scroll the Files list only for `scroll` / `if-visible`. */
  function syncExplorerToPath(path: string, explorer: ExplorerReveal) {
    if (explorer === 'if-visible') {
      void scrollExplorerRow(path)
      return
    }
    const revealed = revealInTree(path)
    if (explorer === 'scroll') void revealed.then(() => scrollExplorerRow(path))
    else void revealed
  }

  type ViewStateSource = { getViewState: () => Omit<FileViewState, 'viewMode'> | null } | undefined

  function rememberEditorView(path: string, ed: ViewStateSource, mode: FileViewState['viewMode']) {
    if (!path) return
    const prev = fileViews[path]
    let live: Omit<FileViewState, 'viewMode'> | null | undefined
    try {
      live = ed?.getViewState?.()
    } catch {
      live = undefined
    }
    fileViews = {
      ...fileViews,
      [path]: {
        viewMode: mode,
        line: live?.line ?? prev?.line ?? 1,
        col: live?.col ?? prev?.col ?? 1,
        scrollTop: live?.scrollTop ?? prev?.scrollTop ?? 0,
        scrollLeft: live?.scrollLeft ?? prev?.scrollLeft ?? 0,
        topLine: live?.topLine ?? prev?.topLine,
        ...previewScroll.previewScrollFields(path, prev),
        ...(live
          ? (live.anchorLine != null
            ? { anchorLine: live.anchorLine, anchorCol: live.anchorCol }
            : {})
          : (prev?.anchorLine != null
            ? { anchorLine: prev.anchorLine, anchorCol: prev.anchorCol }
            : {})),
      },
    }
  }

  function rememberLeavingEditors() {
    // In Diff mode the code editor is hidden and unscrolled — the merge pane
    // holds the position the user actually sees. During a rapid tab run the
    // code editor may still show the *previous* file (editorPath lags
    // selectedPath until hydrate lands) — record only the mode then, never
    // another file's position. (The merge pane is keyed by selectedPath, so
    // when it exists it is always showing the right file.)
    if (selectedPath) {
      const src = viewMode === 'diff'
        ? mergeEditor
        : (editorPath === selectedPath ? editor : undefined)
      rememberEditorView(selectedPath, src, viewMode)
    }
    if (splitOpen && splitPath && splitPath !== selectedPath) {
      rememberEditorView(splitPath, splitEditor, fileViews[splitPath]?.viewMode ?? 'code')
    }
  }

  function viewModeForPath(path: string, opts: {
    wantDiff?: boolean
    deleted?: boolean
  } = {}): FileViewState['viewMode'] {
    const saved = fileViews[path]?.viewMode
    // Trust a saved Diff chip only while this root's GitStatus is still in
    // flight. Once it has landed, canDiff is the real answer — otherwise a
    // clean file would flash Diff on every cold restore.
    const pendingDiff = restoringSessionViews && !gitStatusReady && saved === 'diff'
    const image = isBinaryImagePath(path)
    const media = isMediaPath(path)
    const status = gitStatus[path] || ''
    return resolveRestoredViewMode(saved, {
      wantDiff: opts.wantDiff,
      deleted: media ? false : opts.deleted,
      canDiff: media
        ? false
        : image
          ? status === 'm' || status === 'd' || !!opts.deleted || pendingDiff
          : !!(status || opts.deleted || pendingDiff),
      canPreview: (image || media) ? status !== 'd' && !opts.deleted : !!previewKindForPath(path),
      isImage: image || media,
    })
  }

  function restoreCodeView(path: string, line = 0, endLine = 0, ed?: CodeEditor, startCol = 0, endCol = 0) {
    const target = ed ?? editor
    if (draftEdit) {
      target?.resync()
      return
    }
    if (line > 0) {
      if (ed) ed.scrollToLine(line, endLine, startCol, endCol)
      else scrollToLine(line, endLine, startCol, endCol)
      return
    }
    const vs = skipViewRestore ? undefined : fileViews[path]
    if (vs) target?.restoreViewState(vs)
    else target?.focus()
    target?.resync()
  }

  /**
   * Navigation generation. openPath/activateScratchTab mutate viewMode and
   * caret state after awaits; with rapid tab flips (⌥⌘←/→) a slower open's
   * continuation would otherwise resume after a newer one finished and stamp
   * its file's mode/scroll onto the file now on screen.
   */
  let navGen = 0

  function dropStash(path: string) {
    if (!path || !stashedBuffers[path]) return
    const next = { ...stashedBuffers }
    delete next[path]
    stashedBuffers = next
  }

  function stashPath(path: string, body: string, saved: string) {
    if (!path || isScratchPath(path)) return
    if (editorTextEq(body, saved)) {
      dropStash(path)
      return
    }
    stashedBuffers = { ...stashedBuffers, [path]: { draft: body, saved } }
  }

  function warnStashDiskMoved(path: string) {
    const name = path.split('/').pop() || path
    pushToast(
      'info',
      `${name} changed on disk while you had unsaved edits. Showing your draft; save will overwrite the disk version.`,
      undefined,
      8000,
    )
  }

  /**
   * A live stash carries the disk text it diverged from (compare directly); a
   * stash restored from the backend carries only that text's hash — compare it
   * against the freshly read file off the sync path (the draft is already on
   * screen either way; drift only decides whether to warn).
   */
  function checkStashDiskMoved(path: string, s: StashedBuffer, disk: string) {
    if (typeof s.saved === 'string') {
      if (!editorTextEq(s.saved, disk)) warnStashDiskMoved(path)
      return
    }
    const baseHash = s.baseHash
    if (!baseHash) return
    void editorContentHash(disk).then((h) => {
      if (h !== baseHash && stashedBuffers[path] === s) warnStashDiskMoved(path)
    })
  }

  function overlayStash(path: string) {
    const s = stashedBuffers[path]
    if (!s) return
    draft = s.draft
    mainEditorDirty = !editorTextEq(s.draft, savedContent)
    if (!mainEditorDirty) dropStash(path)
    else checkStashDiskMoved(path, s, savedContent)
  }

  function overlaySplitStash(path: string) {
    const s = stashedBuffers[path]
    if (!s) return
    splitDraft = s.draft
    splitEditorDirty = !editorTextEq(s.draft, splitSaved)
    if (!splitEditorDirty) dropStash(path)
    else checkStashDiskMoved(path, s, splitSaved)
  }

  function readMainDoc(): string {
    try {
      if (showDiff) return mergeEditor?.getDoc() ?? draft
      if (editor && file && !isBinaryPreviewMime(file.mime)) return editor.peekDoc()
    } catch { /* view may be mid-teardown */ }
    return draft
  }

  function stashCurrentBuffer() {
    const path = selectedPath
    if (!path) return
    if (isScratchPath(path)) {
      const body = readMainDoc()
      scratchDocs = {
        ...scratchDocs,
        [path]: { ...(scratchDocs[path] ?? { title: 'Scratch', lang: 'txt' }), content: body },
      }
      return
    }
    if (!file || isBinaryPreviewMime(file.mime)) return
    stashPath(path, readMainDoc(), savedContent)
  }

  function stashSplitBuffer() {
    if (!splitPath || isScratchPath(splitPath)) return
    let body = splitDraft
    try {
      if (splitEditor) body = splitEditor.peekDoc()
    } catch { /* ignore */ }
    stashPath(splitPath, body, splitSaved)
  }

  function pathIsDirty(path: string): boolean {
    if (!path) return false
    // Live dirty for the mounted pane — a stash retained by overlayStash can
    // lag behind undo/delete-to-original and would otherwise keep the chip lit.
    if (path === selectedPath && file) return dirty
    if (splitOpen && path === splitPath) return splitDirty
    return !!stashedBuffers[path]
  }

  function sessionStashedBuffers(): Record<string, StashedBuffer> {
    const buffers = { ...stashedBuffers }
    const put = (path: string, body: string, saved: string) => {
      if (!path || isScratchPath(path)) return
      if (editorTextEq(body, saved)) {
        delete buffers[path]
        return
      }
      buffers[path] = { draft: body, saved }
    }
    if (selectedPath && file && !isBinaryPreviewMime(file.mime)) {
      put(selectedPath, readMainDoc(), savedContent)
    }
    if (splitOpen && splitPath) {
      let body = splitDraft
      try { if (splitEditor) body = splitEditor.peekDoc() } catch { /* ignore */ }
      put(splitPath, body, splitSaved)
    }
    return buffers
  }

  function writeEditorSession(flush: boolean, pid = projectId, wid = workspaceId) {
    if (!pid && !wid) return
    // Existence check must not subscribe this persist path to `$workspaces` —
    // that store updates on every streamed workspace.event/updated.
    if (wid && wid !== '_' && !untrack(() => $workspaces.some((w) => w.id === wid))) return
    const syn = untrack(() => syntaxModeSnapshot())
    if (!openFiles.length && !syntaxMapsNonEmpty(syn) && mainSurfacePref !== 'review' && !paneChromeNonDefault()) return
    const keep = [...openFiles]
    if (splitOpen && splitPath) keep.push(splitPath)
    // Capture live caret only on flush (quit / root switch). The persist
    // $effect runs on every debounced editor emit (~120ms) and must not force
    // layout (getBoundingClientRect / posAtCoords) or reallocate fileViews.
    const views = untrack(() => {
      if (!restoringSessionViews && flush) rememberLeavingEditors()
      return previewScroll.applyPreviewScrollToFileViews(pruneFileViews(fileViews, keep), keep)
    })
    saveEditorSession(pid, wid, {
      openFiles,
      selectedPath,
      splitPath: splitOpen && splitPath ? splitPath : undefined,
      scratchDocs: pruneScratchDocs(scratchDocs, keep),
      scratchSeq,
      stashedBuffers: sessionStashedBuffers(),
      fileViews: views,
      syntaxByPath: syn.syntaxByPath,
      syntaxByFileType: syn.syntaxByFileType,
      syntaxMapsKnown: syntaxMapsReady,
      mainSurface: mainSurfacePref === 'review' ? 'review' : undefined,
      leftKind,
      rightKind,
      leftOpen: leftGutterOpen,
      rightOpen: showSide,
    })
    if (flush) flushEditorSessionNow()
  }

  /**
   * Restore is an async RPC now, so leaving a root with no tabs open is
   * ambiguous: either there was nothing to restore / the user closed
   * everything (clear is right), or the restore simply hadn't landed yet
   * (clearing would delete a stored session that was never even shown). Only
   * clear once the load for this root actually settled.
   */
  let sessionLoadSettled = false
  /** False until restore has authoritative syntax maps (not an unsettled journal). */
  let syntaxMapsReady = false

  function paneChromeNonDefault(): boolean {
    return leftKind !== 'maestro' || rightKind !== 'files' || !showSide || !leftGutterOpen
  }

  function panesAtDefaults(): boolean {
    return leftKind === 'maestro' && rightKind === 'files' && showSide && leftGutterOpen
  }

  /** Cold restore of dock kind/visibility. Skip if the user already moved chrome. */
  function applySessionPaneChrome(sess: Session | null | undefined) {
    if (!sess || !panesAtDefaults()) return
    if (isCodePanelKey(sess.leftKind)) leftKind = sess.leftKind
    if (isCodePanelKey(sess.rightKind)) rightKind = sess.rightKind
    if (typeof sess.rightOpen === 'boolean') showSide = sess.rightOpen
    if (typeof sess.leftOpen === 'boolean') setLeftGutterOpen(sess.leftOpen)
  }

  function persistOrClearEditorSession(pid = projectId, wid = workspaceId) {
    if (!openFiles.length && !syntaxMapsNonEmpty() && mainSurfacePref !== 'review' && !paneChromeNonDefault()) {
      if (sessionLoadSettled && syntaxMapsReady) clearEditorSession(pid, wid)
      return
    }
    writeEditorSession(true, pid, wid)
  }

  function persistSessionForRoot(root: string) {
    if (!root || root === '\0') return
    const i = root.indexOf('\0')
    const pid = i >= 0 ? root.slice(0, i) : root
    const wid = i >= 0 ? root.slice(i + 1) : ''
    if (!openFiles.length && !syntaxMapsNonEmpty() && mainSurfacePref !== 'review' && !paneChromeNonDefault()) {
      if (sessionLoadSettled && syntaxMapsReady) clearEditorSession(pid, wid)
      return
    }
    writeEditorSession(true, pid, wid)
  }

  async function openPath(path: string, line = 0, preferDiff = false, endLine = 0, explorer: ExplorerReveal = 'reveal', fromDisk = false, startCol = 0, endCol = 0) {
    if (!path) return
    if (imageBaseFailedSrc) imageBaseFailedSrc = ''
    reviewUnhidFor = ''
    if (!restoringSessionViews) setMainSurface('editor')
    // Opening a file is an editor action — bring the surface back if it was
    // tucked away for a terminal-first layout.
    if (workspaceId && editorHidden) setEditorHidden(workspaceId, false)
    const st = gitStatus[path] || ''
    const imagePath = isBinaryImagePath(path)
    const mediaPath = isMediaPath(path)
    const wantDiff = preferDiff || st === 'd'
    skipViewRestore = line > 0
    if (line > 0) editorRevealGen++
    rememberLeavingEditors()
    // With a split open, tree / Quick Open / tabs land in the focused pane
    // (Diff / deleted files stay on the main pane — merge UI is single-pane).
    // Scratch buffers always open as their own main-pane tabs.
    if (splitOpen && focusedPane === 'split' && !wantDiff && !imagePath && !mediaPath && !isScratchPath(path)) {
      await openInSplit(path, line, endLine, explorer, fromDisk, startCol, endCol)
      return
    }
    // Re-hydrating the current file from disk would wipe unsaved edits. Reload /
    // git-discard pass fromDisk after the user confirmed. Don't early-return
    // when the previous open failed (file still null) — re-click should retry.
    if (!fromDisk && path === selectedPath && (file || selectedIsDir || fileLoading)) {
      if (preferDiff && (canShowDiff || canShowImageDiff)) await setViewMode('diff')
      if (line > 0) {
        if (viewMode === 'code' || viewMode === 'diff') scrollToLine(line, endLine, startCol, endCol)
      } else if (showDiff) {
        mergeEditor?.focus()
      } else {
        editor?.focus()
      }
      revealActiveFileTab()
      if (shouldSelectInExplorer(path, explorer)) treePath = path
      syncExplorerToPath(path, explorer)
      return
    }
    if (fromDisk) dropStash(path)
    else stashCurrentBuffer()
    if (isScratchPath(path) && path !== selectedPath) {
      if (shouldSelectInExplorer(path, explorer)) treePath = path
      ensureOpenTab(path)
      await activateScratchTab(path)
      if (line > 0) scrollToLine(line, endLine, startCol, endCol)
      if (explorer === 'if-visible') void scrollExplorerRow(path)
      return
    }
    const gen = ++navGen
    focusedPane = 'main'
    selectedPath = path
    revealActiveFileTab()
    if (shouldSelectInExplorer(path, explorer)) treePath = path
    highlightLine = line
    // A stale first-miss timestamp from a previously-open (and possibly
    // different) path must not survive a navigation: refreshOpenFileIfStale
    // only resets on a path match or a successful read, neither of which
    // fires here, so without this a leftover timestamp already older than
    // GONE_CONFIRM_MS could evict *this* path on its first miss.
    openFileNotFoundPath = ''
    openFileNotFoundSince = 0
    maybeClearGoneConfirm()
    selectedIsDir = false
    hydrateFailedPath = ''
    baseDoc = ''
    baseDocPath = ''
    if (st === 'd') {
      // Deleted on disk — only the base side exists. Text files get the merge
      // Diff; rasters stay on the image pane at the previous revision.
      file = null
      draft = ''
      savedContent = ''
      editorPath = ''
      mainEditorDirty = false
      outline = []
      detachLsp()
      ensureOpenTab(path)
      viewMode = mediaPath ? 'preview' : 'diff'
      if (!imagePath && !mediaPath) await loadBaseDoc(path)
      if (gen !== navGen) return
    } else if (wantDiff) {
      // Diff-first (Changed tab / explicit prefer): base + current in parallel.
      // The mode is committed before the awaits — showing the code view for a
      // beat and then flipping was jarring, and a later rapid open must find
      // this file's mode already correct, not the previous file's.
      ensureOpenTab(path)
      if (imagePath) {
        viewMode = (st === 'm' || st === 'd') ? 'diff' : 'preview'
        await hydrateFile(path, line, endLine, startCol, endCol)
      } else if (mediaPath) {
        viewMode = 'preview'
        await hydrateFile(path, line, endLine, startCol, endCol)
      } else {
        viewMode = st ? 'diff' : 'code'
        await Promise.all([
          loadBaseDoc(path),
          hydrateFile(path, line, endLine, startCol, endCol),
        ])
      }
      if (gen !== navGen) return
    } else {
      const cached = tabBuffers[path]
      if (cached) {
        file = cached.file
        draft = cached.draft
        savedContent = cached.saved
        editorPath = path
        selectedIsDir = false
      }
      // The restored mode is known synchronously (fileViews + git status), so
      // commit it before the awaits: a diff tab enters the diff state now and
      // fetches base + current in parallel (serial awaits showed the code
      // view first, then flipped), and rapid tab flips never observe a stale
      // mode on this file.
      const targetMode = viewModeForPath(path)
      viewMode = targetMode
      if (targetMode === 'diff') {
        if (imagePath) {
          await hydrateFile(path, line, endLine, startCol, endCol)
        } else {
          await Promise.all([
            loadBaseDoc(path),
            hydrateFile(path, line, endLine, startCol, endCol),
          ])
        }
      } else {
        await hydrateFile(path, line, endLine, startCol, endCol)
      }
      if (gen !== navGen) return
      // Keep the other pane in sync when both show the same file.
      if (splitOpen && splitPath === path) {
        splitDraft = draft
        splitSaved = savedContent
        splitWritable = !!file?.writable && !isBinaryPreviewMime(file.mime) && file.size <= MAX_EDIT_BYTES
        splitEditorDirty = mainEditorDirty
      }
    }
    // Tree reveal is independent of editor paint — don't block first content.
    syncExplorerToPath(path, explorer)
    // Re-apply after tick so a doc swap $effect can't clobber a range select.
    await tick()
    if (gen !== navGen) return
    if (viewMode === 'code' || (viewMode === 'diff' && line > 0)) {
      restoreCodeView(path, line, endLine, undefined, startCol, endCol)
    }
  }

  async function openInSplit(path: string, line = 0, endLine = 0, explorer: ExplorerReveal = 'reveal', fromDisk = false, startCol = 0, endCol = 0) {
    if (!path) return
    if (workspaceId && editorHidden) setEditorHidden(workspaceId, false)
    if (!fromDisk && path === splitPath) {
      await tick()
      if (line > 0) splitEditor?.scrollToLine(line, endLine, startCol, endCol)
      else splitEditor?.focus()
      if (shouldSelectInExplorer(path, explorer)) treePath = path
      syncExplorerToPath(path, explorer)
      return
    }
    if (fromDisk) dropStash(path)
    else if (path !== splitPath) {
      rememberEditorView(splitPath, splitEditor, fileViews[splitPath]?.viewMode ?? 'code')
      stashSplitBuffer()
    }
    let f: FileContent | null = null
    try {
      f = await readEditorFile(path)
    } catch (err: unknown) {
      if (isDirectoryError(err)) {
        pushToast('info', 'Directories can’t be opened in the split pane')
        return
      }
      if (isWorktreeMissing(err)) {
        worktreeMissing = true
        return
      }
      pushToast('error', `Read failed: ${bridgeErrorMessage(err)}`)
      return
    }
    if (!isAbsFilesystemPath(path)) worktreeMissing = false
    if (!f || isBinaryPreviewMime(f.mime)) {
      pushToast('info', 'Cannot open that file in the split pane')
      return
    }
    splitOpen = true
    splitPath = path
    const buf = tabBuffers[path]
    const keepDirty = !!buf && !editorTextEq(buf.draft, buf.saved)
    splitWritable = !!f.writable && !isBinaryPreviewMime(f.mime) && f.size <= MAX_EDIT_BYTES
    if (keepDirty) {
      splitDraft = buf.draft
      splitSaved = f.content
      splitEditorDirty = true
      putTabBuffer(path, { ...buf, saved: f.content, file: f })
    } else {
      splitDraft = f.content
      splitSaved = f.content
      splitEditorDirty = false
      putTabBuffer(path, { draft: f.content, saved: f.content, file: f })
      overlaySplitStash(path)
    }
    focusedPane = 'split'
    if (shouldSelectInExplorer(path, explorer)) treePath = path
    ensureOpenTab(path)
    syncExplorerToPath(path, explorer)
    // Outline / symbol palette follow the focused pane.
    await tick()
    const o = await resolveFileOutline(
      path,
      'split',
      diskOutline(path),
    )
    if (focusedPane === 'split' && splitPath === path) outline = o ?? []
    prefetchBaseDoc(path)
    if (line > 0) splitEditor?.scrollToLine(line, endLine, startCol, endCol)
    else restoreCodeView(path, 0, 0, splitEditor)
  }

  function focusPane(pane: 'main' | 'split') {
    focusedPane = pane
    if (pane === 'split') {
      splitEditor?.focus()
      if (splitPath) {
        void resolveFileOutline(
          splitPath,
          'split',
          diskOutline(splitPath),
        ).then((o) => {
          if (focusedPane === 'split' && o) outline = o
        })
      }
    } else {
      editor?.focus()
      if (selectedPath && file && !isBinaryPreviewMime(file.mime)) {
        void resolveFileOutline(
          selectedPath,
          'main',
          diskOutline(selectedPath),
        ).then((o) => {
          if (focusedPane === 'main' && o) outline = o
        })
      }
    }
  }

  /** Map a focus target to the tab-underline zone (editor / terminal / agent / other). */
  function onObsFocusIn(e: FocusEvent) {
    const el = e.target
    if (!(el instanceof Element)) {
      focusZone = 'other'
      return
    }
    if (el.closest('.term-dock')) {
      focusZone = 'terminal'
      return
    }
    // File explorer + source control are part of the code workbench — keep
    // file tabs lit while opening/navigating files. Must run before .obs-left
    // so these panels on the left aren't treated as the agent dock.
    if (el.closest('.obs-files, .obs-files-panel, .obs-scm-panel')) {
      focusZone = 'editor'
      return
    }
    // Agent/Maestro dock (chat tabs live in the side-head; composer in the body).
    if (el.closest('.obs-left')) {
      focusZone = 'agent'
      return
    }
    // File tabs sit in the toolbar, outside .obs-main, but they belong to the editor.
    if (el.closest('.obs-main') || el.closest('.obs-tb-center')) {
      focusZone = 'editor'
      return
    }
    focusZone = 'other'
  }

  // Chat transcript / empty chrome aren't focusable, so a click there never
  // fires focusin. Park on the aside (same pattern as empty .obs-main) so the
  // agent zone lights up — without stealing into the composer, which would
  // break selecting message text.
  function onLeftMouseDown(e: MouseEvent) {
    if (e.button !== 0) return
    const t = e.target
    if (!(t instanceof Element)) return
    if (
      t.closest(
        'textarea, input, select, button, a, [role="button"], [role="tab"], [contenteditable="true"]',
      )
    ) {
      return
    }
    leftAsideEl?.focus({ preventScroll: true })
  }

  function clearTermMax() {
    if (workspaceId) setTerminalDockMaximized(workspaceId, false)
  }

  // ⌘1 / ⌘2 / ⌘3 — jump to agent, editor, terminal (VS Code/Sublime-style numbered
  // regions). Always act even while typing in a composer or shell: these are
  // workbench chords, not text input.
  /** Move keys into an already-visible agent composer. Does not open the dock
   *  or switch leftKind — those are ⌘1 (`focusAgentPanel`), not restore. */
  function focusAgentComposer(token = rootToken) {
    if (rootToken !== token) return
    const scope = portalAgentToPR
      ? document.querySelector<HTMLElement>('#prd-agent')
      : (obsEl ?? leftAsideEl)
    if (!scope) return
    const composer = scope.querySelector<HTMLElement>('.chat-in .chat-composer')
    if (composer) { composer.focus({ preventScroll: true }); return }
    const ta = scope.querySelector<HTMLTextAreaElement>('textarea:not([disabled])')
    if (ta) { ta.focus(); return }
    scope.querySelector<HTMLElement>('[tabindex="0"], input, button')?.focus()
  }

  /** Bring the agent transcript panel on screen without stealing focus (deep
   *  links that land on a card, not the composer). */
  export function showAgentPanel() {
    if (portalAgentToPR) {
      clearTermMax()
      setPRAgentDockOpen(true)
      return
    }
    clearTermMax()
    void selectCodePanel('maestro')
  }

  async function focusAgentPanel() {
    const token = rootToken
    if (portalAgentToPR) {
      clearTermMax()
      setPRAgentDockOpen(true)
    } else if (!hasLeft || !leftPanels.some((p) => p.key === 'maestro')) {
      return
    } else {
      clearTermMax()
      leftKind = 'maestro'
      setLeftGutterOpen(true)
    }
    await tick()
    if (rootToken !== token) return
    // Snippet remount (files → agent) needs a second frame before the composer exists.
    await tick()
    if (rootToken !== token) return
    focusAgentComposer(token)
  }

  /**
   * ⌘L — attach the active editor file (or selected span) as an @file chip
   * in the agent composer. Whole file when the caret is collapsed; `@path:a-b`
   * for whole-line selections; `@path:line:col-line:col` when the highlight is
   * mid-line. Diff mode reads the merge view (current worktree lines).
   */
  async function addEditorMentionToChat() {
    if (!onAddEditorMention) return
    if (!hasLeft || !leftPanels.some((p) => p.key === 'maestro')) return
    // Split pane unmounts for main-pane diff/image while splitOpen/splitPath/
    // focusedPane can still say "split" — only treat split as active when its
    // editor is mounted so ⌘L attaches the file the user is actually viewing.
    const useSplit = focusedPane === 'split' && !!splitEditor
    const path = useSplit && splitPath ? splitPath : selectedPath
    if (!path || isScratchPath(path)) return
    const focused = showDiff ? mergeEditor : useSplit ? splitEditor : editor
    const sel = focused?.getMentionSelection?.() ?? null
    await focusAgentPanel()
    if (sel) onAddEditorMention(path, sel.startLine, sel.endLine, sel.startCol, sel.endCol)
    else onAddEditorMention(path)
  }

  function focusCodePanel() {
    const token = rootToken
    clearTermMax()
    focusPane(splitOpen && focusedPane === 'split' ? 'split' : 'main')
    // Empty editor (no file open) has nothing for CodeMirror to focus — park on
    // the main column so the terminal/agent lose lit and ⌘2 still selects code.
    queueMicrotask(() => {
      if (rootToken !== token) return
      const ae = document.activeElement
      if (ae instanceof Element && obsEl?.contains(ae) && ae.closest('.obs-main') && !ae.closest('.term-dock')) return
      focusZone = 'editor'
      obsEl?.querySelector<HTMLElement>('.obs-main')?.focus({ preventScroll: true })
    })
  }

  async function focusTerminalPanel(token = rootToken) {
    const id = workspaceId
    if (!id) return
    setTerminalDockOpen(id, true)
    await tick()
    if (rootToken !== token || workspaceId !== id) return
    // Dock may still be spawning its first pane; retry briefly so ⌘3 lands in the
    // shell rather than on the empty chrome.
    for (let i = 0; i < 8; i++) {
      if (rootToken !== token || workspaceId !== id) return
      termDock?.focus()
      if (document.activeElement?.closest('.term-dock')) return
      await new Promise((r) => setTimeout(r, 40))
    }
  }

  /** Put keys back on the chrome this workspace last had (A→B→A). Only moves
   *  focus — does not open a collapsed dock or switch leftKind. When the editor
   *  is hidden the middle pane is the dock, so any fallback that would park on
   *  `.obs-main` goes to the shell instead (⌥⌘E leaves `focusZone` as editor). */
  function restoreChromeFocus(zone: ObservatoryFocusZone, token = rootToken) {
    if (mainBlocked || rootToken !== token) return
    const codeOrTerminal = () => {
      if (editorHidden && termOpen && workspaceId) {
        void focusTerminalPanel(token)
        return
      }
      focusCodePanel()
    }
    if (zone === 'terminal') {
      if (termOpen && workspaceId) {
        void focusTerminalPanel(token)
        return
      }
      codeOrTerminal()
      return
    }
    if (zone === 'agent') {
      if (maestroPanelVisible) {
        void tick().then(() => focusAgentComposer(token))
        return
      }
      codeOrTerminal()
      return
    }
    if (editorHidden || viewMode !== 'code') {
      codeOrTerminal()
      return
    }
    const target = focusedPane === 'split' && splitOpen ? splitEditor : editor
    if (!target) {
      codeOrTerminal()
      return
    }
    target.focus()
    if (target.hasFocus()) focusZone = 'editor'
  }

  // ⌃` / the toolbar toggle open the dock on this workspace. Focus the shell
  // then — TerminalPane no longer autofocuses on remount, so a workspace
  // switch cannot steal keys, but a user-initiated open still has to land in
  // the PTY. Skip the rising edge when workspaceId changed: that's restore
  // chrome, not a toggle.
  $effect(() => {
    const id = workspaceId
    const open = termOpen
    untrack(() => {
      if (id !== termOpenForWs) {
        termOpenForWs = id
        termWasOpen = open
        return
      }
      if (open && !termWasOpen && !mainBlocked) void focusTerminalPanel()
      termWasOpen = open
    })
  })

  function onMainDocChange(v: string) {
    draft = v
    const bufPath = editorPath || selectedPath
    if (bufPath && file) {
      putTabBuffer(bufPath, { draft: v, saved: savedContent, file })
    }
    if (splitOpen && splitPath === selectedPath) {
      splitDraft = v
    }
    scheduleLspOutline()
  }

  function onSplitDocChange(v: string) {
    splitDraft = v
    if (splitPath === selectedPath) {
      draft = v
    }
    const bufFile = (splitPath === selectedPath && file)
      ? file
      : tabBuffers[splitPath]?.file
    if (splitPath && bufFile) {
      const saved = splitPath === selectedPath ? savedContent : (tabBuffers[splitPath]?.saved ?? splitSaved)
      putTabBuffer(splitPath, { draft: v, saved, file: bufFile })
    }
  }

  async function resolveFileOutline(
    path: string,
    pane: 'main' | 'split',
    disk: Promise<OutlineSymbol[] | null | undefined>,
  ): Promise<OutlineSymbol[] | null> {
    const tm = await outlineOpenFile(tmDocIdFor(pane, path), (syms) => {
      if (pane === 'split') {
        if (focusedPane === 'split' && splitPath === path) outline = syms
        return
      }
      // Prefer language-server documentSymbol once attach succeeds.
      if (lspAttach && lspHasDocumentSymbol(lspAttach.client)) return
      if (selectedPath === path && editorPath === path) outline = syms
    }, 120)
    if (tm) return tm
    const fromDisk = await disk
    return fromDisk ?? null
  }

  /** Read + outline for the open path (may run after Diff is already showing). */
  async function hydrateFile(path: string, line = 0, endLine = 0, startCol = 0, endCol = 0): Promise<boolean> {
    if (isScratchPath(path)) {
      const scratch = scratchDocs[path]
      const content = scratch?.content ?? ''
      const mime = scratch?.mime || 'text/plain'
      const binaryPreview = isBinaryPreviewMime(mime)
      file = {
        path,
        content,
        size: content.length,
        writable: !binaryPreview,
        language: scratch?.lang || 'text',
        mime,
      }
      draft = content
      savedContent = content
      editorPath = path
      putTabBuffer(path, { draft: content, saved: content, file })
      mainEditorDirty = false
      outline = []
      selectedIsDir = false
      ensureOpenTab(path)
      if (line > 0 && (viewMode === 'code' || showDiff) && !binaryPreview) scrollToLine(line, endLine, startCol, endCol)
      void tick().then(() => refreshLsp(path))
      return true
    }
    const gen = ++fileLoadGen
    fileLoadAbort?.abort()
    const ac = new AbortController()
    fileLoadAbort = ac
    const hadBuffer = !!tabBuffers[path]
    setFileLoading(!hadBuffer)
    // Keep the previous buffer on screen while this path loads so CodeMirror
    // is not torn down (that reset scroll/caret and broke click mapping).
    // Read and outline are independent — overlapping the RPCs cuts a round-trip
    // off the critical path to first editor paint. But extensionless leaves are
    // often DIRECTORY stubs (git submodules in Changed), and a prefetched
    // Outline for one is a wasted, erroring RPC — those wait for the read to
    // succeed instead. Directory leaves must not toast a cryptic read failure.
    const likelyFile = /\.[^/.]+$/.test(path)
    const outlineP = likelyFile && !isAbsFilesystemPath(path) ? quiet(CodeService.Outline(projectId, workspaceId, path)) : null
    try {
      const f = await readEditorFile(path, ac.signal)
      if (gen !== fileLoadGen || path !== selectedPath) return false
      if (!isAbsFilesystemPath(path)) worktreeMissing = false
      selectedIsDir = false
      const buf = tabBuffers[path]
      const keepDirty = !!buf && !editorTextEq(buf.draft, buf.saved)
      file = f
      if (keepDirty) {
        // Warn before overwriting buf.saved — that's the disk text the draft
        // diverged from, and the only handle checkStashDiskMoved has.
        checkStashDiskMoved(path, { draft: buf.draft, saved: buf.saved }, f.content)
        draft = buf.draft
        savedContent = f.content
        mainEditorDirty = true
        putTabBuffer(path, { ...buf, saved: f.content, file: f })
      } else if (!editorTextEq(draft, f.content)) {
        draft = f.content
        savedContent = f.content
        mainEditorDirty = false
        putTabBuffer(path, { draft: f.content, saved: f.content, file: f })
      } else {
        savedContent = f.content
        mainEditorDirty = false
        putTabBuffer(path, { draft, saved: f.content, file: f })
      }
      editorPath = path
      hydrateFailedPath = ''
      ensureOpenTab(path)
      if (isBinaryPreviewMime(f.mime)) {
        outline = []
        void tick().then(() => refreshLsp(path))
        return true
      }
      prefetchBaseDoc(path)
      if (!keepDirty) overlayStash(path)
      await tick()
      const diskOutline = outlineP ?? (isAbsFilesystemPath(path)
        ? Promise.resolve(null)
        : quiet(CodeService.Outline(projectId, workspaceId, path)))
      const tmOrDisk = await resolveFileOutline(path, 'main', diskOutline)
      if (gen !== fileLoadGen || path !== selectedPath) return false
      outline = tmOrDisk ?? []
      if (line > 0 && (viewMode === 'code' || showDiff)) scrollToLine(line, endLine, startCol, endCol)
      void tick().then(() => refreshLsp(path))
      return true
    } catch (err: unknown) {
      if (ac.signal.aborted || gen !== fileLoadGen || path !== selectedPath) return false
      outline = []
      // Git submodule / directory leaves show up in Changed as fake files.
      // Keep Diff usable; don't follow up with Outline or a cryptic read toast.
      if (isDirectoryError(err)) {
        selectedIsDir = true
        file = null
        draft = ''
        savedContent = ''
        editorPath = ''
        hydrateFailedPath = ''
        detachLsp()
        return false
      }
      if (isWorktreeMissing(err)) {
        worktreeMissing = true
        selectedIsDir = false
        file = null
        draft = ''
        savedContent = ''
        editorPath = ''
        hydrateFailedPath = path
        detachLsp()
        return false
      }
      selectedIsDir = false
      file = null
      draft = ''
      savedContent = ''
      editorPath = ''
      hydrateFailedPath = path
      detachLsp()
      pushToast('error', `Read failed: ${bridgeErrorMessage(err)}`)
      return false
    } finally {
      if (gen === fileLoadGen) setFileLoading(false)
    }
  }

  async function readEditorFile(path: string, signal?: AbortSignal): Promise<FileContent> {
    if (isAbsFilesystemPath(path)) {
      return readExternalEditorFile(path, signal)
    }
    const f = await CodeService.ReadFile(projectId, workspaceId, path)
    if (fileNeedsHTTPBody(f)) {
      f.content = await fetchWorkspaceFileText(projectId, workspaceId, path, signal)
    }
    return f
  }

  function scrollToLine(line: number, endLine = 0, startCol = 0, endCol = 0) {
    if (!line) return
    highlightLine = line
    if (showDiff) {
      mergeEditor?.scrollToLine(line, endLine, startCol, endCol)
      return
    }
    if (focusedPane === 'split' && splitOpen) splitEditor?.scrollToLine(line, endLine, startCol, endCol)
    else editor?.scrollToLine(line, endLine, startCol, endCol)
  }

  function searchOpts(offset = 0): SearchOpts {
    return new SearchOpts({
      regex: searchRegex,
      caseSensitive: searchCase,
      glob: searchGlob.trim(),
      limit: 200,
      offset,
    })
  }

  async function runSearch(reset = true) {
    const q = query.trim()
    if (!q) return
    await selectCodePanel('search')
    if (reset) searchOffset = 0
    const res = await call(
      CodeService.SearchAdvanced(projectId, workspaceId, q, searchOpts(searchOffset)),
      'Search failed',
    )
    if (res) {
      const page = res.hits ?? []
      hits = reset ? page : [...hits, ...page]
      if (page.length) searchOffset = hits.length
      // The walk skips files over the per-file size cap; saying so beats a
      // silently incomplete result set.
      searchSkippedLarge = reset
        ? (res.skippedLarge ?? 0)
        : searchSkippedLarge + (res.skippedLarge ?? 0)
      searchTruncated = !!res.truncated
    }
  }

  async function runReplace() {
    const q = query.trim()
    if (!q || !treeWritable) return
    if (!(await askConfirm(
      'Replace in files',
      `Replace all matches of “${q}” across the worktree? This cannot be undone (except via Discard/git).`,
      'Replace all',
    ))) return
    const res = await call(
      CodeService.ReplaceInFiles(projectId, workspaceId, q, replaceText, searchOpts(0)),
      'Replace failed',
    )
    if (!res) return
    pushToast('success', `Replaced ${res.replacements} in ${res.filesChanged} file(s)`)
    await softRefresh([''])
    await runSearch(true)
    if (selectedPath) await openPath(selectedPath, highlightLine, false, 0, 'reveal', true)
  }

  /** Attach @codemirror/lsp-client for the open file; soft-degrades when no server. */
  async function refreshLsp(path: string) {
    const gen = ++lspAttachGen
    const ws = workspaceId
    const proj = projectId
    lspAttach = null
    editor?.setLspExtension([])
    if (!path || isScratchPath(path) || !workspaceId) return
    if (!writable) return
    try {
      const attach = await attachLsp(proj, ws, path, () =>
        gen === lspAttachGen && workspaceId === ws && selectedPath === path && writable,
      )
      if (gen !== lspAttachGen || path !== selectedPath || workspaceId !== ws || !writable) return
      lspAttach = attach
      editor?.setLspExtension(attach?.extension ?? [])
      if (!attach) return
      await attach.client.initializing.catch(() => null)
      if (gen !== lspAttachGen || path !== selectedPath) return
      const syms = await lspOutline(attach.client, attach.fileUri)
      if (path === selectedPath && gen === lspAttachGen && syms?.length) outline = syms
    } catch (err) {
      console.warn('[lsp] refresh failed', err)
      if (gen === lspAttachGen) {
        lspAttach = null
        editor?.setLspExtension([])
      }
    }
  }

  function scheduleLspOutline() {
    if (!lspAttach || !lspHasDocumentSymbol(lspAttach.client)) return
    window.clearTimeout(lspOutlineTimer)
    const attach = lspAttach
    const path = selectedPath
    lspOutlineTimer = window.setTimeout(() => {
      if (lspAttach !== attach || selectedPath !== path) return
      void lspOutline(attach.client, attach.fileUri).then((syms) => {
        if (lspAttach === attach && selectedPath === path && syms) outline = syms
      })
    }, 300)
  }

  async function findRefs(name: string, line = 0) {
    await selectCodePanel('refs')
    const panePath = focusedPane === 'split' && splitOpen && splitPath ? splitPath : selectedPath
    const attachPath = lspAttach ? uriToRelPath(lspAttach.rootUri, lspAttach.fileUri) : ''
    if (lspAttach && line > 0 && panePath === attachPath) {
      let character = 0
      const view = focusedPane === 'split' ? splitEditor?.getView() : editor?.getView()
      const doc = view?.state.doc
      if (doc && line <= doc.lines) {
        const idx = doc.line(line).text.indexOf(name)
        character = idx >= 0 ? idx : 0
      }
      const lspRefs = await lspReferences(
        lspAttach.client, lspAttach.fileUri, lspAttach.rootUri, line - 1, character,
      )
      if (lspRefs?.length) {
        refs = lspRefs
        return
      }
    }
    const r = await call(CodeService.FindRefs(projectId, workspaceId, name), 'Find refs failed')
    if (r) refs = r
  }

  async function applyProblemFix(problem: LspProblem) {
    if (problem.workspaceId && problem.workspaceId !== workspaceId) return
    problemFixBusy = `${problem.uri}:${problem.line}:${problem.message}`
    try {
      if (await applyLspQuickFix(problem, lspAttach?.client) && selectedPath && editor) {
        draft = editor.getDoc()
      }
    } finally {
      problemFixBusy = ''
    }
  }

  let prevDisabledLS = '\x00'
  $effect(() => {
    const key = ($settings?.disabledLanguageServers ?? []).slice().sort().join('\0')
    if (key === prevDisabledLS) return
    const skip = prevDisabledLS === '\x00'
    prevDisabledLS = key
    if (skip) return
    const path = untrack(() => selectedPath)
    if (path) void refreshLsp(path)
  })

  async function gotoDef(name: string) {
    const n = name.trim()
    if (!n) return
    const defs = await call(CodeService.FindDefs(projectId, workspaceId, n), 'Find definition failed')
    if (!defs?.length) {
      pushToast('info', `No definition for “${n}”`)
      return
    }
    if (defs.length === 1) {
      await openPath(defs[0].path, defs[0].line)
      return
    }
    await selectCodePanel('refs')
    refs = defs.map((d) => ({ path: d.path, line: d.line, text: `${d.kind} ${d.name}`, def: true }))
    pushToast('info', `${defs.length} definitions for “${n}”`)
  }

  function openQuickOpen(mode: 'file' | 'symbol' = 'file') {
    quickOpenMode = mode
    quickOpen = true
  }

  type SearchableEditor = {
    toggleSearch: () => boolean
    isSearchOpen: () => boolean
  }

  function focusedSearchableEditor(): SearchableEditor | undefined {
    if (showDiff) return mergeEditor
    if (focusedPane === 'split' && splitOpen && splitEditor) return splitEditor
    if (showPreview) return filePreview
    return editor
  }

  const canToggleFind = $derived.by(() => {
    if (showImagePane || showPdfPane || showVideoPane) return false
    if (showDiff) return !!selectedPath
    if (focusedPane === 'split' && splitOpen) return !!splitPath
    if (showPreview && previewKind === 'svg') return false
    return !!selectedPath
  })

  function syncFindPanelOpen() {
    findPanelOpen = focusedSearchableEditor()?.isSearchOpen() ?? false
  }

  function toggleFindPanel() {
    focusedSearchableEditor()?.toggleSearch()
    syncFindPanelOpen()
  }

  // Keep the crumb-bar find toggle in sync when focus moves between panes/modes.
  $effect(() => {
    void focusedPane
    void splitOpen
    void showDiff
    void editor
    void splitEditor
    void mergeEditor
    void filePreview
    void showPreview
    syncFindPanelOpen()
  })

  async function humanCommit() {
    if (!isGit || !treeWritable || commitBusy) return
    const msg = commitMsg.trim()
    if (!msg) {
      pushToast('info', 'Enter a commit message')
      return
    }
    if (stagedFiles.length === 0) {
      pushToast('info', 'Stage changes to commit')
      return
    }
    commitBusy = true
    try {
      const res = await call(CodeService.CommitStaged(projectId, workspaceId, msg), 'Commit failed')
      if (!res) return
      if (!res.committed) {
        pushToast('info', 'Nothing to commit')
        return
      }
      commitMsg = ''
      pushToast('success', `Committed ${res.hash} (+${res.additions}/−${res.deletions})`)
      await softRefresh([''])
    } finally {
      commitBusy = false
    }
  }

  async function generateCommitMsg() {
    if (!isGit || !treeWritable || commitMsgBusy) return
    if (stagedFiles.length === 0) {
      pushToast('info', 'Stage changes to generate a commit message')
      return
    }
    commitMsgBusy = true
    try {
      const msg = await call(
        CodeService.GenerateCommitMessage(projectId, workspaceId),
        'Generate commit message failed',
      )
      if (msg == null) return
      const trimmed = msg.trim()
      if (!trimmed) {
        pushToast('info', 'No commit message generated')
        return
      }
      commitMsg = trimmed
    } finally {
      commitMsgBusy = false
    }
  }

  async function discardPaths(paths: string[], label: string) {
    if (!isGit || !treeWritable || !paths.length) return
    const msg = paths.length === 1
      ? `Discard uncommitted changes to “${paths[0]}”?`
      : `Discard uncommitted changes to ${paths.length} files in “${label}”?`
    if (!(await askConfirm('Discard changes', msg, 'Discard'))) return
    for (const p of paths) {
      if (!(await callOk(CodeService.DiscardFile(projectId, workspaceId, p), 'Discard failed'))) return
    }
    pushToast('success', paths.length === 1 ? 'Discarded ' + paths[0] : `Discarded ${paths.length} files`)
    await softRefresh([''])
    if (paths.includes(selectedPath)) await openPath(selectedPath, 0, false, 0, 'reveal', true)
  }

  async function discardPath(path: string) {
    await discardPaths([path], path)
  }

  async function discardAllWorking() {
    if (!isGit || !treeWritable || workingDirtyCount === 0) return
    if (!(await askConfirm(
      'Discard all',
      `Discard all ${workingDirtyCount} uncommitted change(s) in this worktree?`,
      'Discard all',
    ))) return
    if (!(await callOk(CodeService.DiscardAll(projectId, workspaceId), 'Discard all failed'))) return
    pushToast('success', 'Discarded all uncommitted changes')
    await softRefresh([''])
    if (selectedPath) await openPath(selectedPath, 0, false, 0, 'reveal', true)
  }

  async function stagePaths(paths: string[]) {
    if (!isGit || !treeWritable || !paths.length) return
    if (!(await callOk(CodeService.Stage(projectId, workspaceId, paths), 'Stage failed'))) return
    await refreshWorkingStatus()
  }

  async function unstagePaths(paths: string[]) {
    if (!isGit || !treeWritable || !paths.length) return
    if (!(await callOk(CodeService.Unstage(projectId, workspaceId, paths), 'Unstage failed'))) return
    await refreshWorkingStatus()
  }

  async function stagePath(path: string) {
    await stagePaths([path])
  }

  async function unstagePath(path: string) {
    await unstagePaths([path])
  }

  async function stageAll() {
    await stagePaths(unstagedPaths)
  }

  async function unstageAll() {
    await unstagePaths(stagedPaths)
  }

  function dropEditorBuffer(path: string) {
    if (!path) return
    if (tabBuffers[path]) {
      const next = { ...tabBuffers }
      delete next[path]
      tabBuffers = next
    }
    editor?.forgetPath(path)
    splitEditor?.forgetPath(path)
    forgetEditorHistoryTree(editorSessionKey, path)
    markTabDirty(path, false)
  }

  async function reloadFromDisk() {
    if (focusedPane === 'split' && splitOpen && splitPath) {
      if (splitDirty && !(await askConfirm('Reload', 'Reload split pane from disk and discard unsaved edits?', 'Reload'))) return
      dropEditorBuffer(splitPath)
      const keepFocus = focusedPane
      focusedPane = 'split'
      await openInSplit(splitPath, cursorLine, 0, 'reveal', true)
      focusedPane = keepFocus
      pushToast('info', 'Reloaded from disk')
      return
    }
    if (!selectedPath || selectedGitStatus === 'd') return
    if (dirty && !(await askConfirm('Reload', 'Reload from disk and discard unsaved editor buffer?', 'Reload'))) return
    const artifactId = artifactIdFromScratchPath(selectedPath)
    if (artifactId) {
      const list = await call(ArtifactsService.ListArtifacts(workspaceId), 'Could not list artifacts')
      if (!list) return
      const a = list.find((x) => x.id === artifactId)
      if (!a) {
        pushToast('error', 'Artifact not found')
        return
      }
      dropEditorBuffer(selectedPath)
      if (await openArtifact(a)) pushToast('info', 'Reloaded from disk')
      return
    }
    dropEditorBuffer(selectedPath)
    await openPath(selectedPath, cursorLine, false, 0, 'reveal', true)
    pushToast('info', 'Reloaded from disk')
  }

  function closeSplit() {
    splitOpen = false
    splitPath = ''
    splitDraft = ''
    splitSaved = ''
    splitWritable = false
    focusedPane = 'main'
    editor?.focus()
  }

  async function toggleSplit() {
    if (splitOpen) {
      stashSplitBuffer()
      closeSplit()
      return
    }
    if (!selectedPath || !file || isBinaryPreviewMime(file.mime) || showDiff) {
      pushToast('info', 'Open a file in Code view to split')
      return
    }
    // Clone current file into the new pane (both editable); keep focus on main.
    splitOpen = true
    splitPath = selectedPath
    splitDraft = draft
    splitSaved = savedContent
    splitWritable = writable
    focusedPane = 'main'
  }

  /** Open path in the split pane (creates the split if needed) and focus it. */
  async function loadSplit(path: string) {
    focusedPane = 'split'
    splitOpen = true
    await openInSplit(path)
  }

  function clampSplitPct(n: number): number {
    return Math.min(80, Math.max(20, Math.round(n)))
  }
  function writeSplitPct() {
    try { localStorage.setItem('recur.editorSplitPct', String(splitPct)) } catch { /* ignore */ }
  }
  /** Discrete adjustment (keyboard) — move and persist in one go. */
  function persistSplitPct(n: number) {
    splitPct = clampSplitPct(n)
    writeSplitPct()
  }
  function onSplitResizeDown(e: PointerEvent) {
    if (e.button !== 0) return
    e.preventDefault()
    e.stopPropagation()
    const el = e.currentTarget as HTMLElement
    const row = el.closest('.obs-editors') as HTMLElement | null
    if (!row) return
    el.setPointerCapture(e.pointerId)
    splitResizing = true
    document.body.classList.add('resizing-panels')
    const rect = row.getBoundingClientRect()
    // A pointermove can fire several times per frame, and each one relaid out
    // both CodeMirror panes and hit localStorage synchronously. Coalesce to one
    // update per frame and persist once, when the drag ends.
    let pendingX = 0
    let raf = 0
    const apply = () => {
      raf = 0
      splitPct = clampSplitPct(((pendingX - rect.left) / rect.width) * 100)
    }
    const move = (ev: PointerEvent) => {
      if (rect.width <= 0) return
      pendingX = ev.clientX
      if (!raf) raf = requestAnimationFrame(apply)
    }
    const up = (ev: PointerEvent) => {
      try { el.releasePointerCapture(ev.pointerId) } catch { /* already released */ }
      el.removeEventListener('pointermove', move)
      el.removeEventListener('pointerup', up)
      el.removeEventListener('pointercancel', up)
      if (raf) {
        cancelAnimationFrame(raf)
        apply()
      }
      splitResizing = false
      document.body.classList.remove('resizing-panels')
      writeSplitPct()
    }
    el.addEventListener('pointermove', move)
    el.addEventListener('pointerup', up)
    el.addEventListener('pointercancel', up)
  }
  function onSplitResizeKey(e: KeyboardEvent) {
    const step = e.shiftKey ? 5 : 2
    if (e.key === 'ArrowLeft') { e.preventDefault(); persistSplitPct(splitPct - step) }
    else if (e.key === 'ArrowRight') { e.preventDefault(); persistSplitPct(splitPct + step) }
  }

  function persistTermBesidePct(n: number) {
    if (!workspaceId) return
    setTerminalBesidePct(workspaceId, n, true)
  }
  function onTermBesideResizeDown(e: PointerEvent) {
    if (e.button !== 0) return
    e.preventDefault()
    e.stopPropagation()
    const el = e.currentTarget as HTMLElement
    const row = el.closest('.obs-main') as HTMLElement | null
    if (!row || !workspaceId) return
    el.setPointerCapture(e.pointerId)
    termBesideResizing = true
    document.body.classList.add('resizing-panels')
    const rect = row.getBoundingClientRect()
    const id = workspaceId
    let pendingX = 0
    let lastPct = termBesidePct
    let raf = 0
    const apply = () => {
      raf = 0
      lastPct = clampSplitPct(((pendingX - rect.left) / rect.width) * 100)
      setTerminalBesidePct(id, lastPct, false)
    }
    const move = (ev: PointerEvent) => {
      if (rect.width <= 0) return
      pendingX = ev.clientX
      if (!raf) raf = requestAnimationFrame(apply)
    }
    const up = (ev: PointerEvent) => {
      try { el.releasePointerCapture(ev.pointerId) } catch { /* already released */ }
      el.removeEventListener('pointermove', move)
      el.removeEventListener('pointerup', up)
      el.removeEventListener('pointercancel', up)
      if (raf) {
        cancelAnimationFrame(raf)
        apply()
      }
      termBesideResizing = false
      document.body.classList.remove('resizing-panels')
      setTerminalBesidePct(id, lastPct, true)
    }
    el.addEventListener('pointermove', move)
    el.addEventListener('pointerup', up)
    el.addEventListener('pointercancel', up)
  }
  function onTermBesideResizeKey(e: KeyboardEvent) {
    const step = e.shiftKey ? 5 : 2
    if (e.key === 'ArrowLeft') { e.preventDefault(); persistTermBesidePct(termBesidePct - step) }
    else if (e.key === 'ArrowRight') { e.preventDefault(); persistTermBesidePct(termBesidePct + step) }
  }

  async function loadFindings(): Promise<Finding[]> {
    if (!workspaceId) {
      findings = []
      return []
    }
    const f = await call(ObservatoryService.ListFindings(workspaceId), 'Findings failed')
    if (f) findings = f
    return findings
  }

  async function addReviewFinding(path: string, line: number, body: string, endLine = 0): Promise<boolean> {
    if (!workspaceId) return false
    const trimmed = body.trim()
    if (!trimmed || !path.trim()) return false
    try {
      await ObservatoryService.CreateFinding(workspaceId, path, line, trimmed, endLine)
      void loadFindings()
      return true
    } catch (err) {
      pushToast('error', bridgeErrorMessage(err) || 'Could not save finding')
      return false
    }
  }

  async function setFindingStatus(f: Finding, status: string) {
    if (!workspaceId || findingBusyId) return
    const prev = f.status
    findingBusyId = f.id
    // Optimistic flip so the filter/list reacts immediately.
    findings = findings.map((x) => (x.id === f.id ? { ...x, status } : x))
    try {
      const updated = await ObservatoryService.SetFindingStatus(workspaceId, f.id, status)
      findings = findings.map((x) => (x.id === f.id ? updated : x))
    } catch (err) {
      findings = findings.map((x) => (x.id === f.id ? { ...x, status: prev } : x))
      pushToast('error', bridgeErrorMessage(err) || 'Could not update finding')
    } finally {
      findingBusyId = ''
    }
  }

  async function addFindingNote(f: Finding, body: string) {
    if (!workspaceId || findingBusyId) return
    const trimmed = body.trim()
    if (!trimmed) return
    findingBusyId = f.id
    const prevNotes = f.notes ?? []
    findings = findings.map((x) =>
      x.id === f.id
        ? {
            ...x,
            notes: [
              ...prevNotes,
              { id: 'pending', author: 'human', body: trimmed, createdAt: new Date().toISOString() },
            ],
          }
        : x,
    )
    try {
      const updated = await ObservatoryService.AddFindingNote(workspaceId, f.id, trimmed)
      findings = findings.map((x) => (x.id === f.id ? updated : x))
    } catch (err) {
      findings = findings.map((x) => (x.id === f.id ? { ...x, notes: prevNotes } : x))
      pushToast('error', bridgeErrorMessage(err) || 'Could not add note')
    } finally {
      findingBusyId = ''
    }
  }

  async function deleteFindingNote(f: Finding, noteId: string) {
    if (!workspaceId || findingBusyId || !noteId) return
    findingBusyId = f.id
    const prevNotes = f.notes ?? []
    findings = findings.map((x) =>
      x.id === f.id ? { ...x, notes: prevNotes.filter((n) => n.id !== noteId) } : x,
    )
    try {
      const updated = await ObservatoryService.DeleteFindingNote(workspaceId, f.id, noteId)
      findings = findings.map((x) => (x.id === f.id ? updated : x))
    } catch (err) {
      findings = findings.map((x) => (x.id === f.id ? { ...x, notes: prevNotes } : x))
      pushToast('error', bridgeErrorMessage(err) || 'Could not delete note')
    } finally {
      findingBusyId = ''
    }
  }

  /** Run willSave commands over `body`; on failure keep the original. */
  async function applyWillSaveTransforms(path: string, body: string): Promise<string> {
    if (isScratchPath(path)) return body
    const scope = scopeForPath(path)
    let cmds: RegisteredCommand[]
    try {
      cmds = await commandsForTrigger('willSave', scope, path)
    } catch {
      return body
    }
    if (!cmds.length) return body
    let next = body
    const deadline = Date.now() + 15_000
    const indent = resolveEditorIndent($editorPrefs, languageIdForPath(path))
    const softTabs = indent.startsWith('spaces')
    const tabSize = indent === 'spaces-2' ? 2 : indent === 'spaces-4' ? 4 : $editorPrefs.tabSize || 4
    const root = workspaceRoot()
    for (const cmd of cmds) {
      const remaining = deadline - Date.now()
      if (remaining <= 0) {
        pushToast('error', 'Save transforms timed out — writing original')
        return body
      }
      try {
        const ctx = {
          selectedText: '',
          document: next,
          currentLine: '',
          lineNumber: 1,
          column: 1,
          currentWord: '',
          scope,
          tabSize,
          softTabs,
          path,
          projectDirectory: root,
          workDir: root,
          timeoutMs: Math.min(remaining, 12_000),
        }
        // Force document input for willSave.
        const res = await runCommand(cmd.id, { ...ctx, selectedText: '' }, {})
        if (res.exitCode !== 0) {
          pushToast('error', (res.stderr || `${cmd.label} failed`).trim())
          return body
        }
        if (res.output === 'replaceDocument' || res.output === 'replaceInput') {
          const out = res.stdout ?? ''
          if (!out.trim()) {
            pushToast('error', `${cmd.label} returned empty — keeping original`)
            return body
          }
          if (next.length > 100 && out.length < next.length * 0.1) {
            pushToast('error', `${cmd.label} shrank output too much — keeping original`)
            return body
          }
          next = out
        }
      } catch (err) {
        pushToast('error', bridgeErrorMessage(err) || `${cmd.label} failed`)
        return body
      }
    }
    return next
  }

  async function saveEdit() {
    // Save the focused pane (split or main).
    if (focusedPane === 'split' && splitOpen && splitPath) {
      if (!splitWritable) return
      if (isScratchPath(splitPath)) {
        const body = splitEditor?.getDoc() ?? splitDraft
        scratchDocs = {
          ...scratchDocs,
          [splitPath]: { ...(scratchDocs[splitPath] ?? { title: 'Scratch', lang: 'txt' }), content: body },
        }
        splitDraft = body
        splitSaved = body
        splitEditor?.markSaved()
        markTabDirty(splitPath, false)
        if (splitPath === selectedPath) {
          draft = body
          savedContent = body
          editor?.markSaved()
          if (file) putTabBuffer(splitPath, { draft: body, saved: body, file: fileWithBody(file, body) })
        } else if (tabBuffers[splitPath]) {
          const buf = tabBuffers[splitPath]
          putTabBuffer(splitPath, { draft: body, saved: body, file: fileWithBody(buf.file, body) })
        }
        pushToast('info', 'Scratch buffer updated (not on disk)')
        return
      }
      let body = splitEditor?.getDoc() ?? splitDraft
      body = await applyWillSaveTransforms(splitPath, body)
      if (!(await callOk(CodeService.WriteFile(projectId, workspaceId, splitPath, body), 'Save failed'))) return
      splitDraft = body
      splitSaved = body
      splitEditor?.markSaved()
      markTabDirty(splitPath, false)
      if (splitPath === selectedPath) {
        draft = body
        savedContent = body
        editor?.markSaved()
        if (file) putTabBuffer(splitPath, { draft: body, saved: body, file: fileWithBody(file, body) })
      } else if (tabBuffers[splitPath]) {
        const buf = tabBuffers[splitPath]
        putTabBuffer(splitPath, { draft: body, saved: body, file: fileWithBody(buf.file, body) })
      }
      dropStash(splitPath)
      notifyDidSave(workspaceId, splitPath, body)
      await withFilesScroll(() => refreshGitStatus())
      return
    }
    if (!file?.writable || isBinaryPreviewMime(file.mime) || file.size > MAX_EDIT_BYTES) return
    if (isScratchPath(file.path)) {
      const body = showDiff
        ? (mergeEditor?.getDoc() ?? draft)
        : (editor?.getDoc() ?? draft)
      scratchDocs = {
        ...scratchDocs,
        [file.path]: { ...(scratchDocs[file.path] ?? { title: 'Scratch', lang: 'txt' }), content: body },
      }
      draft = body
      savedContent = body
      editor?.markSaved()
      markTabDirty(file.path, false)
      putTabBuffer(file.path, { draft: body, saved: body, file: fileWithBody(file, body) })
      pushToast('info', 'Scratch buffer updated (not on disk)')
      return
    }
    let body = showDiff
      ? (mergeEditor?.getDoc() ?? draft)
      : (editor?.getDoc() ?? draft)
    body = await applyWillSaveTransforms(file.path, body)
    if (!(await callOk(CodeService.WriteFile(projectId, workspaceId, file.path, body), 'Save failed'))) return
    // Keep the open FileContent identity — replacing it remounts/rekeys the editor
    // and can clear the tab strip via cascading effects.
    draft = body
    savedContent = body
    editor?.markSaved()
    markTabDirty(file.path, false)
    putTabBuffer(file.path, { draft: body, saved: body, file: fileWithBody(file, body) })
    if (splitOpen && splitPath === file.path) {
      splitDraft = body
      splitSaved = body
      splitEditor?.markSaved()
    }
    dropStash(file.path)
    notifyDidSave(workspaceId, file.path, body)
    await withFilesScroll(() => refreshGitStatus())
  }

  async function setViewMode(mode: EditorViewMode) {
    if (mode === 'diff') {
      if (isBinaryImagePath(selectedPath)) {
        if (!canShowImageDiff) {
          pushToast('info', 'No changes versus base for this file')
          return
        }
      } else {
        if (!selectedPath || !canShowDiff) {
          pushToast('info', 'No changes versus base for this file')
          return
        }
        if (baseDocPath !== selectedPath) {
          await loadBaseDoc(selectedPath)
        }
      }
    }
    // Capture from the surface being left (mode is where we're going): the
    // saved position then follows the user across Code ↔ Diff ↔ Preview.
    if (selectedPath) {
      rememberEditorView(selectedPath, viewMode === 'diff' ? mergeEditor : editor, mode)
    }
    viewMode = mode
    if (mode === 'code') {
      await tick()
      // The CodeMirror view stays mounted (hidden) across Preview/Diff, so
      // re-applying saved caret/scroll would jump a long file. Just remeasure.
      editor?.focus()
      editor?.resync()
    }
  }

  // Mod-S while Preview is showing (editor is hidden / unfocused).
  $effect(() => {
    if (!shellActive) return
    if (!showPreview || !writable) return
    const onKey = (e: KeyboardEvent) => {
      if (isCmdOrCtrl(e) && e.key === 's') {
        e.preventDefault()
        void saveEdit()
      }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  })

  function onNode(e: TreeEntry) {
    closeCtx()
    if (e.dir) {
      treePath = e.path
      void toggleDir(e.path)
    } else {
      void openPath(e.path, 0, filterChanged || entryStatus(e) === 'd')
    }
  }

  function onNodeClick(e: TreeEntry, ev: MouseEvent) {
    if (skipNodeClick) return
    if (ev.detail > 1) {
      void startRename(e.path, e.name, e.dir)
      return
    }
    onNode(e)
  }

  /** Skip the click that some engines fire on the source after a drag. */
  let skipNodeClick = false
  let skipNodeClickTimer: ReturnType<typeof setTimeout> | undefined

  function onTreeDragStart(ev: DragEvent, path: string, isDir: boolean) {
    if (!path) {
      ev.preventDefault()
      return
    }
    startFileDrag([isDir ? `${path.replace(/\/+$/, '')}/` : path], ev.dataTransfer)
  }

  /** Folder an explorer drag would move into ('' = root), or null when not droppable. */
  let treeDropDir = $state<string | null>(null)

  function treeDropTarget(ev: DragEvent): string | null {
    if (!treeWritable || !isFileDrag(ev.dataTransfer)) return null
    const row = (ev.target as Element | null)?.closest<HTMLElement>('[data-tree-path]')
    if (!row) return ''
    const p = row.dataset.treePath ?? ''
    return row.classList.contains('dir') ? p : parentPath(p)
  }

  function onTreeDragOver(ev: DragEvent) {
    const dir = treeDropTarget(ev)
    if (dir === null) return
    ev.preventDefault()
    if (ev.dataTransfer) ev.dataTransfer.dropEffect = 'move'
    treeDropDir = dir
  }

  function onTreeDragLeave(ev: DragEvent) {
    if (!(ev.currentTarget as Node).contains(ev.relatedTarget as Node | null)) treeDropDir = null
  }

  async function onTreeDrop(ev: DragEvent) {
    const dir = treeDropTarget(ev)
    treeDropDir = null
    if (dir === null) return
    ev.preventDefault()
    const srcs = fileDragPaths(ev.dataTransfer).map((p) => p.replace(/\/+$/, ''))
    for (const src of srcs) {
      const dest = joinPath(dir, src.split('/').pop() ?? '')
      if (!dest || dest === src || dir === src || dir.startsWith(src + '/')) continue
      await movePath(src, dest)
    }
  }

  function onTreeDragEnd() {
    skipNodeClick = true
    treeDropDir = null
    endFileDrag()
    clearTimeout(skipNodeClickTimer)
    skipNodeClickTimer = setTimeout(() => { skipNodeClick = false }, 0)
  }

  /**
   * Explorer shortcuts when the Files pane (or a tree row) has focus.
   * TextMate/Finder-first; VS Code where they agree (see D48).
   */
  function onFilesKeydown(e: KeyboardEvent) {
    const mod = isCmdOrCtrl(e)
    const alt = e.altKey
    const shift = e.shiftKey
    const lower = e.key.length === 1 ? e.key.toLowerCase() : e.key

    // Consume ⇧⌘N whenever Files is focused so app-level ⇧⌘N (blank workspace)
    // cannot navigate away from a draft name, context menu, or read-only tree.
    // New Folder still requires a writable tree and no draft/menu/input.
    if (mod && shift && !alt && lower === 'n') {
      e.preventDefault()
      e.stopPropagation()
      const t = e.target as HTMLElement | null
      const typing = !!t && (t.tagName === 'INPUT' || t.tagName === 'TEXTAREA' || t.isContentEditable)
      if (!draftEdit && !ctxMenu && !typing && treeWritable) void startNew('folder', actionDir)
      return
    }

    if (draftEdit || ctxMenu) return
    const t = e.target as HTMLElement | null
    if (t && (t.tagName === 'INPUT' || t.tagName === 'TEXTAREA' || t.isContentEditable)) return
    const path = treePath || selectedPath
    // Resolved lazily: this handler sees every keypress in the Files pane, and
    // findEntry is a full recursive walk of the tree — no reason to pay for it
    // on the arrow keys and plain letters that fall through to no branch.
    let entryResolved = false
    let entryValue: TreeEntry | null = null
    const entry = () => {
      if (!entryResolved) {
        entryResolved = true
        entryValue = path ? findEntry(activeTree, path) : null
      }
      return entryValue
    }

    // Open — TextMate ⌘↓
    if (mod && !alt && !shift && e.key === 'ArrowDown' && path && !entry()?.dir) {
      e.preventDefault()
      void openPath(path, 0, filterChanged || entryStatus(entry()!) === 'd')
      return
    }
    // New File — TextMate ⌥⌘N (avoids ⌘N = new loop)
    if (mod && alt && !shift && isLetterKey(e, 'n') && treeWritable) {
      e.preventDefault()
      void startNew('file', actionDir)
      return
    }
    // Copy Path (absolute) — VS Code ⌥⌘C
    if (mod && alt && !shift && isLetterKey(e, 'c') && path) {
      e.preventDefault()
      void copyAbsPath(path)
      return
    }
    // Copy Relative Path — VS Code ⇧⌥⌘C
    if (mod && alt && shift && isLetterKey(e, 'c') && path) {
      e.preventDefault()
      void copyRelPath(path)
      return
    }
    // Reveal in Finder — VS Code ⌥⌘R. Always consume the chord while Files is
    // focused (even with no selection) so the window-level toggle-review chord
    // does not also fire. Match the physical R key: Option+R is ® in WKWebView.
    if (mod && alt && !shift && isLetterKey(e, 'r')) {
      e.preventDefault()
      e.stopPropagation()
      if (path) void revealPath(path)
      return
    }
    // Cut / Copy / Paste / Duplicate
    if (mod && !alt && !shift && lower === 'x' && path && treeWritable) {
      e.preventDefault()
      clipCut(path, !!entry()?.dir)
      return
    }
    if (mod && !alt && !shift && lower === 'c' && path) {
      e.preventDefault()
      clipCopy(path, !!entry()?.dir)
      return
    }
    if (mod && !alt && !shift && lower === 'v' && treeWritable && fileClip) {
      e.preventDefault()
      void clipPaste(entry()?.dir ? path : parentPath(path))
      return
    }
    if (mod && !alt && !shift && lower === 'd' && path && treeWritable) {
      e.preventDefault()
      void duplicatePath(path, !!entry()?.dir)
      return
    }
    // Rename — TextMate / Finder / VS Code Mac ↩
    if (!mod && !alt && !shift && (e.key === 'Enter' || e.key === 'F2') && path && treeWritable) {
      e.preventDefault()
      void startRename(path, entry()?.name ?? path.split('/').pop() ?? '', entry()?.dir ?? false)
      return
    }
    // Delete — TextMate / VS Code ⌘⌫
    if (mod && !alt && !shift && (e.key === 'Backspace' || e.key === 'Delete') && path && treeWritable) {
      e.preventDefault()
      void deletePath(path, entry()?.name ?? path)
    }
  }

  $effect(() => {
    if (!ctxMenu) return
    const onDown = (ev: MouseEvent) => {
      const el = ev.target as HTMLElement | null
      if (el?.closest?.('.obs-ctx')) return
      closeCtx()
    }
    const onKey = (ev: KeyboardEvent) => {
      if (ev.key === 'Escape') closeCtx()
    }
    window.addEventListener('mousedown', onDown)
    window.addEventListener('keydown', onKey)
    return () => {
      window.removeEventListener('mousedown', onDown)
      window.removeEventListener('keydown', onKey)
    }
  })

  $effect(() => {
    if (!draftEdit) return
    void tick().then(() => {
      draftInput?.focus()
      draftInput?.select()
    })
  })

  function copyLine(line: number) {
    if (!file) return
    const ref = `${file.path}:${line}`
    void copyText(ref)
      .then(() => pushToast('info', 'Copied ' + ref))
      .catch((err: any) => pushToast('error', `Copy failed: ${err?.message ?? String(err)}`))
  }

  function closeMRU(path: string, e?: MouseEvent) {
    e?.stopPropagation()
    return closeTabs([path])
  }

  /** Close several tabs in one pass; one discard prompt, and at most one neighbor load. */
  async function closeTabs(paths: string[]) {
    const gone = new Set(paths.filter((p) => openFiles.includes(p) || (p && p === selectedPath)))
    if (!gone.size) return
    const dirty = [...gone].filter(tabIsDirty).length
    if (dirty) {
      const msg = dirty === 1 ? 'Discard unsaved edits?' : `Discard unsaved edits in ${dirty} tabs?`
      if (!(await askConfirm('Unsaved edits', msg, 'Discard'))) return
    }
    const idx = openFiles.indexOf(selectedPath)
    const wasSelected = gone.has(selectedPath)
    const remaining = openFiles.filter((p) => !gone.has(p))
    closedTabs = [...gone, ...closedTabs.filter((p) => !gone.has(p))].slice(0, 20)
    openFiles = remaining
    const nextBuf = { ...tabBuffers }
    const nextScratch = { ...scratchDocs }
    for (const path of gone) {
      dropStash(path)
      delete nextScratch[path]
      delete nextBuf[path]
      markTabDirty(path, false)
      if (editorPath === path) editorPath = ''
      if (treePath === path && wasSelected) treePath = ''
    }
    tabBuffers = nextBuf
    scratchDocs = nextScratch
    if (splitPath && gone.has(splitPath)) closeSplit()
    if (wasSelected) {
      file = null
      selectedIsDir = false
      baseDoc = ''
      baseDocPath = ''
      selectedPath = ''
      draft = ''
      savedContent = ''
      // Activate the neighbor that slid into this slot (right, or left if last) —
      // same as the terminal dock; not always the first tab.
      const next = remaining[Math.min(Math.max(idx, 0), remaining.length - 1)]
      if (next) void openPath(next)
      else detachLsp()
    }
    // Closed tab = dropped undo (like the buffer itself). Deferred past the
    // flush so an editor unmount's capture can't re-add what we just forgot.
    const scope = editorSessionKey
    void tick().then(() => { for (const path of gone) forgetEditorHistoryTree(scope, path) })
    if (!openFiles.length) persistOrClearEditorSession()
  }

  // Only reset explorer state when the observatory root actually changes.
  // Re-running on every parent render (e.g. workspace.event after save) was wiping
  // the tab strip / tree expand state mid-session.
  let loadedRoot = ''

  function flushEditorCaches(ed: { cachedBuffers: () => { path: string; doc: string; dirty: boolean }[] } | undefined) {
    try {
      for (const t of ed?.cachedBuffers() ?? []) {
        const existing = tabBuffers[t.path]
        if (!existing?.file) continue
        // A clean pane must not clobber a dirty buffer for the same path.
        if (!t.dirty && !editorTextEq(existing.draft, existing.saved)) continue
        putTabBuffer(t.path, { draft: t.doc, saved: existing.saved, file: existing.file })
        if (t.dirty) markTabDirty(t.path, true)
      }
    } catch { /* view may be mid-teardown */ }
  }

  /** Clean tabs are omitted — they can be re-read from disk on restore. */
  function dirtyTabBuffersForSnapshot(): Record<string, { draft: string; saved: string; file: FileContent }> {
    const out: Record<string, { draft: string; saved: string; file: FileContent }> = {}
    for (const [p, buf] of Object.entries(tabBuffers)) {
      if (editorTextEq(buf.draft, buf.saved)) continue
      out[p] = {
        draft: buf.draft,
        saved: buf.saved,
        file: { ...buf.file, content: buf.saved },
      }
    }
    return out
  }

  function captureSnapshot(): ObservatorySnapshot {
    // Flush debounced editor text so unsaved buffers survive the switch.
    let mainDraft = draft
    let sideDraft = splitDraft
    try {
      if (editor && selectedPath && !isBinaryPreviewMime(file?.mime)) {
        mainDraft = editor.getDoc()
      }
    } catch { /* view may be mid-teardown */ }
    try {
      if (splitEditor && splitOpen && splitPath) {
        sideDraft = splitEditor.getDoc()
      }
    } catch { /* ignore */ }
    const docs = { ...scratchDocs }
    if (isScratchPath(selectedPath)) {
      docs[selectedPath] = {
        ...(docs[selectedPath] ?? { title: 'Scratch', lang: 'txt' }),
        content: mainDraft,
      }
    }
    if (splitOpen && splitPath && isScratchPath(splitPath)) {
      docs[splitPath] = {
        ...(docs[splitPath] ?? { title: 'Scratch', lang: 'txt' }),
        content: sideDraft,
      }
    }
    // Don't clone multi-megabyte buffers into the workspace-switch LRU. Scratch
    // tabs have no disk copy to re-read, and `docs` already holds their body.
    const mainDirtyNow = mainEditorDirty || (!!file && !isBinaryPreviewMime(file.mime) && !editorTextEq(mainDraft, savedContent))
    const splitDirtyNow = splitEditorDirty || (!!splitPath && !editorTextEq(sideDraft, splitSaved))
    const slimMain = !mainDirtyNow && !isScratchPath(selectedPath) && mainDraft.length > EDITOR_BODY_CAP
    const slimSplit = !splitDirtyNow && !isScratchPath(splitPath) && sideDraft.length > EDITOR_BODY_CAP
    const slimFile = slimMain && file ? { ...file, content: '' } : file
    if (!restoringSessionViews) rememberLeavingEditors()
    if (selectedPath && file && !isBinaryPreviewMime(file.mime)) {
      putTabBuffer(selectedPath, { draft: mainDraft, saved: savedContent, file })
    }
    flushEditorCaches(editor)
    flushEditorCaches(splitEditor)
    const dirtyBufs = dirtyTabBuffersForSnapshot()
    const buffers = { ...stashedBuffers }
    if (selectedPath && !isScratchPath(selectedPath) && mainDirtyNow) {
      buffers[selectedPath] = { draft: mainDraft, saved: savedContent }
    }
    if (splitOpen && splitPath && !isScratchPath(splitPath) && splitDirtyNow) {
      buffers[splitPath] = { draft: sideDraft, saved: splitSaved }
    }
    const syn = syntaxModeSnapshot()
    const reviewView = reviewSurface?.getViewState()
    return {
      tree,
      expanded: [...expanded],
      loadedDirs: [...loadedDirs],
      changedCollapsed: [...changedCollapsed],
      changedPaths: [...changedPaths],
      gitStatus: { ...gitStatus },
      gitStatusReady,
      workingStatus: { ...workingStatus },
      stagedFiles: [...stagedFiles],
      openFiles: [...openFiles],
      closedTabs: [...closedTabs],
      selectedPath,
      treePath,
      file: slimFile,
      draft: slimMain ? '' : mainDraft,
      savedContent: slimMain ? '' : savedContent,
      outline: [...outline],
      viewMode,
      baseDoc,
      baseDocPath,
      browseAll,
      scopeChosen,
      leftKind,
      rightKind,
      leftOpen: leftGutterOpen,
      showSide,
      splitOpen,
      splitPath,
      splitDraft: slimSplit ? '' : sideDraft,
      splitSaved: slimSplit ? '' : splitSaved,
      splitWritable,
      focusedPane,
      focusZone,
      mainEditorDirty: mainDirtyNow,
      splitEditorDirty: splitDirtyNow,
      sessionRestored,
      sessionLoadSettled,
      syntaxMapsReady,
      scratchDocs: docs,
      scratchSeq,
      fileViews: previewScroll.applyPreviewScrollToFileViews({ ...fileViews }, [
        ...openFiles,
        ...(splitOpen && splitPath ? [splitPath] : []),
      ]),
      syntaxByPath: syn.syntaxByPath,
      syntaxByFileType: syn.syntaxByFileType,
      tabBuffers: Object.keys(dirtyBufs).length ? dirtyBufs : undefined,
      stashedBuffers: buffers,
      fileTabScrollLeft: captureStripScrollLeft(fileTabStripEl),
      reviewTab,
      reviewSelectedPath: reviewView?.selectedPath,
      reviewScrollTop: reviewView?.scrollTop,
      // Pref, not the git-clamped derived — capture runs after the incoming
      // workspace's projectId is already current, so isGit may not be this root.
      mainSurface: mainSurfacePref,
    }
  }

  function applySnapshot(snap: ObservatorySnapshot) {
    endTreeLoad()
    tree = snap.tree
    expanded = new Set(snap.expanded)
    loadedDirs = new Set(snap.loadedDirs)
    loadingDirs = new Set()
    dirNotFoundSince.clear()
    maybeClearGoneConfirm()
    changedCollapsed = new Set(snap.changedCollapsed)
    changedPaths = snap.changedPaths
    gitStatus = snap.gitStatus
    gitStatusReady = snap.gitStatusReady === true
    workingStatus = snap.workingStatus
    stagedFiles = snap.stagedFiles
    openFiles = uniqueOpenFiles(snap.openFiles)
    closedTabs = snap.closedTabs
    selectedPath = snap.selectedPath
    treePath = snap.treePath || snap.selectedPath
    file = snap.file
    draft = snap.draft
    savedContent = snap.savedContent
    outline = snap.outline
    viewMode = snap.viewMode
    baseDoc = snap.baseDoc
    baseDocPath = snap.baseDocPath
    if (snap.baseDocPath && snap.baseDoc) rememberBaseDoc(snap.baseDocPath, snap.baseDoc)
    browseAll = snap.browseAll
    scopeChosen = snap.scopeChosen
    leftKind = snap.leftKind
    rightKind = snap.rightKind
    if (typeof snap.leftOpen === 'boolean') setLeftGutterOpen(snap.leftOpen)
    if (typeof snap.showSide === 'boolean') showSide = snap.showSide
    splitOpen = snap.splitOpen
    splitPath = snap.splitPath
    splitDraft = snap.splitDraft
    splitSaved = snap.splitSaved
    splitWritable = snap.splitWritable
    focusedPane = snap.focusedPane
    focusZone = snap.focusZone ?? 'editor'
    mainEditorDirty = snap.mainEditorDirty
    splitEditorDirty = snap.splitEditorDirty
    sessionRestored = snap.sessionRestored
    scratchDocs = snap.scratchDocs ? { ...snap.scratchDocs } : {}
    scratchSeq = snap.scratchSeq ?? maxScratchSeq(Object.keys(scratchDocs))
    fileViews = snap.fileViews ? { ...snap.fileViews } : {}
    previewScroll.hydratePreviewScroll(fileViews, workspaceId)
    hydrateSyntaxMode(snap.syntaxByPath, snap.syntaxByFileType)
    restoringSessionViews = false
    restoringSessionViewsToken = 0
    editorPath = snap.selectedPath && snap.file ? snap.selectedPath : ''
    hydrateFailedPath = ''
    const seed = snap.selectedPath && snap.file && (snap.draft || snap.savedContent)
      ? { [snap.selectedPath]: { draft: snap.draft, saved: snap.savedContent, file: snap.file } }
      : {}
    tabBuffers = { ...seed, ...(snap.tabBuffers ?? {}) }
    const nextDirty: Record<string, boolean> = {}
    for (const [p, b] of Object.entries(tabBuffers)) {
      if (!editorTextEq(b.draft, b.saved)) nextDirty[p] = true
    }
    if (snap.mainEditorDirty && snap.selectedPath) nextDirty[snap.selectedPath] = true
    if (snap.splitEditorDirty && snap.splitPath) nextDirty[snap.splitPath] = true
    dirtyTabs = nextDirty
    stashedBuffers = snap.stashedBuffers ? { ...snap.stashedBuffers } : {}
    fileTabScrollRestore = typeof snap.fileTabScrollLeft === 'number'
      ? { left: snap.fileTabScrollLeft, id: ++fileTabScrollRestoreId }
      : undefined
    if (snap.reviewTab) setReviewTab(snap.reviewTab)
    reviewRestore = snap.reviewSelectedPath
      ? { selectedPath: snap.reviewSelectedPath, scrollTop: snap.reviewScrollTop ?? 0 }
      : null
    const savedSurface = snap.mainSurface
    mainSurfacePref = savedSurface === 'review' ? 'review' : 'editor'
    sessionLoadSettled = !!snap.sessionLoadSettled || snap.openFiles.length > 0
    syntaxMapsReady = snap.syntaxMapsReady ?? !!snap.sessionLoadSettled
    hits = []
    refs = []
    searchSkippedLarge = 0
    searchTruncated = false
    findings = []
    expandedLists = new Set()
    openFileNotFoundPath = ''
    openFileNotFoundSince = 0
    maybeClearGoneConfirm()
    setFileLoading(false)
    // Slimmed or mid-load snapshots have no body — fetch it back. Do this for
    // both panes; otherwise a >1 MB split buffer comes back empty and Cmd+S
    // would truncate the file on disk.
    // Deleted-on-disk tabs also have file == null (diff of baseDoc only) — that
    // is a finished state, not a mid-load, and hydrating it toasts not-found.
    const deletedDiff =
      snap.file == null &&
      snap.viewMode === 'diff' &&
      snap.baseDocPath === snap.selectedPath
    const mainNeedsBody =
      !!snap.selectedPath &&
      !deletedDiff &&
      !isScratchPath(snap.selectedPath) &&
      !isBinaryPreviewMime(snap.file?.mime) &&
      (snap.file == null || (!snap.file.content && snap.file.size > 0))
    if (mainNeedsBody) {
      void hydrateFile(snap.selectedPath)
    } else if (snap.selectedPath && snap.file && !deletedDiff) {
      void tick().then(() => refreshLsp(snap.selectedPath))
    }
    if (
      snap.splitOpen &&
      snap.splitPath &&
      !isScratchPath(snap.splitPath) &&
      !snap.splitDraft &&
      !snap.splitSaved
    ) {
      void restoreSplitBuffer(snap.splitPath)
    }
  }

  async function restoreSplitBuffer(path: string) {
    try {
      const f = await readEditorFile(path)
      if (splitPath !== path) return
      splitDraft = f.content
      splitSaved = f.content
      splitEditorDirty = false
      splitWritable = !!f.writable && !isBinaryPreviewMime(f.mime) && f.size <= MAX_EDIT_BYTES
      overlaySplitStash(path)
    } catch {
      /* user can reopen the tab */
    }
  }

  /**
   * Pull the Go-side session for this root. `sessionLoadSettled` becomes true
   * only when the RPC resolves for the still-current root — a reject or a
   * mid-flight switch leaves it false so persist cannot clear a session that
   * was never shown.
   */
  function restoreSessionFromBackend(root: string, restoreToken: number) {
    void loadEditorSession(projectId, workspaceId).then((sess) => {
      if (rootToken !== restoreToken || loadedRoot !== root) return
      sessionLoadSettled = true
      syntaxMapsReady = !sess || sess.syntaxMapsKnown !== false
      // Always hydrate language maps — they outlive the tab strip. Merge any
      // pick made after the cold empty reset so a click-before-RPC cannot
      // drop a choice, and so Go never sees empty maps overwrite stored ones.
      hydrateSyntaxMode(sess?.syntaxByPath, sess?.syntaxByFileType, true)
      if (mainSurfacePref !== 'review' && !openFiles.length && !selectedPath) {
        mainSurfacePref = sess?.mainSurface === 'review' ? 'review' : 'editor'
      }
      applySessionPaneChrome(sess)
      if (initialPath) return
      if (!sess?.openFiles?.length) {
        if (openFiles.length || selectedPath) writeEditorSession(false)
        return
      }
      if (openFiles.length || selectedPath) {
        writeEditorSession(false)
        return
      }
      restoringSessionViews = true
      restoringSessionViewsToken = restoreToken
      if (sess.fileViews) {
        fileViews = parseFileViews(sess.fileViews)
        previewScroll.hydratePreviewScroll(fileViews, workspaceId)
      }
      if (sess.scratchDocs) scratchDocs = { ...sess.scratchDocs }
      scratchSeq = Math.max(sess.scratchSeq ?? 0, maxScratchSeq([
        ...sess.openFiles,
        ...(sess.splitPath ? [sess.splitPath] : []),
        ...Object.keys(scratchDocs),
      ]))
      openFiles = uniqueOpenFiles(sess.openFiles)
      stashedBuffers = sess.stashedBuffers ? { ...sess.stashedBuffers } : {}
      sessionRestored = true
      const pick = sess.selectedPath && sess.openFiles.includes(sess.selectedPath)
        ? sess.selectedPath
        : sess.openFiles[0]
      const finishViews = () => {
        // Scroll lands on the measure frames after openPath resolves
        // (scheduleApplyFileViewState). Releasing earlier lets a persist tick
        // snapshot topLine 1 / scrollTop 0 over the restored caret.
        requestAnimationFrame(() => {
          requestAnimationFrame(() => {
            if (rootToken !== restoreToken) return
            if (restoringSessionViewsToken !== restoreToken) return
            restoringSessionViews = false
          })
        })
      }
      if (pick) void openPath(pick).finally(finishViews)
      else finishViews()
      if (sess.splitPath) {
        // Restore secondary pane without stealing focus from the primary pick.
        void (async () => {
          await openInSplit(sess.splitPath!)
          focusedPane = 'main'
          editor?.focus()
        })()
      }
    }).catch(() => {
      // Backend read failed — sessionLoadSettled stays false, so leaving
      // this root cannot clear a stored session that was never shown.
    })
  }

  // Before paint: a post-paint $effect left the previous worktree on screen
  // (tree, git, open file) under the new workspace's chrome until the first
  // agent turn flushed more updates. Creating a workspace that immediately
  // runs made that look like "stuck on the last workspace".
  $effect.pre(() => {
    const root = observatoryCacheKey(projectId, workspaceId)
    if (root === loadedRoot) return
    // Restore any in-flight file-tab drag onto this root's openFiles before
    // keep-alive snapshot / persist, then before W2's files replace them.
    fileTabDrag.cancel()

    // Keep-alive: stash the outgoing root so A→B→A can paint instantly.
    if (loadedRoot && loadedRoot !== '\0') {
      putObservatorySnapshot(loadedRoot, captureSnapshot())
      persistSessionForRoot(loadedRoot)
      // Park parsed EditorStates before openFiles/editorPath change — retainPaths
      // and a cold editorPath='' unmount would otherwise drop the live cache.
      editor?.parkSession(loadedRoot)
      splitEditor?.parkSession(loadedRoot)
      const prevWid = loadedRoot.includes('\0') ? loadedRoot.slice(loadedRoot.indexOf('\0') + 1) : ''
      detachLsp()
      if (prevWid) {
        clearLspForWorkspace(prevWid)
        void CloseWorkspaceLSP(prevWid)
      }
    }

    loadedRoot = root
    rootToken += 1
    // Drop in-flight git/working polls from the outgoing workspace so they
    // cannot apply — or toast — against the incoming id (eager create has
    // no worktree yet; StagedFiles used to 422).
    gitStatusGen += 1
    workingStatusGen += 1
    worktreeMissing = false
    baseDocCache.clear()
    baseDocInflight.clear()
    baseDocEpoch++

    const cached = root !== '\0' ? getObservatorySnapshot(root) : null
    if (cached) {
      applySnapshot(cached)
      editorSessionKey = root
      // Retry restore when the snapshot was taken before load settled — an
      // empty keep-alive is not proof the stored session is empty, and
      // sessionLoadSettled must not flip true or persist will clear it.
      if (!sessionLoadSettled) restoreSessionFromBackend(root, rootToken)
      // Stale-while-revalidate: show cached UI, refresh tree/git in background
      // without a skeleton flash over the restored tree.
      void refreshTree({ soft: true })
      if (selectedPath && file && !mainEditorDirty && editorTextEq(draft, savedContent)) {
        void refreshOpenFileIfStale()
      }
      if (selectedPath) void revealInTree(selectedPath)
      if (selectedPath) prefetchBaseDoc(selectedPath)
      if (splitPath) prefetchBaseDoc(splitPath)
      if (workspaceId) void loadFindings()
      skipViewRestore = false
      // Parked EditorState already has caret+scroll. restoreCodeView would
      // focus immediately and yank WebKit to the caret. Resync the height map,
      // then restore this workspace's last chrome on the next frame so typing
      // and ⌘F land without moving the viewport. The terminal dock remounts
      // on {#key workspaceId} and must not steal — TerminalPane only takes
      // keys when the dock already owns them.
      const restoreFocusToken = rootToken
      const restoreZone = untrack(() => focusZone)
      void tick().then(() => {
        if (rootToken !== restoreFocusToken) return
        editor?.resync()
        if (splitOpen) splitEditor?.resync()
        requestAnimationFrame(() => {
          requestAnimationFrame(() => {
            if (rootToken !== restoreFocusToken) return
            restoreChromeFocus(restoreZone, restoreFocusToken)
          })
        })
      })
      return
    }

    // Cold open — drop previous worktree data synchronously so scopePaths
    // never unions two repos' files while the new reads land.
    resetExplorerData()
    // Honour an explicit Changed/All-files choice for this project+loop; only
    // fall back to the per-view default when the user has never picked one.
    const savedScope = loadExplorerScope(projectId, workspaceId)
    scopeChosen = savedScope !== null
    browseAll = savedScope ?? !loopScoped
    changedCollapsed = new Set()
    openFiles = []
    scratchDocs = {}
    scratchSeq = 0
    stashedBuffers = {}
    file = null
    selectedIsDir = false
    selectedPath = ''
    treePath = ''
    closeSplit()
    sessionRestored = false
    closedTabs = []
    rightKind = 'files'
    leftKind = 'maestro'
    showSide = true
    mainEditorDirty = false
    splitEditorDirty = false
    focusZone = 'editor'
    baseDoc = ''
    baseDocPath = ''
    viewMode = 'code'
    fileViews = {}
    previewScroll.hydratePreviewScroll({}, workspaceId)
    hydrateSyntaxMode()
    restoringSessionViews = false
    restoringSessionViewsToken = 0
    skipViewRestore = false
    fileTabScrollRestore = undefined
    reviewRestore = null
    mainSurfacePref = 'editor'
    draft = ''
    savedContent = ''
    editorPath = ''
    tabBuffers = {}
    dirtyTabs = {}
    editorSessionKey = root
    void refreshTree()
    if (workspaceId) void loadFindings()
    // Session restore — the session lives Go-side, so this is an async round
    // trip. Guarded so a root switch, or a file the user opened while the RPC
    // was in flight, wins over the restore.
    sessionLoadSettled = false
    syntaxMapsReady = false
    restoreSessionFromBackend(root, rootToken)
    // No keep-alive snapshot: last chrome is unknown. Dock chrome (open /
    // editorHidden) is restored from disk, so a terminal-only workspace would
    // otherwise mount a dock that no longer autofocuses and an editor that
    // isn't there — keys land on <body>.
    if (editorHidden && termOpen && workspaceId) {
      const restoreFocusToken = rootToken
      void tick().then(() => {
        if (rootToken !== restoreFocusToken) return
        requestAnimationFrame(() => {
          requestAnimationFrame(() => {
            if (rootToken !== restoreFocusToken) return
            restoreChromeFocus('terminal', restoreFocusToken)
          })
        })
      })
    }
  })

  // Drop keep-alive entries for workspaces that no longer exist. (Stored
  // sessions need no pruning: they live in workspaces/<id>/ Go-side and are
  // swept when the workspace is deleted.)
  $effect(() => {
    const live = $workspaces.map((w) => w.id)
    pruneObservatorySnapshots(live)
    pruneSessionTabParks(live)
  })

  onDestroy(() => {
    // Persist keep-alive on unmount (e.g. leaving the detail shell entirely).
    // dropWorkspace already pruned this root — do not put it back.
    if (!loadedRoot || loadedRoot === '\0') return
    if (workspaceId && !get(workspaces).some((w) => w.id === workspaceId && !w.archived)) return
    try {
      putObservatorySnapshot(loadedRoot, captureSnapshot())
    } catch { /* ignore */ }
    persistSessionForRoot(loadedRoot)
  })

  // Keep scratchDocs in sync with the live editor buffer (session persist reads it).
  $effect(() => {
    if (!isScratchPath(selectedPath)) return
    const path = selectedPath
    const content = draft
    const prev = scratchDocs[path]
    if (prev?.content === content) return
    scratchDocs = {
      ...scratchDocs,
      [path]: { ...(prev ?? { title: 'Scratch', lang: 'txt' }), content },
    }
  })

  $effect(() => {
    if (!splitOpen || !splitPath || !isScratchPath(splitPath)) return
    const path = splitPath
    const content = splitDraft
    const prev = scratchDocs[path]
    if (prev?.content === content) return
    scratchDocs = {
      ...scratchDocs,
      [path]: { ...(prev ?? { title: 'Scratch', lang: 'txt' }), content },
    }
  })

  $effect(() => {
    setSyntaxModePersist(() => writeEditorSession(false))
    return () => setSyntaxModePersist(null)
  })

  $effect(() => {
    // Persist open tabs (+ scratch + unsaved disk buffers) whenever they change.
    // Skip the empty wipe that runs on root switch before restore.
    if (!projectId && !workspaceId) return
    void draft
    void savedContent
    void mainEditorDirty
    void stashedBuffers
    void splitDraft
    void splitSaved
    void splitEditorDirty
    void scratchDocs
    void scratchSeq
    void selectedPath
    void splitOpen
    void splitPath
    void viewMode
    void openFiles
    void leftKind
    void rightKind
    void showSide
    void leftGutterOpen
    void mainSurfacePref
    if (fileTabDrag.active || fileTabDrag.committing) return
    if (!openFiles.length && !paneChromeNonDefault() && mainSurfacePref !== 'review') {
      persistOrClearEditorSession()
      return
    }
    writeEditorSession(false)
  })

  $effect(() => {
    const onHide = () => persistSessionForRoot(loadedRoot)
    window.addEventListener('pagehide', onHide)
    const onVis = () => {
      if (document.visibilityState === 'hidden') onHide()
    }
    document.addEventListener('visibilitychange', onVis)
    return () => {
      window.removeEventListener('pagehide', onHide)
      document.removeEventListener('visibilitychange', onVis)
    }
  })

  $effect(() => {
    const jump = $observatoryJump
    if (!jump) return
    if (jump.workspaceId && workspaceId && jump.workspaceId !== workspaceId) return
    if (jump.projectId && projectId && jump.projectId !== projectId) return
    if (!shellActive && !jump.workspaceId) return
    if (jump.panel === 'plan') {
      observatoryJump.set(null)
      if (jump.planTab) planPanelTab.set(jump.planTab)
      void selectCodePanel('plan')
      return
    }
    if (jump.panel === 'review') {
      if (mainBlocked || !isGit) return
      observatoryJump.set(null)
      showReviewSurface()
      setReviewTab('changes')
      return
    }
    if (jump.findingId || jump.panel === 'findings') {
      const id = jump.findingId || ''
      const focusNote = !!jump.focusNote
      observatoryJump.set(null)
      void (async () => {
        await selectCodePanel('findings')
        findingsFilter = 'open'
        const list = await loadFindings()
        if (id) {
          findingsExpandIds = [id]
          findingsFocusNoteId = focusNote ? id : ''
          const f = list.find((x) => findingMatchesRef(x, id))
          if (f?.file) openPath(f.file, f.line ?? 0, false, f.endLine || 0)
          // Clear after FindingsPanel applies so the same id can deep-link again
          // and so expand/focus props don't keep re-applying.
          await tick()
          findingsExpandIds = []
          findingsFocusNoteId = ''
        }
      })()
      return
    }
    if (!jump.path) return
    // Chat file chips (and other path jumps): open the editor without stealing
    // the current side panel. Explorer selection only moves if Files is showing
    // that path already — no expand-to-reveal of collapsed/filtered rows.
    openPath(jump.path, jump.line ?? 0, !!jump.preferDiff, jump.endLine ?? 0, 'if-visible', false, jump.startCol ?? 0, jump.endCol ?? 0)
    observatoryJump.set(null)
  })

  $effect(() => {
    if (initialPath) openPath(initialPath)
  })

  // Push enable/disable to the native menubar as focus / tabs / dirty change.
  // Reset only on unmount — an effect cleanup would flash everything disabled on
  // every focus change.
  $effect(() => {
    if (!termOpen) termTabCount = 0
  })
  $effect(() => {
    if (!shellActive) return
    const id = workspaceId
    menuOwner = id
    const inTerm = focusZone === 'terminal'
    const inEditor = focusZone === 'editor'
    const inAgent = focusZone === 'agent'
    const inReview = inEditor && mainSurface === 'review'
    const tabCount = inTerm
      ? termTabCount
      : inReview
        ? (reviewHasCommits ? 2 : 1)
        : inEditor
          ? openFiles.length
          : inAgent
            ? agentTabCount
            : 0
    const canClose = inTerm
      ? termTabCount > 0
      : inReview
        ? false
        : inEditor
          ? !!selectedPath
          : inAgent && agentTabCount > 0 && !!onAgentCloseTab
    setMenuState({
      newTab: (inTerm && !!workspaceId) || (inAgent && !!onAgentNewTab),
      save: dirty,
      closeTab: canClose,
      toggleLeftPanel: hasLeft || portalAgentToPR,
      toggleRightPanel: hasRight && !mainBlocked,
      toggleEditor: !!workspaceId && !mainBlocked && !termOverlaying,
      toggleTerminal: !!workspaceId && !mainBlocked,
      maximizeTerminal: !!workspaceId && termOpen && !mainBlocked,
      focusAgent: (hasLeft && leftPanels.some((p) => p.key === 'maestro')) || portalAgentToPR,
      focusEditor: !mainBlocked,
      focusTerminal: !!workspaceId && !mainBlocked,
      prevTab: tabCount > 1,
      nextTab: tabCount > 1,
      runDev: !!workspaceId && !mainBlocked && !!workspaceDevCmd(workspaceId, $workspaces, $projects),
      stopRun: !!workspaceId && !mainBlocked && isAnyRunRunning(workspaceId, $runSessionBy),
    })
  })
  onDestroy(() => {
    if (menuOwner === workspaceId) {
      menuOwner = ''
      setMenuState(MENU_STATE_IDLE)
    }
  })

  // Native menubar actions (accelerators may never reach the webview on macOS).
  $effect(() => {
    if (!shellActive) return
    return onMenuAction((action) => {
      switch (action) {
        case 'focus-agent':
          void focusAgentPanel()
          return
        case 'focus-editor':
          if (!mainBlocked) focusCodePanel()
          return
        case 'focus-terminal':
          if (!mainBlocked) void focusTerminalPanel()
          return
        case 'toggle-left-panel':
          toggleVisibleLeftPanel()
          return
        case 'toggle-right-panel':
          if (hasRight && !mainBlocked) setShowSide(!showSide)
          return
        case 'toggle-editor':
          if (workspaceId && !mainBlocked && !termOverlaying) toggleEditorSurface()
          return
        case 'quick-open':
          if (!mainBlocked) void openQuickOpen('file')
          return
        case 'go-symbol':
          if (!mainBlocked) void openQuickOpen('symbol')
          return
        case 'save':
          void saveEdit()
          return
        case 'new-tab':
          // Active context: terminal dock or solo/pair agent chats. Editor tabs
          // open via Quick Open / the tree, not an empty "new tab".
          if (focusZone === 'terminal' && !mainBlocked) termDock?.newTab()
          else if (focusZone === 'agent') onAgentNewTab?.()
          return
        case 'close-tab':
          if (focusZone === 'terminal') termDock?.closeActiveTab()
          else if (focusZone === 'editor' && mainSurface === 'editor' && selectedPath && !mainBlocked) void closeMRU(selectedPath)
          else if (focusZone === 'agent') onAgentCloseTab?.()
          return
        case 'prev-tab':
          if (focusZone === 'terminal') termDock?.rotateActiveTab(-1)
          else if (focusZone === 'editor' && mainSurface === 'review') {
            if (!mainBlocked) cycleReviewTab(-1)
          } else if (focusZone === 'editor' && openFiles.length > 1 && !mainBlocked) {
            const i = Math.max(0, openFiles.indexOf(selectedPath))
            void openPath(openFiles[(i - 1 + openFiles.length) % openFiles.length])
          } else if (focusZone === 'agent') {
            onAgentRotateTab?.(-1)
          }
          return
        case 'next-tab':
          if (focusZone === 'terminal') termDock?.rotateActiveTab(1)
          else if (focusZone === 'editor' && mainSurface === 'review') {
            if (!mainBlocked) cycleReviewTab(1)
          } else if (focusZone === 'editor' && openFiles.length > 1 && !mainBlocked) {
            const i = Math.max(0, openFiles.indexOf(selectedPath))
            void openPath(openFiles[(i + 1) % openFiles.length])
          } else if (focusZone === 'agent') {
            onAgentRotateTab?.(1)
          }
          return
      }
    })
  })

  // Global editor chords (Quick Open, tabs, reload, symbol, split, panel focus)
  $effect(() => {
    if (!shellActive) return
    const onKey = (e: KeyboardEvent) => {
      const t = e.target as HTMLElement | null
      const typing = t && (t.tagName === 'INPUT' || t.tagName === 'TEXTAREA' || t.isContentEditable)
      const mod = isCmdOrCtrl(e)
      const lower = e.key.toLowerCase()

      // ⌘1 agent · ⌘2 editor · ⌘3 terminal — workbench focus, not text.
      if (mod && !e.shiftKey && !e.altKey && (lower === '1' || lower === '2' || lower === '3')) {
        e.preventDefault()
        if (lower === '1') void focusAgentPanel()
        else if (mainBlocked) return
        else if (lower === '2') focusCodePanel()
        else void focusTerminalPanel()
        return
      }

      // ⌘L — @-mention the current editor file / selection in the agent composer.
      if (mod && !e.shiftKey && !e.altKey && lower === 'l') {
        if (!onAddEditorMention) return
        const path = focusedPane === 'split' && splitOpen && splitPath ? splitPath : selectedPath
        if (!path || isScratchPath(path)) return
        e.preventDefault()
        void addEditorMentionToChat()
        return
      }

      // ⌥⌘B left panel · ⌥⌘⇧B right panel — visibility toggles (⌘B is the app rail).
      if (mod && e.altKey && isLetterKey(e, 'b')) {
        e.preventDefault()
        if (e.shiftKey) {
          if (hasRight && !mainBlocked) setShowSide(!showSide)
        } else {
          toggleVisibleLeftPanel()
        }
        return
      }

      // ⌥⌘E — hide/show the center editor so the terminal can own the middle panel.
      if (mod && e.altKey && !e.shiftKey && isLetterKey(e, 'e')) {
        if (!workspaceId || mainBlocked || termOverlaying) return
        e.preventDefault()
        toggleEditorSurface()
        return
      }

      // ⌥⌘R — toggle the branch review surface (Changes | Commits).
      // Match e.code: Option+R is ® on US Mac, so e.key !== 'r' in WKWebView.
      // Files (Reveal in Finder) stopPropagates while the tree is focused.
      // Yield to the find-widget regexp toggle. INPUT/TEXTAREA (Quick Open, Find,
      // explorer filter, xterm helper) keep the chord; CodeMirror is
      // contenteditable and must still receive it (same trap as ⌘⇧V).
      if (mod && e.altKey && !e.shiftKey && isLetterKey(e, 'r')) {
        if (t?.closest('.cm-recur-search') || t?.closest('.file-preview-find') || t?.closest('.term-dock')) return
        if (t && (t.tagName === 'INPUT' || t.tagName === 'TEXTAREA')) return
        if (t?.isContentEditable && !t.closest('.cm-editor')) return
        if (quickOpen || get(paletteOpen) || get(commandsOpen) || get(shortcutsOpen) || get(newTaskOpen) || get(newProjectOpen) || get(syntaxModeOpen)) return
        if (get(keybindingMenu)) return
        if (!workspaceId || mainBlocked || termOverlaying || !isGit) return
        e.preventDefault()
        const opening = mainSurface !== 'review'
        const focusEl = document.activeElement as HTMLElement | null
        const needsPark = opening
          ? !!focusEl?.closest('.obs-main, .obs-tb-center')
          : !!focusEl?.closest('.obs-review-host')
        toggleReview()
        if (needsPark) parkAfterReviewToggle(opening)
        return
      }

      if (mod && !e.shiftKey && !e.altKey && lower === 'p') {
        if (mainBlocked) return
        e.preventDefault()
        void openQuickOpen('file')
        return
      }
      if (mod && e.shiftKey && !e.altKey && lower === 'o') {
        if (mainBlocked) return
        e.preventDefault()
        void openQuickOpen('symbol')
        return
      }
      if (mod && !e.shiftKey && !e.altKey && lower === 't') {
        // Terminal owns ⌘T while focused (capture). Agent dock: new chat tab.
        if (t?.closest('.term-dock')) return
        if (focusZone === 'agent' && onAgentNewTab) {
          e.preventDefault()
          onAgentNewTab()
        }
        return
      }
      if (mod && !e.shiftKey && !e.altKey && lower === 'w') {
        // Terminal dock owns ⌘W while focused (capture); else close the active
        // file tab or agent chat tab for the focused context.
        if (t?.closest('.term-dock')) return
        if (focusZone === 'editor' && mainSurface === 'editor' && selectedPath) {
          if (mainBlocked) return
          e.preventDefault()
          void closeMRU(selectedPath)
        } else if (focusZone === 'agent' && onAgentCloseTab) {
          e.preventDefault()
          onAgentCloseTab()
        }
        return
      }
      if (mod && e.shiftKey && !e.altKey && lower === 't' && closedTabs.length) {
        if (mainBlocked) return
        e.preventDefault()
        const path = closedTabs[0]
        closedTabs = closedTabs.slice(1)
        void openPath(path)
        return
      }
      if (mod && e.altKey && !e.shiftKey && (e.key === 'ArrowRight' || e.key === 'ArrowLeft')) {
        if (t?.closest('.term-dock')) return
        // Match menubar: cycle tabs in the focused context (editor or agent).
        if (focusZone === 'editor' && mainSurface === 'review') {
          if (mainBlocked) return
          e.preventDefault()
          cycleReviewTab(e.key === 'ArrowRight' ? 1 : -1)
        } else if (focusZone === 'editor' && openFiles.length > 1) {
          if (mainBlocked) return
          e.preventDefault()
          const i = Math.max(0, openFiles.indexOf(selectedPath))
          const next = e.key === 'ArrowRight'
            ? openFiles[(i + 1) % openFiles.length]
            : openFiles[(i - 1 + openFiles.length) % openFiles.length]
          void openPath(next)
        } else if (focusZone === 'agent' && agentTabCount > 1 && onAgentRotateTab) {
          e.preventDefault()
          onAgentRotateTab(e.key === 'ArrowRight' ? 1 : -1)
        }
        return
      }
      if (mod && e.key === '\\') {
        if (mainBlocked) return
        e.preventDefault()
        void toggleSplit()
        return
      }
      // ⌘⇧V / ⌃⇧V — cycle Code / Diff / Preview (whatever the view chip offers).
      // Editor-scoped like ⌘W: skip the composer, palettes, and terminal dock
      // (⌃⇧V is paste in many shells). INPUT/TEXTAREA (Find, explorer search)
      // keep the keystroke for themselves; CodeMirror is contenteditable and
      // must still receive the cycle. viewMode is main-pane only — Diff
      // unmounts the split editor, so don't fire from a focused split.
      // ⌃⇧V is an extra Mac alias on top of CmdOrCtrl (⌘⇧V); on Windows/Linux
      // CmdOrCtrl is already Ctrl.
      if ((mod || e.ctrlKey) && e.shiftKey && !e.altKey && lower === 'v') {
        if (t?.closest('.term-dock')) return
        if (focusZone !== 'editor' || mainBlocked || editorHidden) return
        if (splitOpen && focusedPane === 'split') return
        if (t && (t.tagName === 'INPUT' || t.tagName === 'TEXTAREA')) return
        if (get(paletteOpen) || get(commandsOpen) || get(shortcutsOpen) || get(newTaskOpen) || get(newProjectOpen) || get(syntaxModeOpen)) return
        if (get(keybindingMenu)) return
        const next = nextEditorViewMode(viewMode, availableViewModes)
        if (!next) return
        e.preventDefault()
        void setViewMode(next)
        return
      }
      if (!typing && lower === 'f5') {
        if (mainBlocked) return
        e.preventDefault()
        void reloadFromDisk()
      }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  })

  /** Sync index of palette commands and snippets by canonical keyEquivalent. */
  type ChordHit =
    | { kind: 'command'; cmd: RegisteredCommand }
    | { kind: 'snippet'; sn: RegisteredSnippet }
  let chordIndex = $state<Map<string, ChordHit[]>>(new Map())

  $effect(() => {
    void $bundlesEpoch
    const path = focusedPane === 'split' && splitOpen && splitPath ? splitPath : selectedPath
    if (!path) {
      chordIndex = new Map()
      return
    }
    let cancelled = false
    const scope = scopeForPath(path)
    void Promise.all([listCommands(scope, path), listSnippets(scope, path)]).then(([cmds, snips]) => {
      if (cancelled) return
      const m = new Map<string, ChordHit[]>()
      for (const c of cmds) {
        if (!c.triggers.includes('palette') || !c.keyEquivalent) continue
        const list = m.get(c.keyEquivalent) ?? []
        list.push({ kind: 'command', cmd: c })
        m.set(c.keyEquivalent, list)
      }
      for (const s of snips) {
        if (!s.keyEquivalent) continue
        const list = m.get(s.keyEquivalent) ?? []
        list.push({ kind: 'snippet', sn: s })
        m.set(s.keyEquivalent, list)
      }
      chordIndex = m
    })
    return () => { cancelled = true }
  })

  function focusedEditor() {
    return focusedPane === 'split' && splitOpen && splitEditor ? splitEditor : editor
  }

  function insertBundleSnippet(sn: RegisteredSnippet) {
    const focused = focusedEditor()
    if (!focused) {
      pushToast('error', 'Open a file in the editor first')
      return
    }
    const next = focused.applyBundleSnippet(sn)
    if (next != null) {
      if (focusedPane === 'split' && splitOpen) {
        splitDraft = next
        if (splitPath === selectedPath) draft = next
      } else {
        draft = next
      }
    }
    focused.focus()
  }

  function applyChordHit(hit: ChordHit) {
    if (hit.kind === 'command') {
      recordCommandUsage(hit.cmd.id)
      void runEditorCommand(hit.cmd.id)
      return
    }
    insertBundleSnippet(hit.sn)
  }

  function chordItem(hit: ChordHit) {
    if (hit.kind === 'command') {
      return {
        id: `c:${hit.cmd.id}`,
        label: hit.cmd.label,
        category: hit.cmd.category,
        order: hit.cmd.order,
      }
    }
    return {
      id: `s:${hit.sn.id}`,
      label: hit.sn.label,
      category: hit.sn.bundleName || 'Snippets',
      order: hit.sn.order,
    }
  }

  // Bundle keyEquivalent dispatch (editor focus only; app-reserved chords ignored).
  $effect(() => {
    if (!shellActive) return
    const onKey = (e: KeyboardEvent) => {
      const path = focusedPane === 'split' && splitOpen && splitPath ? splitPath : selectedPath
      if (focusZone !== 'editor' || !path) return
      if (get(paletteOpen) || get(commandsOpen) || get(shortcutsOpen) || get(newTaskOpen) || get(newProjectOpen) || get(syntaxModeOpen)) return
      if (get(keybindingMenu)) return
      if (e.isComposing || e.repeat) return
      const chord = chordFromKeyboardEvent(e)
      if (!chord || isReservedAppChord(chord)) return
      const hasMod = e.metaKey || e.ctrlKey || e.altKey
      const keyPart = chord.split('+').pop() ?? ''
      const isFn = /^f\d{1,2}$/.test(keyPart)
      if (!hasMod && !isFn) return

      const matches = chordIndex.get(chord) ?? []
      if (!matches.length) return
      e.preventDefault()
      e.stopPropagation()
      if (matches.length === 1) {
        applyChordHit(matches[0]!)
        return
      }
      openKeybindingMenu(matches.map(chordItem), (item) => {
        const hit = matches.find((h) => chordItem(h).id === item.id)
        if (hit) applyChordHit(hit)
      })
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  })
</script>

<svelte:window onmessage={onPreviewIframeMessage} />

<div
  class="obs"
  class:busy
  class:loop-scoped={loopScoped}
  class:term-max={termOverlaying}
  class:term-max-anim={termMaxAnim}
  class:editor-hidden={mainSurface === 'editor' && (termOverlaying ? overlayKeepEditorHidden : editorHidden)}
  bind:this={obsEl}
  onfocusin={onObsFocusIn}
>
  <div
    class="obs-toolbar"
    class:no-side={!hasRight || !showSide}
    class:no-left={!hasLeft || !leftGutterOpen}
    class:has-left={hasLeft}
    inert={termOverlaying || undefined}
  >
    {#if hasLeft}
      <div class="obs-tb-left">
        <button
          type="button"
          class="obs-pane-toggle"
          class:on={leftGutterOpen}
          onclick={() => setLeftGutterOpen(!leftGutterOpen)}
          title={leftGutterOpen ? 'Hide left panel (⌥⌘B)' : 'Show left panel (⌥⌘B)'}
          aria-label={leftGutterOpen ? 'Hide left panel' : 'Show left panel'}
          aria-pressed={leftGutterOpen}
        >
          <Icon name="panel-left" />
        </button>
        <div class="obs-panel-icons" role="toolbar" aria-label="Left panels">
          {#each leftPanels as p (p.key)}
            <button
              type="button"
              class="obs-pane-toggle"
              class:on={leftGutterOpen && leftKind === p.key}
              title={paneBadgeTitle(p)}
              aria-label={paneBadgeTitle(p)}
              aria-pressed={leftGutterOpen && leftKind === p.key}
              onclick={() => selectCodePanel(p.key)}
            >
              <span class="obs-pane-icon-wrap">
                {@render panelIcon(p.key)}
                {#if paneBadgeCount(p.key)}
                  <span class="obs-pane-badge">{paneBadgeLabel(paneBadgeCount(p.key))}</span>
                {/if}
              </span>
            </button>
          {/each}
        </div>
      </div>
    {:else if !loopScoped}
      <div class="obs-tb-left">
        <span class="obs-title">Code observatory</span>
      </div>
    {/if}
    <TabStrip
      class="obs-tb-center"
      label={mainSurface === 'review' ? 'Review' : 'Open files'}
      activeKey={mainSurface === 'review' ? reviewStripTab : selectedPath}
      restoreScroll={fileTabScrollRestore}
      revealNonce={fileTabRevealNonce}
      bind:stripEl={fileTabStripEl}
    >
      {#if mainSurface === 'review'}
        <TabChip
          class="obs-reviewchip"
          label="Changes"
          title={reviewCount ? `Changes (${reviewCount})` : 'Changes'}
          icon="file-diff"
          badge={reviewCount}
          active={reviewStripTab === 'changes'}
          lit={focusZone === 'editor'}
          onclick={() => { if (!mainBlocked) setReviewTab('changes') }}
        />
        {#if reviewHasCommits}
          <TabChip
            class="obs-reviewchip"
            label="Commits"
            title={reviewCommitCount
              ? (workingDirtyCount ? `Commits (${reviewCommitCount}) · ${workingDirtyCount} uncommitted files` : `Commits (${reviewCommitCount})`)
              : (workingDirtyCount ? `Commits · ${workingDirtyCount} uncommitted files` : 'Commits')}
            icon="git-commit"
            badge={reviewCommitCount}
            dirty={workingDirtyCount > 0}
            active={reviewStripTab === 'commits'}
            lit={focusZone === 'editor'}
            onclick={() => { if (!mainBlocked) setReviewTab('commits') }}
          />
        {/if}
      {:else}
        {#each uniqueOpenFiles(openFiles) as p (p)}
          {@const name = isScratchPath(p) ? scratchTabLabel(p) : (p.split('/').pop() || p)}
          {@const floating = fileTabDrag.active && fileTabDrag.fromKey === p}
          {#if floating}
            <div class="tab-chip-ph" style="width:{fileTabDrag.w}px" role="presentation" aria-hidden="true"></div>
          {:else}
            <TabChip
              class="obs-filechip"
              tabKey={p}
              reorderable={!mainBlocked && openFiles.length > 1}
              label={name}
              title={isScratchPath(p) ? `${scratchDocs[p]?.title || name} (${p})` : p}
              iconClass={fileIconClass(isScratchPath(p) ? `file.${scratchDocs[p]?.lang || 'txt'}` : name, $theme)}
              active={selectedPath === p}
              lit={focusZone === 'editor'}
              dirty={tabIsDirty(p)}
              onclick={() => {
                if (fileTabDrag.suppressClick) {
                  fileTabDrag.suppressClick = false
                  return
                }
                if (!mainBlocked) openPath(p)
              }}
              onclose={mainBlocked ? undefined : (e) => closeMRU(p, e)}
              oncontextmenu={mainBlocked ? undefined : (e) => openCtx(e, p, false, name, true)}
              onpointerdown={(e) => fileTabDrag.onPointerDown(e, p)}
              onkeydown={(e) => fileTabDrag.onKeydown(e, p)}
            />
          {/if}
        {/each}
      {/if}
      {#snippet trailing()}
        {#if workspaceId}
          <button
            type="button"
            class="obs-pane-toggle obs-editor-toggle"
            class:on={mainSurface === 'editor' && !editorHidden}
            disabled={mainBlocked}
            onclick={() => toggleEditorSurface()}
            title={mainBlocked ? 'Waiting for the worktree…' : (mainSurface !== 'editor' || editorHidden ? 'Show editor (⌥⌘E)' : 'Hide editor (⌥⌘E)')}
            aria-label={mainSurface !== 'editor' || editorHidden ? 'Show editor' : 'Hide editor'}
            aria-pressed={mainSurface === 'editor' && !editorHidden}
          >
            <Icon name="code" />
          </button>
          {#if isGit}
            <button
              type="button"
              class="obs-pane-toggle obs-review-toggle"
              class:on={mainSurface === 'review'}
              disabled={mainBlocked}
              title={mainBlocked ? 'Waiting for the worktree…' : 'Review changes (⌥⌘R)'}
              aria-label="Review changes"
              aria-pressed={mainSurface === 'review'}
              onclick={() => toggleReview()}
            >
              <Icon name="file-diff" />
            </button>
          {/if}
          <button
            type="button"
            class="obs-pane-toggle obs-term-toggle"
            class:on={termOpen}
            disabled={mainBlocked}
            onclick={() => setTerminalDockOpen(workspaceId, !termOpen)}
            title={mainBlocked ? 'Waiting for the worktree…' : termOpen ? 'Hide terminal (⌃`)' : 'Show terminal (⌃`)'}
            aria-label={termOpen ? 'Hide terminal' : 'Show terminal'}
            aria-pressed={termOpen}
          >
            <Icon name="terminal" />
          </button>
          <button
            type="button"
            class="obs-pane-toggle obs-term-beside-toggle"
            class:on={termBeside}
            disabled={mainBlocked || termOverlaying}
            onclick={() => toggleTermBeside()}
            title={mainBlocked ? 'Waiting for the worktree…' : termBeside ? 'Terminal below editor' : 'Terminal beside editor'}
            aria-label={termBeside ? 'Terminal below editor' : 'Terminal beside editor'}
            aria-pressed={termBeside}
          >
            <Icon name="square-split-horizontal" />
          </button>
        {/if}
      {/snippet}
    </TabStrip>
    {#if fileTabDrag.active && fileTabDrag.fromKey}
      {@const p = fileTabDrag.fromKey}
      {@const name = isScratchPath(p) ? scratchTabLabel(p) : (p.split('/').pop() || p)}
      <div
        class="tab-chip-float"
        bind:this={fileTabFloatEl}
        style="left:{fileTabDrag.x}px;top:{fileTabDrag.y}px;width:{fileTabDrag.w}px;height:{fileTabDrag.h}px"
      >
        <TabChip
          class="obs-filechip"
          floating
          label={name}
          iconClass={fileIconClass(isScratchPath(p) ? `file.${scratchDocs[p]?.lang || 'txt'}` : name, $theme)}
          active={selectedPath === p}
          lit={focusZone === 'editor'}
          dirty={tabIsDirty(p)}
        />
      </div>
    {/if}
    {#if hasRight}
      <div class="obs-tb-right">
        <div class="obs-panel-icons" role="toolbar" aria-label="Side panels">
          {#each rightPanels as p (p.key)}
            <button
              type="button"
              class="obs-pane-toggle"
              class:on={showSide && rightKind === p.key}
              disabled={mainBlocked}
              title={mainBlocked ? 'Waiting for the worktree…' : paneBadgeTitle(p)}
              aria-label={paneBadgeTitle(p)}
              aria-pressed={showSide && rightKind === p.key}
              onclick={() => selectCodePanel(p.key)}
            >
              <span class="obs-pane-icon-wrap">
                {@render panelIcon(p.key)}
                {#if paneBadgeCount(p.key)}
                  <span class="obs-pane-badge">{paneBadgeLabel(paneBadgeCount(p.key))}</span>
                {/if}
              </span>
            </button>
          {/each}
        </div>
        <button
          type="button"
          class="obs-pane-toggle"
          class:on={showSide}
          disabled={mainBlocked}
          onclick={() => setShowSide(!showSide)}
          title={mainBlocked ? 'Waiting for the worktree…' : showSide ? 'Hide right panel (⌥⌘⇧B)' : 'Show right panel (⌥⌘⇧B)'}
          aria-label={showSide ? 'Hide right panel' : 'Show right panel'}
          aria-pressed={showSide}
        >
          <Icon name="panel-right" />
        </button>
      </div>
    {/if}
  </div>

  <div
    class="obs-body"
    class:no-side={!hasRight || !showSide}
    class:no-left={!hasLeft || !leftGutterOpen}
    class:has-left={hasLeft}
  >
    {#if (hasLeft && leftGutterOpen && leftMeta) || portalAgentToPR}
      {@const kind = portalAgentToPR ? 'maestro' : leftKind}
      {@const meta = portalAgentToPR
        ? (allPanelMeta.find((p) => p.key === 'maestro') ?? leftMeta)
        : leftMeta}
      <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
      <aside
        class="obs-left"
        aria-label={meta?.title ?? 'Agent'}
        tabindex="-1"
        bind:this={leftAsideEl}
        onmousedown={onLeftMouseDown}
        inert={termOverlaying || undefined}
        {@attach portalAgentToPR && attachAgentPortal}
      >
        <div
          class="obs-side-head"
          class:tabbed={(kind === 'maestro' && !!leftHead && leftTitle === 'Agent') || (kind === 'plan' && !!planHead)}
        >
          {#if portalAgentToPR}
            <button
              type="button"
              class="obs-pane-toggle prd-chat-toggle"
              class:on={$prAgentDockOpen}
              onclick={() => setPRAgentDockOpen(false)}
              title="Hide left panel (⌥⌘B)"
              aria-label="Hide left panel"
              aria-pressed={true}
            >
              <Icon name="panel-left" />
            </button>
          {/if}
          {#if kind === 'maestro' && leftHead}
            {@render leftHead(focusZone === 'agent')}
          {:else if kind === 'plan' && planHead}
            {@render planHead()}
          {:else}
            <span class="obs-side-title">{meta?.label ?? 'Agent'}</span>
            {@render panelHeadExtras(kind)}
          {/if}
        </div>
        <div class="obs-pane-body obs-left-body">
          {@render panelBody(kind)}
        </div>
        <ResizeHandle panel={portalAgentToPR ? 'prChatDock' : 'maestroDock'} edge="end" />
      </aside>
    {/if}

    <!-- Editor + files share one grid area so the creating overlay can cover
         both without occupying cells and wrapping them onto a second row. -->
    <div class="obs-work">
    <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
    <main
      class="obs-main"
      class:term-beside={termBesideLayout}
      class:fullscreen={editorFullscreen}
      style={termBesideLayout ? `--obs-term-beside-pct:${termBesidePct}%` : undefined}
      tabindex="-1"
      inert={mainBlocked || undefined}
      onkeydown={(e) => {
        if (!editorFullscreen || e.key !== 'Escape' || e.defaultPrevented) return
        e.preventDefault()
        void setEditorFullscreen(false)
      }}
      onmousemove={editorFullscreen ? revealFsExit : undefined}
    >
      {#if selectedPath || (splitOpen && splitPath)}
        <div
          class="obs-crumbbar"
          class:off={mainSurface !== 'editor'}
          class:split={showSplitLayout}
          style={showSplitLayout ? `--obs-split-pct:${splitPct}%` : undefined}
          aria-label="File path"
          inert={termOverlaying || mainSurface !== 'editor' || undefined}
        >
          {#if showSplitLayout}
            <button
              type="button"
              class="pane-file"
              class:on={focusedPane === 'main'}
              class:lit={focusZone === 'editor' && focusedPane === 'main'}
              class:dirty={dirty}
              title={selectedPath || 'Empty'}
              onclick={() => focusPane('main')}
            >
              <span class="name">{selectedPath ? selectedPath.split('/').pop() : 'Empty'}</span>
              {#if dirty}<span class="dot" aria-label="unsaved"></span>{/if}
            </button>
            <button
              type="button"
              class="pane-file"
              class:on={focusedPane === 'split'}
              class:lit={focusZone === 'editor' && focusedPane === 'split'}
              class:dirty={splitDirty}
              title={splitPath || 'Empty'}
              onclick={() => focusPane('split')}
            >
              <span class="name">{splitPath ? splitPath.split('/').pop() : 'Empty'}</span>
              {#if splitDirty}<span class="dot" aria-label="unsaved"></span>{/if}
            </button>
          {:else}
            {#each pathCrumbs as part, i}
              {#if i > 0}<span class="sep">/</span>{/if}
              <button
                type="button"
                class="crumb"
                class:file={i === pathCrumbs.length - 1}
                title={pathCrumbs.slice(0, i + 1).join('/')}
                onclick={() => {
                  if (i === pathCrumbs.length - 1) return
                  void revealInTree(pathCrumbs.slice(0, i + 1).join('/') + '/_')
                  void selectCodePanel('files')
                }}
              >{part}</button>
            {/each}
            <div class="sp"></div>
          {/if}
          <div class="obs-crumb-actions">
            {#if isBinaryImagePath(selectedPath) && availableViewModes.includes('diff')}
              <div class="seg obs-view-seg" role="group" aria-label="View mode" title={reviewBase ? `Cycle view (⌘⇧V) · Diff vs ${reviewBase} (merge-base)` : 'Cycle view (⌘⇧V) · Diff vs HEAD'}>
                {#if availableViewModes.includes('preview')}
                  <button
                    type="button"
                    class:on={viewMode !== 'diff'}
                    onclick={() => setViewMode('preview')}
                  >Preview</button>
                {/if}
                <button
                  type="button"
                  class:on={viewMode === 'diff'}
                  title={reviewBase ? `Diff vs ${reviewBase} (merge-base)` : 'Diff vs HEAD'}
                  onclick={() => setViewMode('diff')}
                >Diff</button>
              </div>
            {:else if canShowDiff || previewKind}
              <div class="seg obs-view-seg" role="group" aria-label="View mode" title={reviewBase ? `Cycle view (⌘⇧V) · Diff vs ${reviewBase} (merge-base)` : 'Cycle view (⌘⇧V) · Diff vs HEAD'}>
                <button
                  type="button"
                  class:on={viewMode === 'code'}
                  disabled={!file || selectedGitStatus === 'd'}
                  onclick={() => setViewMode('code')}
                >Code</button>
                {#if canShowDiff}
                  <button
                    type="button"
                    class:on={viewMode === 'diff'}
                    title={reviewBase ? `Diff vs ${reviewBase} (merge-base)` : 'Diff vs HEAD'}
                    onclick={() => setViewMode('diff')}
                  >Diff</button>
                {/if}
                {#if previewKind}
                  <button
                    type="button"
                    class:on={viewMode === 'preview'}
                    disabled={!file || selectedGitStatus === 'd'}
                    onclick={() => setViewMode('preview')}
                  >Preview</button>
                {/if}
              </div>
            {/if}
            {#if isGit && viewMode === 'diff'}
              <button
                type="button"
                class="obs-icon-btn"
                title="All diffs — review changes since {reviewBase || 'HEAD'} (⌥⌘R)"
                aria-label="Review all diffs"
                onclick={() => { setReviewTab('changes'); toggleReview() }}
              >
                <Icon name="rows-3" />
              </button>
            {/if}
            <button
              type="button"
              class="obs-icon-btn"
              class:on={findPanelOpen}
              title="Find (⌘F)"
              aria-label="Find"
              aria-pressed={findPanelOpen}
              disabled={!canToggleFind}
              onclick={() => toggleFindPanel()}
            >
              <Icon name="search" />
            </button>
            <button type="button" class="obs-icon-btn" class:on={splitOpen} title="Split editor (⌘\\) — click a pane to focus; opens go there" aria-label="Split editor" onclick={() => toggleSplit()}>
              <Icon name="columns" />
            </button>
            <button type="button" class="obs-icon-btn" title="Reload from disk (F5)" aria-label="Reload" onclick={() => reloadFromDisk()}>
              <Icon name="refresh" />
            </button>
            {#if previewKind || selectedArtifactId || showImagePane || showPdfPane || showVideoPane}
              <button type="button" class="obs-icon-btn" title="Full screen" aria-label="Full screen" onclick={async () => { if (previewKind) await setViewMode('preview'); await setEditorFullscreen(true) }}>
                <Icon name="maximize" />
              </button>
            {/if}
          </div>
        </div>
      {/if}
      <div
        class="obs-editors"
        class:off={mainSurface !== 'editor'}
        class:split={showSplitLayout}
        style={showSplitLayout ? `--obs-split-pct:${splitPct}%` : undefined}
        inert={termOverlaying || mainSurface !== 'editor' || undefined}
      >
        <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_noninteractive_element_interactions -->
        <div
          class="obs-editor-pane"
          class:focused={focusedPane === 'main'}
          class:lit={focusZone === 'editor' && focusedPane === 'main'}
          role="group"
          aria-label="Main editor"
          onclick={() => { if (splitOpen) focusPane('main') }}
        >
          {#if showImagePane}
            <div class="obs-image" class:compare={imageCompare}>
              {#if imageCompare}
                <div class="obs-image-pair">
                  <figure class="obs-image-pane">
                    <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_noninteractive_element_interactions -->
                    <img
                      src={imageBaseSrc}
                      alt=""
                      onerror={() => { imageBaseFailedSrc = imageBaseSrc }}
                      onclick={() => { mediaZoom = { src: imageBaseSrc, name: `${selectedPath} (previous)` } }}
                    />
                    <figcaption>Previous</figcaption>
                  </figure>
                  {#if file && selectedGitStatus !== 'd'}
                    <figure class="obs-image-pane">
                      <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_noninteractive_element_interactions -->
                      <img src={imageSrc} alt={file.path} onclick={() => { mediaZoom = { src: imageSrc, name: file?.path ?? '' } }} />
                      <figcaption>Current</figcaption>
                    </figure>
                  {/if}
                </div>
              {:else if file && selectedGitStatus !== 'd'}
                <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_noninteractive_element_interactions -->
                <img src={imageSrc} alt={file.path} onclick={() => { mediaZoom = { src: imageSrc, name: file?.path ?? '' } }} />
              {:else}
                <div class="obs-empty pad">This image was deleted.</div>
              {/if}
              <div class="obs-image-meta">
                <span class="path">{file?.path || selectedPath}</span>
                {#if file}
                  <span class="meta">{file.mime} · {formatBytes(file.size)}</span>
                  {#if selectedGitStatus !== 'd'}
                    <button type="button" class="obs-icon-btn" title="Zoom" aria-label="Zoom image" onclick={() => { mediaZoom = { src: imageSrc, name: file?.path ?? '' } }}>
                      <Icon name="zoom-in" />
                    </button>
                  {/if}
                {/if}
              </div>
            </div>
          {:else if showPdfPane && file}
            <div class="obs-pdf">
              <PdfPreview src={mediaSrc} path={file.path} />
              <div class="obs-image-meta">
                <span class="path">{file.path}</span>
                <span class="meta">{file.mime} · {formatBytes(file.size)}</span>
              </div>
            </div>
          {:else if showVideoPane && file}
            <div class="obs-image">
              <!-- svelte-ignore a11y_media_has_caption -->
              <video bind:this={paneVideo} src={mediaSrc} controls></video>
              <div class="obs-image-meta">
                <span class="path">{file.path}</span>
                <span class="meta">{file.mime} · {formatBytes(file.size)}</span>
                <button type="button" class="obs-icon-btn" title="Zoom" aria-label="Zoom video" onclick={zoomVideo}>
                  <Icon name="zoom-in" />
                </button>
              </div>
            </div>
          {:else}
            {#if editorPath}
              <div class="obs-surface" class:off={showPreview || showDiff || viewMode === 'diff'}>
                <CodeEditor
                  bind:this={editor}
                  doc={editorPath === selectedPath ? draft : (tabBuffers[editorPath]?.draft ?? draft)}
                  savedDoc={editorPath === selectedPath ? savedContent : (tabBuffers[editorPath]?.saved ?? savedContent)}
                  path={editorPath}
                  sessionKey={editorSessionKey}
                  readOnly={!writable || showPreview || showDiff || viewMode === 'diff'}
                  viewState={skipViewRestore ? undefined : fileViews[editorPath]}
                  findingMarks={mainFindingMarks}
                  baseDoc={gutterBaseFor(editorPath)}
                  onDocChange={onMainDocChange}
                  onDirtyChange={(d) => {
                    mainEditorDirty = d
                    markTabDirty(editorPath, d)
                    if (!d && editorPath) dropStash(editorPath)
                  }}
                  onSave={saveEdit}
                  onCopyLine={copyLine}
                  onCursor={(ln, col) => { if (focusedPane === 'main') { cursorLine = ln; cursorCol = col } }}
                  onGotoDef={gotoDef}
                  onSearchOpenChange={syncFindPanelOpen}
                  onOpenFinding={openFindingFromEditor}
                />
              </div>
            {/if}
            {#if previewKind}
              <div class="obs-surface" class:off={!showPreview} inert={!showPreview || undefined}>
                <FilePreview bind:this={filePreview} kind={previewKind} content={draft} path={selectedPath} loaded={file?.path ?? ''} workspace={workspaceId} scroll={previewScroll} active={filePreviewFindActive({ showPreview, shellActive, mainSurface, termOverlaying, editorHidden })} onSearchOpenChange={syncFindPanelOpen} />
              </div>
            {/if}
            {#if showDiff}
              <div class="obs-diff">
                {#key selectedPath + '\0' + baseDocPath}
                  <MergeEditor
                    bind:this={mergeEditor}
                    path={selectedPath}
                    doc={mergeCurrent}
                    original={baseDoc}
                    readOnly={!writable || selectedGitStatus === 'd'}
                    historyScope={editorSessionKey}
                    viewState={skipViewRestore ? null : (fileViews[selectedPath] ?? null)}
                    onDocChange={onMainDocChange}
                    onSave={saveEdit}
                    onCopyLine={copyLine}
                    onSearchOpenChange={syncFindPanelOpen}
                  />
                {/key}
              </div>
            {:else if !editorPath && !showPreview}
              {#if showFileLoading}
                <div class="obs-empty pad" aria-busy="true">Loading {selectedPath.split('/').pop() || selectedPath}…</div>
              {:else if fileLoading}
                <div class="obs-empty pad" aria-busy="true"></div>
              {:else}
                <div class="obs-empty pad">
                  {#if selectedIsDir}
                    This path is a directory (often a git submodule). Use <b>Diff</b> to see the recorded change, or browse it from the Files tree.
                  {:else if treeWritable}
                    Select a file to view/edit.
                  {:else}
                    Select a file to view.
                  {/if}
                </div>
              {/if}
            {/if}
            {#if (showFileLoading || diffPending) && editorPath}
              <div class="obs-load-overlay" aria-busy="true">Loading {selectedPath.split('/').pop() || selectedPath}…</div>
            {/if}
          {/if}
        </div>
        {#if showSplitLayout}
          <button
            type="button"
            class="vsplit obs-esplit"
            class:dragging={splitResizing}
            aria-label="Resize editor panes"
            title="Drag to resize"
            onpointerdown={onSplitResizeDown}
            onkeydown={onSplitResizeKey}
          ></button>
          <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_noninteractive_element_interactions -->
          <div
            class="obs-editor-pane split-side"
            class:focused={focusedPane === 'split'}
            class:lit={focusZone === 'editor' && focusedPane === 'split'}
            role="group"
            aria-label="Split editor"
            onclick={() => focusPane('split')}
          >
            {#if splitPath}
              <div class="obs-surface">
                <CodeEditor
                  bind:this={splitEditor}
                  doc={splitDraft}
                  savedDoc={splitSaved}
                  path={splitPath}
                  sessionKey={editorSessionKey}
                  parkPane="split"
                  readOnly={!splitWritable}
                  viewState={fileViews[splitPath]}
                  findingMarks={splitFindingMarks}
                  baseDoc={gutterBaseFor(splitPath)}
                  onDocChange={onSplitDocChange}
                  onDirtyChange={(d) => {
                    splitEditorDirty = d
                    markTabDirty(splitPath, d)
                    if (!d && splitPath) dropStash(splitPath)
                  }}
                  onSave={saveEdit}
                  onCopyLine={copyLine}
                  onCursor={(ln, col) => { if (focusedPane === 'split') { cursorLine = ln; cursorCol = col } }}
                  onGotoDef={gotoDef}
                  onSearchOpenChange={syncFindPanelOpen}
                  onOpenFinding={openFindingFromEditor}
                />
              </div>
            {:else}
              <div class="obs-empty pad">Open a file — it will land in this pane while focused.</div>
            {/if}
          </div>
        {/if}
      </div>
      {#if isGit}
        <div
          class="obs-review-host"
          class:off={mainSurface === 'editor'}
          inert={termOverlaying || mainSurface === 'editor' || undefined}
          tabindex="-1"
        >
          <ReviewSurface
            bind:this={reviewSurface}
            {projectId}
            {workspaceId}
            baseLabel={reviewBase}
            refresh={reviewRefresh}
            active={mainSurface === 'review'}
            tab={reviewTab}
            onTabChange={setReviewTab}
            onOpenFile={(path, line) => { void openPath(path, line ?? 0, true) }}
            onAddFinding={addReviewFinding}
            findingsByPath={reviewFindingsByPath}
            onOpenFinding={openReviewFinding}
            onShowFindings={() => { void selectCodePanel('findings') }}
            dirtyPaths={reviewDirtyPaths}
            workingDirtyCount={workingDirtyCount}
            restore={reviewRestore}
            count={reviewCount}
            {excludeMatchers}
            bind:commitCount={reviewCommitCount}
            bind:commitScopeHash={reviewScopeHash}
            bind:workingOpen={reviewWorkingOpen}
            bind:diffTotals={reviewDiffTotals}
            bind:fileCount={reviewFileCount}
          />
        </div>
      {/if}
      {#if activePath && mainSurface === 'editor'}
        <div class="obs-statusbar" aria-label="Editor status" inert={termOverlaying || undefined}>
          <span>Ln {cursorLine}, Col {cursorCol}</span>
          <span class="sep">·</span>
          <button type="button" class="linkish" onclick={() => { quickOpen = false; openSyntaxModePicker() }} aria-haspopup="dialog" aria-label="Change language mode">{editorLang || 'Plain Text'}</button>
          <span class="sep">·</span>
          <button type="button" class="linkish" title="Commands and snippets for this file" onclick={() => openCommandsPalette()}>Bundles</button>
          <Kbd text="⌘⇧P" />
          {#if activeGitStatus}<span class="sep">·</span><span class="st st-{activeGitStatus}">{activeGitStatus.toUpperCase()}</span><span class="obs-review-vs">vs {reviewBase || 'HEAD'}</span>{/if}
          {#if activeDirty}<span class="sep">·</span><span>Modified</span>{/if}
          {#if splitOpen}<span class="sep">·</span><span>{focusedPane === 'split' ? 'Split' : 'Main'} focused</span>{/if}
          {#if activeFileProblems}
            <span class="sep">·</span>
            <button
              type="button"
              class="linkish st-problems"
              class:warn={!activeFileProblems.errors && !!activeFileProblems.warnings}
              title="Open Problems"
              onclick={() => selectCodePanel('problems')}
            >
              {#if activeFileProblems.errors}{activeFileProblems.errors} error{activeFileProblems.errors === 1 ? '' : 's'}{/if}
              {#if activeFileProblems.errors && activeFileProblems.warnings}<span class="sep">·</span>{/if}
              {#if activeFileProblems.warnings}{activeFileProblems.warnings} warning{activeFileProblems.warnings === 1 ? '' : 's'}{/if}
              {#if !activeFileProblems.errors && !activeFileProblems.warnings}{activeFileProblems.count} issue{activeFileProblems.count === 1 ? '' : 's'}{/if}
            </button>
          {/if}
          <div class="sp"></div>
          {#if currentSymbol}
            <span class="sep">·</span>
            <button
              type="button"
              class="linkish cur-sym"
              title="{currentSymbol.name.trim()} — Go to Symbol"
              onclick={() => openQuickOpen('symbol')}
            >{currentSymbol.name.trim()}</button>
            <span class="sep">·</span>
          {/if}
          <button type="button" class="linkish" onclick={() => openQuickOpen('symbol')}>Go to Symbol</button>
          <Kbd text="⌘⇧O" />
        </div>
      {:else if mainSurface === 'review'}
        <div class="obs-statusbar" aria-label="Review status" inert={termOverlaying || undefined}>
          <span>{reviewFileCount} file{reviewFileCount === 1 ? '' : 's'}</span>
          <span class="sep">·</span>
          <span class="st"><span class="st-a">+{reviewDiffTotals.add}</span> <span class="st-d">−{reviewDiffTotals.del}</span></span>
          <span class="sep">·</span>
          {#if reviewScopeHash}
            <span class="obs-review-vs">this commit</span>
          {:else if reviewWorkingOpen}
            <span class="obs-review-vs">vs HEAD</span>
          {:else}
            <span class="obs-review-vs">vs {reviewBase || 'HEAD'}</span>
          {/if}
          {#if openFindingsCount}
            <span class="sep">·</span>
            <button type="button" class="linkish st st-findings" onclick={() => void selectCodePanel('findings')}>{openFindingsCount} finding{openFindingsCount === 1 ? '' : 's'}</button>
          {/if}
        </div>
      {/if}

      <!-- Terminal drawer. Default is under the editor (full width of .obs-main);
           term-beside puts it in a right column. Not in the movable panel rail:
           a shell needs ~80 columns and the side docks default to 260px. Keyed
           by workspace so switching rebuilds the dock against that workspace's
           shells; the previous ones keep running in Go and are re-adopted on
           return. Remount must not autofocus — Observatory restores this
           workspace's last chrome (editor / agent / terminal) instead. -->
      {#if termOpen}
        {#if termOverlaying && !overlayKeepEditorHidden}
          <div
            class="term-dock-slot"
            style={termBesideLayout ? undefined : `height:${termSlotH}px`}
            aria-hidden="true"
          ></div>
        {/if}
        {#if termBesideLayout}
          <button
            type="button"
            class="vsplit obs-tsplit"
            class:dragging={termBesideResizing}
            aria-label="Resize editor and terminal"
            title="Drag to resize"
            onpointerdown={onTermBesideResizeDown}
            onkeydown={onTermBesideResizeKey}
          ></button>
        {/if}
        {#key workspaceId}
          <TerminalDock
            bind:this={termDock}
            {workspaceId}
            overlay={termOverlaying}
            beside={termBesideLayout}
            gpu={shellActive}
            lit={focusZone === 'terminal'}
            onTabsChange={(n) => (termTabCount = n)}
          />
        {/key}
      {/if}
      {#if editorFullscreen}
        <div class="obs-fs-exit" class:show={fsExitVisible}>
          <button type="button" class="obs-icon-btn" title="Exit full screen (Esc)" aria-label="Exit full screen" onclick={() => setEditorFullscreen(false)}>
            <Icon name="minimize" />
          </button>
        </div>
      {/if}
      {#if mediaZoom}
        <AttachmentLightbox
          {...mediaZoom}
          onclose={(t) => {
            if (mediaZoom?.video && paneVideo && t !== undefined) paneVideo.currentTime = t
            mediaZoom = null
          }}
        />
      {/if}
    </main>

    {#if hasRight && showSide && rightMeta && !(portalAgentToPR && rightKind === 'maestro')}
      <aside class="obs-side" inert={mainBlocked || termOverlaying || undefined}>
        {#if !mainBlocked}
          <ResizeHandle panel="obsSide" edge="start" />
        {/if}
        <div class="obs-side-head" class:tabbed={rightKind === 'plan' && !!planHead}>
          {#if rightKind === 'plan' && planHead}
            {@render planHead()}
          {:else}
            <span class="obs-side-title">{rightMeta.label}</span>
            {@render panelHeadExtras(rightKind)}
          {/if}
        </div>
        <div class="obs-pane-body">
          {@render panelBody(rightKind)}
        </div>
      </aside>
    {/if}
    {#if mainBlocked && mainOverlay}
      <div class="obs-work-overlay">
        {@render mainOverlay()}
      </div>
    {/if}
    </div>
  </div>
  <div class="obs-term-scrim" aria-hidden="true"></div>
</div>

{#snippet panelIcon(key: CodePanelKey)}
  <Icon name={PANEL_ICONS[key]} />
{/snippet}

{#snippet panelHeadExtras(kind: CodePanelKey)}
  {#if kind === 'files'}
    {#if showChangedToggle}
      <div class="obs-file-tools">
        <div class="seg obs-scope" role="group" aria-label="Explorer scope">
          <button
            type="button"
            class:on={!browseAll}
            aria-pressed={!browseAll}
            title="Show files changed since {reviewBase || 'HEAD'}"
            onclick={() => setBrowse(false)}
          >
            Changed
            <span class="cnt">{reviewCount}</span>
          </button>
          <button
            type="button"
            class:on={browseAll}
            aria-pressed={browseAll}
            title="Browse the full repository tree"
            onclick={() => setBrowse(true)}
          >All files</button>
        </div>
      </div>
    {/if}
  {:else if kind === 'scm'}
    {#if isGit && treeWritable}
      <div class="obs-file-tools">
        {@render scmFileListModeSeg()}
        {#if workingDirtyCount}
          <button
            type="button"
            class="obs-icon-btn"
            title="Review uncommitted changes"
            aria-label="Review uncommitted changes"
            onclick={() => openWorkingReview()}
          >
            <Icon name="file-diff" />
          </button>
          <button
            type="button"
            class="obs-icon-btn"
            title="Discard all uncommitted changes"
            aria-label="Discard all"
            onclick={() => discardAllWorking()}
          >
            <Icon name="undo" />
          </button>
        {/if}
      </div>
    {/if}
  {:else if kind === 'findings'}
    <div class="obs-file-tools">
      <div class="seg obs-scope findings-scope" role="group" aria-label="Finding status">
        {#each FINDINGS_FILTERS as f (f.key)}
          <button
            type="button"
            class:on={findingsFilter === f.key}
            aria-pressed={findingsFilter === f.key}
            onclick={() => (findingsFilter = f.key)}
          >
            {f.label}
            {#if findingsFilterCounts[f.key]}
              <span class="cnt">{findingsFilterCounts[f.key]}</span>
            {/if}
          </button>
        {/each}
      </div>
      {#if openFindingsCount > 0 && onFixFindings}
        <button
          type="button"
          class="obs-icon-btn"
          onclick={() => {
            // Surface the Agent dock (and the tab fixFindings will select) so the
            // turn is visible — staying on Findings looks like a no-op.
            void selectCodePanel('maestro')
            onFixFindings()
          }}
          title="Fix open findings ({openFindingsCount})"
          aria-label="Fix open findings"
        >
          <Icon name="wrench" />
        </button>
      {/if}
    </div>
  {/if}
{/snippet}

{#snippet panelBody(kind: CodePanelKey)}
  {#if kind === 'maestro'}
    {#if leftPanel}
      {@render leftPanel(maestroPanelVisible)}
    {/if}
  {:else if kind === 'plan'}
    {#if planPanel}
      {@render planPanel(selectedArtifactId)}
    {/if}
  {:else if kind === 'findings'}
    <FindingsPanel
      {findings}
      filter={findingsFilter}
      busyId={findingBusyId}
      expandIds={findingsExpandIds}
      focusNoteId={findingsFocusNoteId}
      onOpen={(f) => f.file && openPath(f.file, f.line, false, f.endLine || 0)}
      onSetStatus={setFindingStatus}
      onAddNote={addFindingNote}
      onDeleteNote={deleteFindingNote}
    />
  {:else if kind === 'artifacts'}
    <ArtifactsPanel
      workspaceId={workspaceId}
      selectedId={selectedArtifactId}
      onOpen={(a) => { void openArtifact(a) }}
      onRenamed={renameArtifactTab}
      onDeleted={dropArtifactTab}
    />
  {:else if kind === 'files'}
    <div class="obs-files-panel">
      <div class="overlay-sb-shell">
      <div
        class="obs-files"
        bind:this={filesPaneEl}
        tabindex="0"
        role="tree"
        aria-label="File explorer"
        class:drop-into={treeDropDir === ''}
        {@attach overlayScrollbar()}
        {@attach stickyFolderFades('.obs-node.dir')}
        onkeydown={onFilesKeydown}
        ondragover={onTreeDragOver}
        ondragleave={onTreeDragLeave}
        ondrop={onTreeDrop}
        oncontextmenu={(e) => openCtx(e, '', true, '')}
      >
        {#each visibleTree as e (e.path)}
          {@render node(e, 0)}
        {/each}
        {#if draftEdit && draftEdit.mode !== 'rename' && draftEdit.parent === ''}
          {@render draftRow(0)}
        {/if}
        {#if showTreeSkeleton && visibleTree.length === 0}
          <!-- Placeholder rows while this root's reads are in flight. The
               previous worktree's rows are cleared up front, so the alternative
               here is a blank pane. -->
          <div class="obs-tree-skel" aria-hidden="true">
            {#each [70, 52, 61, 44, 58, 48] as w, i}
              <div class="obs-skel-row" style="padding-left:{i === 0 || i === 3 ? 10 : 22}px">
                <span class="skel" style="width:{w}%"></span>
              </div>
            {/each}
          </div>
        {/if}
        {#if !treeLoading && visibleTree.length === 0 && !draftEdit}
          <div class="obs-empty">
            {#if worktreeMissing}
              This workspace's folder no longer exists.
            {:else if filterChanged}
              No changes since base.
            {:else}
              No files — set a repo path on the project.
            {/if}
          </div>
        {:else if worktreeMissing}
          <div class="obs-empty">This workspace's folder no longer exists.</div>
        {/if}
      </div>
      </div>
      <div class="obs-statusbar obs-files-statusbar" aria-label="Explorer tools">
        {#if treeWritable}
          <button
            type="button"
            class="obs-icon-btn"
            title="New File (⌥⌘N)"
            aria-label="New File"
            onclick={() => startNew('file', actionDir)}
          >
            <Icon name="file-plus" />
          </button>
          <button
            type="button"
            class="obs-icon-btn"
            title="New Folder (⇧⌘N)"
            aria-label="New Folder"
            onclick={() => startNew('folder', actionDir)}
          >
            <Icon name="folder-plus" />
          </button>
        {:else}
          <span>Browse only</span>
        {/if}
        <div class="sp"></div>
        {#if workingDirtyCount}
          <button type="button" class="linkish st st-m" onclick={() => selectCodePanel('scm')}>{workingDirtyCount} dirty</button>
        {/if}
        {#if workingDirtyCount && openFindingsCount}
          <span class="sep">·</span>
        {/if}
        {#if openFindingsCount}
          <button
            type="button"
            class="linkish st st-findings"
            onclick={() => selectCodePanel('findings')}
          >{openFindingsCount} finding{openFindingsCount === 1 ? '' : 's'}</button>
        {/if}
      </div>
    </div>
  {:else if kind === 'scm'}
    <div class="obs-scm-panel">
      {#if worktreeMissing}
        <div class="obs-empty pad">This workspace's folder no longer exists.</div>
      {:else if !isGit}
        <div class="obs-empty pad">Not a git repository.</div>
      {:else if !treeWritable}
        <div class="obs-empty pad">Open a loop worktree to stage and commit changes.</div>
      {:else if workingDirtyCount === 0}
        <div class="obs-empty pad">No uncommitted changes.</div>
      {:else}
        <div class="overlay-sb-shell">
        <div class="obs-scm-body" {@attach overlayScrollbar()} {@attach stickyFolderFades('.obs-scm-row.dir')}>
          {#if stagedPaths.length}
            <div class="obs-scm-sec">
              <div class="obs-scm-sec-head">
                <span>Staged</span>
                <span class="cnt">{stagedPaths.length}</span>
                <button type="button" class="linkish" onclick={() => unstageAll()}>Unstage all</button>
              </div>
              <ul class="obs-scm-rows">
                {#if scmFileListMode === 'tree'}
                  {#each capVisibleTree(stagedTree, scmCollapsed, expandedLists.has('staged') ? Infinity : LIST_CAP) as e (`${e.dir ? 'd' : 'f'}:${e.path}`)}
                    {@render scmTreeRow(e, true, 0)}
                  {/each}
                  {@render moreRow('staged', flattenVisibleTree(stagedTree, scmCollapsed).length)}
                {:else}
                  {#each capped('staged', stagedPaths) as p (p)}
                    {@render scmRow(p, true)}
                  {/each}
                  {@render moreRow('staged', stagedPaths.length)}
                {/if}
              </ul>
            </div>
          {/if}
          {#if unstagedPaths.length}
            <div class="obs-scm-sec">
              <div class="obs-scm-sec-head">
                <span>Changes</span>
                <span class="cnt">{unstagedPaths.length}</span>
                <button type="button" class="linkish" onclick={() => stageAll()}>Stage all</button>
              </div>
              <ul class="obs-scm-rows">
                {#if scmFileListMode === 'tree'}
                  {#each capVisibleTree(unstagedTree, scmCollapsed, expandedLists.has('unstaged') ? Infinity : LIST_CAP) as e (`${e.dir ? 'd' : 'f'}:${e.path}`)}
                    {@render scmTreeRow(e, false, 0)}
                  {/each}
                  {@render moreRow('unstaged', flattenVisibleTree(unstagedTree, scmCollapsed).length)}
                {:else}
                  {#each capped('unstaged', unstagedPaths) as p (p)}
                    {@render scmRow(p, false)}
                  {/each}
                  {@render moreRow('unstaged', unstagedPaths.length)}
                {/if}
              </ul>
            </div>
          {/if}
        </div>
        </div>
        <div class="obs-scm-foot">
          <div class="finding-note-compose">
            <textarea
              class="finding-note-input"
              bind:value={commitMsg}
              placeholder="Commit message"
              aria-label="Commit message"
              disabled={commitBusy}
              {@attach autosizeTextarea(commitMsg, { minPx: 56, maxPx: 120 })}
              {@attach emojiShortcodes((v) => { commitMsg = v })}
              onkeydown={(e) => {
                if (e.key === 'Enter' && isCmdOrCtrl(e)) {
                  e.preventDefault()
                  void humanCommit()
                }
              }}
            ></textarea>
            <div class="finding-note-compose-foot">
              <button
                type="button"
                class="obs-icon-btn"
                class:busy={commitMsgBusy}
                disabled={commitMsgBusy || !stagedFiles.length}
                onclick={() => generateCommitMsg()}
                title={commitMsgBusy ? 'Generating…' : (stagedFiles.length ? 'Generate commit message with AI' : 'Stage changes to generate a message')}
                aria-label="Generate commit message with AI"
                aria-busy={commitMsgBusy}
              >
                <Icon name="sparkle" />
              </button>
              <button
                type="button"
                class="nt-go"
                disabled={commitBusy || !stagedFiles.length || !commitMsg.trim()}
                onclick={() => humanCommit()}
                title={stagedFiles.length ? `Commit ${stagedFiles.length} file${stagedFiles.length === 1 ? '' : 's'}` : 'Stage changes to commit'}
              >{commitBusy ? 'Committing…' : 'Commit'}<span class="nt-ret">⌘↵</span></button>
            </div>
          </div>
        </div>
      {/if}
    </div>
  {:else if kind === 'outline'}
    <div class="obs-search-box">
      <button type="button" class="rbtn" style="width:100%" onclick={() => openQuickOpen('symbol')}>Go to Symbol ⌘⇧O</button>
    </div>
    {#each capped('outline', outline) as s}
      <button
        class="obs-sym"
        onclick={() => { scrollToLine(s.line); findRefs(symbolLookupName(s.name, s.kind), s.line) }}
        ondblclick={() => gotoDef(symbolLookupName(s.name, s.kind))}
        title="Click: refs · Double-click: go to definition"
      >
        <span class="k">{s.kind}</span><span class="n">{s.name}</span><span class="ln">{s.line}</span>
      </button>
    {:else}
      <div class="obs-empty">{lspAttach ? 'No symbols from language server' : 'No symbols'}</div>
    {/each}
{@render moreRow('outline', outline.length)}
  {:else if kind === 'problems'}
    <div class="obs-problems">
      <div class="obs-problems-head">
        {#if problemCounts.total}
          <span class="obs-problems-stat err" title="Errors">{problemCounts.errors}</span>
          <span class="obs-problems-stat warn" title="Warnings">{problemCounts.warnings}</span>
          {#if lspAttach}
            <span class="obs-problems-server" title="Language server">{lspAttach.serverKey}</span>
          {/if}
        {:else}
          <span class="obs-problems-empty-label">
            {lspAttach ? 'No problems' : 'Open a file with a language server on PATH'}
          </span>
        {/if}
      </div>
      <div class="obs-problems-list" role="list" aria-label="Problems">
        {#each capped('problems', problemsList) as p ((p.sessionId ?? '') + ':' + p.uri + ':' + p.line + ':' + p.character + ':' + p.message)}
          {@const busyKey = `${p.uri}:${p.line}:${p.message}`}
          {@const fixing = problemFixBusy === busyKey}
          <div class="obs-problem-row" role="listitem" class:busy={fixing}>
            <button
              type="button"
              class="obs-problem-main"
              title={p.message}
              onclick={() => openPath(p.path, p.line)}
            >
              <span class="obs-problem-sev {p.severity}" aria-hidden="true">
                <Icon name={problemSevIcon(p.severity)} />
              </span>
              <span class="obs-problem-body">
                <span class="obs-problem-msg">{p.message}</span>
                <span class="obs-problem-meta">
                  <span class="obs-problem-loc" title={p.path}>{problemLocLabel(p)}</span>
                  {#if p.source}
                    <span class="obs-problem-src">{p.source}</span>
                  {/if}
                </span>
              </span>
            </button>
            <button
              type="button"
              class="obs-problem-fix"
              disabled={!!problemFixBusy}
              onclick={() => applyProblemFix(p)}
              title="Apply quick fix"
            >{fixing ? '…' : 'Fix'}</button>
            <button
              type="button"
              class="obs-problem-fix"
              onclick={() => fixProblemWithAI(p)}
              title="Ask the agent to fix this"
              aria-label="Fix with AI"
            ><Icon name="sparkle" size={12} /></button>
          </div>
        {:else}
          {#if problemCounts.total === 0 && lspAttach}
            <div class="obs-empty">No diagnostics yet</div>
          {/if}
        {/each}
      </div>
      {@render moreRow('problems', problemsList.length)}
    </div>
  {:else if kind === 'search'}
    <div class="obs-search-box">
      <input
        class="obs-search"
        bind:this={searchInput}
        bind:value={query}
        placeholder="Search"
        aria-label="Search repository"
        onkeydown={(e) => e.key === 'Enter' && runSearch(true)}
      />
      <input
        class="obs-search"
        bind:value={replaceText}
        placeholder="Replace"
        aria-label="Replace"
        disabled={!treeWritable}
      />
      <input
        class="obs-search"
        bind:value={searchGlob}
        placeholder="Glob (e.g. *.go)"
        aria-label="File glob"
      />
      <div class="obs-search-opts">
        <label><input type="checkbox" bind:checked={searchRegex} /> Regex</label>
        <label><input type="checkbox" bind:checked={searchCase} /> Case</label>
        <button type="button" class="rbtn" onclick={() => runSearch(true)}>Find</button>
        {#if treeWritable}
          <button type="button" class="rbtn" disabled={!query.trim()} onclick={() => runReplace()}>Replace all</button>
        {/if}
      </div>
    </div>
    {#each hits as h}
      <button class="obs-hit" onclick={() => openPath(h.path, h.line)}>
        <span class="fp">{h.path}:{h.line}</span>
        <span class="pv">{h.preview}</span>
      </button>
    {:else}
      <div class="obs-empty">{query.trim() ? 'No matches' : 'Type a query and press Enter'}</div>
    {/each}
    {#if searchSkippedLarge > 0}
      <div class="obs-empty">
        {searchSkippedLarge} large file{searchSkippedLarge === 1 ? '' : 's'} skipped
      </div>
    {/if}
    {#if searchTruncated}
      <button type="button" class="obs-hit more" onclick={() => runSearch(false)}>Load more…</button>
    {/if}
  {:else if kind === 'refs'}
    {#each capped('refs', refs) as r}
      <button class="obs-hit" onclick={() => openPath(r.path, r.line)}>
        <span class="fp">{r.def ? 'def' : 'ref'} · {r.path}:{r.line}</span>
        <span class="pv">{r.text}</span>
      </button>
    {:else}
      <div class="obs-empty">Pick a symbol from Outline</div>
    {/each}
{@render moreRow('refs', refs.length)}
  {/if}
{/snippet}

{#snippet moreRow(key: string, total: number)}
  {#if total > LIST_CAP && !expandedLists.has(key)}
    <button type="button" class="obs-hit more" onclick={() => expandList(key)}>
      Show all {total}…
    </button>
  {/if}
{/snippet}

{#snippet draftRow(depth: number)}
  {#if draftEdit}
    <div class="obs-node draft" style="padding-left:{10 + depth * 12}px">
      {#if draftEdit.dir}
        <span class="car spacer" aria-hidden="true"></span>
      {:else}
        <span class="fico {fileIconClass(draftEdit.name || 'file', $theme)}" aria-hidden="true"></span>
      {/if}
      <input
        class="obs-rename"
        bind:this={draftInput}
        bind:value={draftEdit.name}
        aria-label={draftEdit.mode === 'rename' ? 'Rename' : draftEdit.mode === 'new-folder' ? 'Folder name' : 'File name'}
        onkeydown={(ev) => {
          if (ev.key === 'Enter') {
            ev.preventDefault()
            void commitDraft()
          } else if (ev.key === 'Escape') {
            ev.preventDefault()
            cancelDraft()
          }
        }}
        onblur={() => void commitDraft()}
      />
    </div>
  {/if}
{/snippet}

{#snippet node(e: TreeEntry, depth: number)}
  <!-- In Changed scope the subtree is derived from paths we already have, so it
       renders expanded by default and needs no exclude/scope re-filtering. -->
  {@const isOpen = filterChanged ? !changedCollapsed.has(e.path) : expanded.has(e.path)}
  {@const isLoading = !filterChanged && loadingDirs.has(e.path)}
  {@const kids = filterChanged ? (e.children ?? []) : visibleEntries(e.children, false)}
  {@const st = entryStatus(e)}
  {@const unsaved = !e.dir && unsavedPaths.has(e.path)}
  {@const findingFlag = findingFlagForPath(e.path, e.dir, openFindingsByPath)}
  {@const problemFlag = problemFlagFor(e.path, e.dir)}
  {@const renaming = draftEdit?.mode === 'rename' && draftEdit.path === e.path}
  {#snippet nodeRow()}
    {#if renaming}
      {@render draftRow(depth)}
    {:else}
      <button
        class="obs-node"
        class:dir={e.dir}
        class:open={e.dir && isOpen}
        class:ignored={e.ignored}
        class:on={treePath === e.path}
        class:cut={fileClip?.mode === 'cut' && fileClip.path === e.path}
        class:deleted={st === 'd' && !e.dir}
        class:drop-into={e.dir && treeDropDir === e.path}
        style="padding-left:{10 + depth * 12}px"
        data-tree-path={e.path}
        draggable
        role="treeitem"
        aria-selected={treePath === e.path}
        aria-expanded={e.dir ? isOpen : undefined}
        onclick={(ev) => onNodeClick(e, ev)}
        ondragstart={(ev) => onTreeDragStart(ev, e.path, e.dir)}
        ondragend={onTreeDragEnd}
        oncontextmenu={(ev) => openCtx(ev, e.path, e.dir, e.name)}
      >
        {#if e.dir}
          {#if isLoading}
            <span class="car load" aria-hidden="true">…</span>
          {:else}
            <Icon class="car" name="chevron-down" strokeWidth={2.4} />
          {/if}
        {:else}
          <span class="fico {fileIconClass(e.name, $theme)}" aria-hidden="true"></span>
        {/if}
        <span class="label-row">
          <span class="label" class:st-m={st === 'm'} class:st-a={st === 'a'} class:st-d={st === 'd'} class:st-u={st === 'u'}>{e.name}</span>
          {#if unsaved}<span class="unsaved" aria-label="unsaved"></span>{/if}
        </span>
        {#if findingFlag.count}
          <span
            class="obs-finding-flag {findingFlag.severity}"
            class:dir={e.dir}
            title="{findingFlag.count} open finding{findingFlag.count === 1 ? '' : 's'}"
            aria-label="{findingFlag.count} open finding{findingFlag.count === 1 ? '' : 's'}"
          >
            <Icon name="hat-glasses" strokeWidth={2.2} />
          </span>
        {/if}
        {#if problemFlag}
          <span
            class="obs-problem-flag"
            class:warn={problemFlag.severity === 'warning'}
            class:info={problemFlag.severity === 'info'}
            class:dir={e.dir}
            title="{problemFlag.errors} error{problemFlag.errors === 1 ? '' : 's'}, {problemFlag.warnings} warning{problemFlag.warnings === 1 ? '' : 's'}"
            aria-label="{problemFlag.count} problem{problemFlag.count === 1 ? '' : 's'}"
          >{problemFlag.errors || problemFlag.warnings || problemFlag.count}</span>
        {/if}
        {#if st && !e.dir}
          <span class="fst {st}" class:wip={!!workingStatus[e.path]} title={statusTitle[st] ?? st}>{st.toUpperCase()}</span>
        {/if}
      </button>
    {/if}
  {/snippet}
  {#if e.dir}
    <div class="obs-branch" style="--d:{depth}">
      {@render nodeRow()}
      {#if isOpen}
        {#each kids as c (c.path)}
          {@render node(c, depth + 1)}
        {:else}
          {#if loadedDirs.has(e.path) && !isLoading && !(draftEdit && draftEdit.parent === e.path && draftEdit.mode !== 'rename')}
            <div class="obs-tree-empty" style="padding-left:{22 + depth * 12}px">empty</div>
          {/if}
        {/each}
        {#if draftEdit && draftEdit.mode !== 'rename' && draftEdit.parent === e.path}
          {@render draftRow(depth + 1)}
        {/if}
      {/if}
    </div>
  {:else}
    {@render nodeRow()}
  {/if}
{/snippet}

{#snippet scmFileListModeSeg()}
  <div class="seg commit-diff-layout" role="group" aria-label="File list mode">
    <button type="button" class:on={scmFileListMode === 'flat'} title="Flat list" aria-label="Flat list" aria-pressed={scmFileListMode === 'flat'} onclick={() => setScmFileListMode('flat')}><Icon name="list" /></button>
    <button type="button" class:on={scmFileListMode === 'tree'} title="Tree" aria-label="Tree" aria-pressed={scmFileListMode === 'tree'} onclick={() => setScmFileListMode('tree')}><Icon name="workflow" /></button>
  </div>
{/snippet}

{#snippet scmTreeRow(e: TreeEntry, staged: boolean, depth: number)}
  {#if e.dir}
    {@const open = !scmCollapsed.has(e.path)}
    {@const files = pathsUnderDir(e.path, staged ? stagedPaths : unstagedPaths)}
    {@const n = files.length}
    <li class="obs-scm-branch" style="--d:{depth}">
      <div class="obs-scm-row dir" class:open={open}>
        <button
          type="button"
          class="stage"
          class:on={staged}
          title={staged ? `Unstage ${n} file${n === 1 ? '' : 's'}` : `Stage ${n} file${n === 1 ? '' : 's'}`}
          aria-label={staged ? `Unstage ${e.path}` : `Stage ${e.path}`}
          onclick={() => staged ? unstagePaths(files) : stagePaths(files)}
        >
          <Icon name={staged ? 'minus' : 'plus'} strokeWidth={2} />
        </button>
        <button
          type="button"
          class="file"
          title={e.path}
          aria-expanded={open}
          onclick={() => toggleScmDir(e.path)}
        >
          <Icon class="car" name="chevron-down" strokeWidth={2.4} />
          <span class="name">{e.name}</span>
        </button>
        <button
          type="button"
          class="discard"
          title="Discard changes"
          aria-label={`Discard ${e.path}`}
          onclick={() => discardPaths(files, e.path)}
        >
          <Icon name="undo" />
        </button>
      </div>
      {#if open}
        <ul class="obs-scm-rows">
          {#each e.children ?? [] as c (`${c.dir ? 'd' : 'f'}:${c.path}`)}
            {@render scmTreeRow(c, staged, depth + 1)}
          {/each}
        </ul>
      {/if}
    </li>
  {:else}
    {@render scmRow(e.path, staged, depth, false)}
  {/if}
{/snippet}

{#snippet scmRow(path: string, staged: boolean, depth = 0, showDir = true)}
  {@const st = workingStatus[path] || '?'}
  <li class="obs-scm-row" style="--d:{depth}">
    <button
      type="button"
      class="stage"
      class:on={staged}
      title={staged ? 'Unstage' : 'Stage'}
      aria-label={staged ? `Unstage ${path}` : `Stage ${path}`}
      onclick={() => staged ? unstagePath(path) : stagePath(path)}
    >
      <Icon name={staged ? 'minus' : 'plus'} strokeWidth={2} />
    </button>
    <button
      type="button"
      class="file"
      title={path}
      onclick={() => openPath(path, 0, true)}
    >
      <span class="st st-{st}">{st.toUpperCase()}</span>
      <span class="name">{path.split('/').pop()}</span>
      {#if showDir && path.includes('/')}<span class="dir">{path.split('/').slice(0, -1).join('/')}</span>{/if}
    </button>
    <button
      type="button"
      class="discard"
      title="Discard changes"
      aria-label={`Discard ${path}`}
      onclick={() => discardPath(path)}
    >
      <Icon name="undo" />
    </button>
  </li>
{/snippet}

{#snippet ctxBtn(label: string, kb: string, action: () => void, disabled = false)}
  <button type="button" role="menuitem" {disabled} onclick={action}>
    <span class="obs-ctx-label">{label}</span>
    {#if kb}<Kbd class="obs-ctx-kbd" text={kb} />{/if}
  </button>
{/snippet}

{#if ctxMenu}
  {@const m = ctxMenu}
  {@const into = m.dir ? m.path : parentPath(m.path)}
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div
    class="obs-ctx"
    style="left:{m.x}px;top:{m.y}px"
    role="menu"
    tabindex="-1"
    onmousedown={(e) => e.stopPropagation()}
  >
    {#if m.tab}
      {@render ctxBtn('Close', '', () => { const p = m.path; closeCtx(); void closeMRU(p) })}
      {@render ctxBtn('Close others', '', () => { const p = m.path; closeCtx(); void closeTabs(openFiles.filter((f) => f !== p)) }, openFiles.length < 2)}
      <div class="obs-ctx-sep"></div>
      {@render ctxBtn('Close all', '', () => { closeCtx(); void closeTabs([...openFiles]) })}
    {:else if treeWritable}
      {@render ctxBtn('New File', '⌥⌘N', () => startNew('file', into))}
      {@render ctxBtn('New Folder', '⇧⌘N', () => startNew('folder', into))}
      <div class="obs-ctx-sep"></div>
      {#if m.path}
        {@render ctxBtn('Cut', '⌘X', () => clipCut(m.path, m.dir))}
        {@render ctxBtn('Copy', '⌘C', () => clipCopy(m.path, m.dir))}
        {@render ctxBtn(
          fileClip ? `Paste “${fileClip.path.split('/').pop()}”` : 'Paste',
          '⌘V',
          () => clipPaste(into),
          !fileClip,
        )}
        {@render ctxBtn('Duplicate', '⌘D', () => duplicatePath(m.path, m.dir))}
        <div class="obs-ctx-sep"></div>
        {@render ctxBtn('Rename…', '↩', () => startRename(m.path, m.name, m.dir))}
        {@render ctxBtn('Delete', '⌘⌫', () => deletePath(m.path, m.name))}
        <div class="obs-ctx-sep"></div>
      {:else if fileClip}
        {@render ctxBtn(`Paste “${fileClip.path.split('/').pop()}”`, '⌘V', () => clipPaste(''))}
        <div class="obs-ctx-sep"></div>
      {/if}
    {:else if m.path}
      {@render ctxBtn('Copy', '⌘C', () => clipCopy(m.path, m.dir))}
      <div class="obs-ctx-sep"></div>
    {/if}
    {#if m.path && !m.tab}
      {@render ctxBtn('Copy Path', '⌥⌘C', () => copyAbsPath(m.path))}
      {@render ctxBtn('Copy Relative Path', '⇧⌥⌘C', () => copyRelPath(m.path))}
      {@render ctxBtn('Reveal in Finder', '⌥⌘R', () => revealPath(m.path))}
      {#if !m.dir}
        {@render ctxBtn('Open', '⌘↓', () => { closeCtx(); void openPath(m.path, 0, filterChanged || gitStatus[m.path] === 'd') })}
        {@render ctxBtn('Open in Editor', '', () => openExternal(m.path))}
        {@render ctxBtn('Open in Split', '', () => { closeCtx(); void loadSplit(m.path) })}
        {#if treeWritable && workingStatus[m.path]}
          <div class="obs-ctx-sep"></div>
          {#if stagedSet.has(m.path)}
            {@render ctxBtn('Unstage', '', () => { closeCtx(); void unstagePath(m.path) })}
          {:else}
            {@render ctxBtn('Stage', '', () => { closeCtx(); void stagePath(m.path) })}
          {/if}
          {@render ctxBtn('Discard Changes', '', () => { closeCtx(); void discardPath(m.path) })}
        {/if}
      {/if}
    {/if}
  </div>
{/if}

<QuickOpen
  bind:open={quickOpen}
  mode={quickOpenMode}
  {projectId}
  {workspaceId}
  symbols={outline}
  onPickFile={(path, line) => openPath(path, line ?? 0, false, 0, 'scroll')}
  onPickSymbol={(_name, line) => scrollToLine(line)}
/>

<CommandPrompts
  open={promptDialog.open}
  title={promptDialog.title}
  prompts={promptDialog.prompts}
  onResolve={(v) => promptDialog.resolve?.(v)}
  onCancel={() => promptDialog.resolve?.(null)}
/>
