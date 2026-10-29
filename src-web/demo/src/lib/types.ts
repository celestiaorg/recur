// Data shapes returned by the Recur services (from the Go domain types).

export type PR = {
  id: string
  workspaceId: string
  projectId?: string
  group: string
  number: number
  source: PRSource
  url?: string
  headSha?: string
  draft?: boolean
  branch: string
  base: string
  title: string
  summary: SummaryPara[]
  focus: FocusNote
  state: string
  diff: DiffStat
  fileCount: number
  checksOk: boolean
  hasChecks?: boolean
  when: string
  createdAt?: string
  updatedAt?: string
  commits: Commit[]
  files: FileDiff[]
  comments: PRComment[]
  pendingComments?: PendingLineComment[]
  verify: VerifyStep[]
  checks: Check[]
  behindBase?: number
  updatingBase?: string
  impact?: ImpactItem[]
  authorLogin?: string
  participants?: PRParticipant[]
  reviewers?: PRReviewer[]
  viewerLogin?: string
  viewerCanWrite?: boolean
  nodeId?: string
  mergeable?: string
  mergeStateStatus?: string
  reviewDecision?: string
  reviewRequested?: boolean
  attentionReasons?: string[]
  isCrossRepository?: boolean
  allowedMergeMethods?: string[]
  mergeQueueEnabled?: boolean
  mergeQueued?: boolean
}

export type PRComment = {
  id: string
  code: string
  author: string
  authorLogin?: string
  avatarCss: string
  avatarUrl?: string
  time: string
  bodyHtml: string
  reactHtml?: string
  kind?: string
  verb?: string
  bodyMd?: string
  at?: string
  commitSha?: string
  verified?: boolean
  checkStatus?: string
  url?: string
  parentId?: string
  reviewId?: string
  path?: string
  line?: number
  side?: string
  diffHunk?: string
  startLine?: number
  startSide?: string
  outdated?: boolean
  inReplyToId?: string
  threadId?: string
  resolved?: boolean
  nodeId?: string
  reactions?: PRReaction[]
}

export type PRReaction = {
  content: string
  count: number
  mine?: boolean
}

export type Commit = {
  hash: string
  sha?: string
  message: string
  body?: string
  author?: string
  authorRole: string
  agentCode: string
  color: string
  iterLabel: string
  diff: DiffStat
  when: string
  files: FileDiff[]
  avatarUrl?: string
  verified?: boolean
  checkStatus?: string
}

export type FileDiff = {
  path: string
  oldPath?: string
  status: string
  diff: DiffStat
  lines: DiffLine[]
}

export type Finding = {
  id: string
  shortId: string
  workspaceId: string
  iteration: number
  file: string
  line: number
  endLine?: number
  severity: Severity
  message: string
  summary?: string
  status: string
  statusReason?: string
  createdAt?: string
  updatedBy?: string
  updatedAt?: string
  notes?: FindingNote[]
  source?: string
}

export type TreeEntry = {
  name: string
  path: string
  dir: boolean
  status?: string
  ignored?: boolean
  children?: TreeEntry[]
}

export type FileContent = {
  path: string
  language: string
  content: string
  size: number
  writable: boolean
  mime?: string
  mtime?: number
}

export type Project = {
  id: string
  name: string
  pic: string
  color: string
  open: boolean
  idle: boolean
  repoPath: string
  baseBranch: string
  setupCmd: string
  teardownCmd: string
  appURL: string
  runCommands?: RunCommand[]
  pinnedRun?: string
  groupId?: string
  hidden?: boolean
  env?: string
  copyFiles?: string
  hasProjectFile?: boolean
  projectFileError?: string
  writeProjectFile?: boolean
  prOnGithub: boolean
  deleteBranchOnRemove?: boolean
  linearProjects?: string
  mergeMethod?: MergeMethod
  updateBaseMethod?: UpdateBaseMethod
  squashCommitMessage?: SquashCommitMessage
  mergeCommitMessage?: MergeCommitMessage
  disabledSkills?: string[]
  enabledSkills?: string[]
  disabledMCP?: string[]
  enabledMCP?: string[]
  isGit: boolean
}

export type Workspace = {
  scheduledPromptPending?: boolean
  scheduleId?: string
  id: string
  projectId: string
  type?: string
  runStarted?: boolean
  isMain?: boolean
  hostPrId?: string
  issue?: IssueRef | null
  group: Group
  goal: string
  goalDisplay?: string
  short: string
  definitionOfDone: string
  state: WorkspaceState
  statusLabel: string
  phase: string
  iteration: number
  progress: number
  autonomy: AutonomyLevel
  loopId?: string
  policyId?: string
  loopChatCode?: string
  branch: string
  baseBranch?: string
  behindBase: number
  worktreePath: string
  port?: number
  diff: DiffStat
  agentCodes: string[]
  last: string
  lastActivityAt?: number
  lastViewedAt?: number
  seenActivityAt?: number
  elapsedLabel: string
  convergence: ConvPoint[]
  parentId?: string
  parentStepIds?: string[]
  fanOut?: ChildFanOut | null
  parentPhaseId?: string
  fanOutWaves?: number
  handoff?: string
  verify?: VerifyStep[]
  verifyState?: string
  archived: boolean
  testResult?: TestResult | null
  checkpoint?: Checkpoint | null
  plan?: Plan | null
  lastError?: string
  retryable?: boolean
  chatTabOrder?: string[]
}

export type InboxItem = {
  id: string
  group: string
  kind: string
  color: string
  title: string
  sub: string
  time: string
  action: string
  actionClass: string
  unread: boolean
  target: string
  workspaceId?: string
}

export type WorkspaceBrief = {
  workspaceId: string
  projectId?: string
  projectName?: string
  title: string
  branch?: string
  bucket: string
  reasons: string[]
  now: string
  next: string
  lastActivityAt?: number
  lastViewedAt?: number
  seenActivityAt?: number
  changedSinceViewed?: boolean
  quietAge?: string
  lastSummary?: string
  state?: string
  phase?: string
  planSummary?: string
  planDone?: number
  planTotal?: number
  activeAgents?: number
  activeAgentTask?: string
  latestChatCode?: string
  latestChatTitle?: string
  diffAdd?: number
  diffDel?: number
  prId?: string
  prNumber?: number
  isPrHost?: boolean
  primary: BriefAction
  secondary?: BriefAction[]
}

export type ChatItem = {
  role: string
  kind: string
  text?: string
  tool?: string
  toolInput?: string
  tokens?: number
  description?: string
  input?: string
  result?: string
  resultError?: boolean
  resultTruncated?: boolean
  resultPending?: boolean
  durationMs?: number
  messageId?: string
  checkpoint?: boolean
  inputTokens?: number
  cacheCreationTokens?: number
  cacheReadTokens?: number
  contextWindow?: number
  attachments?: ChatImageRef[]
  ts?: number
  toolId?: string
  children?: ChatItem[]
}

export type FeedItem = {
  id: string
  agentCode: string
  name: string
  text: string
  time: string
  kind?: string
  clickAgent?: string
  isDeviation: boolean
  rule?: string
  why?: string
  toolUseId?: string
  toolResult?: string
  isToolError?: boolean
  ts?: number
  chatCode?: string
  card?: string
  data?: string
}

export type WorkspaceChatTab = {
  code: string
  title: string
  updatedAt: number
  mode?: string
  unseen?: boolean
  loopId?: string
  policyId?: string
  loopStarted?: boolean
  promptGroup?: string
}

export type Settings = {
  modelAsk: string
  modelPlan: string
  modelAgent: string
  modelReview: string
  modelLoop: string
  modelWorkspaceTitle?: string
  flagsAsk?: { [_ in string]?: string }
  flagsPlan?: { [_ in string]?: string }
  flagsAgent?: { [_ in string]?: string }
  flagsReview?: { [_ in string]?: string }
  flagsLoop?: { [_ in string]?: string }
  flagsWorkspaceTitle?: { [_ in string]?: string }
  childWorkspaceModel?: string
  bypassPermissions: boolean
  maxLoops: number
  maxAgents: number
  defaultMergeMethod?: MergeMethod
  defaultUpdateBaseMethod?: UpdateBaseMethod
  defaultSquashCommitMessage?: SquashCommitMessage
  defaultMergeCommitMessage?: MergeCommitMessage
  branchNamePrefix?: string | null
  branchPrefixMode?: BranchPrefixMode
  branchPrefixCustom?: string
  cursorApiKey?: string
  cursorBridgePath?: string
  cursorStrictConfig?: boolean
  cursorAttributeCommits?: boolean
  cursorAttributePRs?: boolean
  bundleValues?: { [_ in string]?: string }
  linearApiKey?: string
  linearTeamKey?: string
  preventSleepWhileAgentsRun?: boolean
  showHiddenProjects?: boolean
  queueFollowUps?: boolean
  agentTurnTimeoutMinutes?: number
  scheduledWorkspaceKeep?: number
  codexApiKey?: string
  codexChatGPTAuth?: boolean
  codexBinPath?: string
  codexNetworkAccess?: boolean
  chatGPTAccountId?: string
  chatGPTModels?: CompatibleModel[]
  disabledBundleCommands?: string[]
  disabledBundlePrompts?: string[]
  disabledBundleGrammars?: string[]
  disabledBundleThemes?: string[]
  disabledBundleSnippets?: string[]
  disabledLanguageServers?: string[]
  defaultLoop?: string
  defaultPolicy?: string
  disabledBundleLoops?: string[]
  disabledBundleRoles?: string[]
  disabledBundlePolicies?: string[]
  disabledSkills?: string[]
  mcpServers?: MCPServer[]
  simplify?: boolean | null
  disabledProviders?: string[]
  compatibleProviders?: CompatibleProvider[]
}

export type GrammarInfo = {
  scopeName: string
  id: string
  path: string
  language?: string
  fileTypes?: string[]
  firstLineMatch?: string
  injectTo?: string[]
  embeddedLanguages?: { [_ in string]?: string }
  bundleName: string
  sourceRoot: string
  lineComment?: string
  blockCommentStart?: string
  blockCommentEnd?: string
  increaseIndentPattern?: string
  decreaseIndentPattern?: string
  indentNextLinePattern?: string
  zeroIndentPattern?: string
  disableIndentCorrections?: boolean
  closeBrackets?: string[]
}

export type ThemeInfo = {
  id: string
  name: string
  path: string
  uiTheme: string
  order?: number
  preview: ThemePreviewInfo
  bundleName: string
  sourceRoot: string
}

export type PRSource = "" | "gh" | "local"

export type SummaryPara = {
  kind: string
  html?: string
  items?: string[]
}

export type FocusNote = {
  title: string
  body: string
}

export type DiffStat = {
  add: number
  del: number
}

export type PendingLineComment = {
  id: string
  file: string
  line: number
  side?: string
  startLine?: number
  startSide?: string
  body: string
  findingId?: string
}

export type VerifyStep = {
  html: string
  passed: boolean
  screenshot?: string
}

export type Check = {
  name: string
  detail: string
  passed: boolean
  status?: string
  url?: string
  startedAt?: string
}

export type ImpactItem = {
  kind: string
  name: string
  path: string
  line?: number
  detail: string
  changed: boolean
}

export type PRParticipant = {
  login?: string
  name?: string
  avatarUrl?: string
  avatarCss?: string
  code?: string
}

export type PRReviewer = {
  login?: string
  name?: string
  avatarUrl?: string
  avatarCss?: string
  code?: string
  state: string
}

export type DiffLine = {
  old: string
  new: string
  type: string
  codeHtml: string
  extraHtml?: string
}

export type Severity = "" | "info" | "low" | "medium" | "high" | "critical"

export type FindingNote = {
  id: string
  author: string
  body: string
  createdAt: string
}

export type RunCommand = {
  name: string
  cmd: string
}

export type MergeMethod = "" | "merge" | "squash" | "rebase"

export type UpdateBaseMethod = "" | "merge" | "rebase"

export type SquashCommitMessage = "" | "title" | "title-and-commits" | "title-and-description"

export type MergeCommitMessage = "" | "title" | "title-and-description"

export type IssueRef = {
  source: string
  id: string
  key?: string
}

export type Group = "" | "needs" | "active" | "ready"

export type WorkspaceState = "" | "queued" | "planning" | "coding" | "reviewing" | "verifying" | "pr_open" | "merged" | "paused" | "escalated" | "failed" | "stopped"

export type AutonomyLevel = "" | "cautious" | "balanced" | "aggressive"

export type ConvPoint = {
  iteration: number
  findings: number
  color: string
}

export type ChildFanOut = {
  wave: number
  after?: string[]
  outcome?: string
  note?: string
  chat?: string
  mode?: string
}

export type TestResult = {
  passed: boolean
  summary: string
  when: string
}

export type Checkpoint = {
  state: WorkspaceState
  iteration: number
  at: string
  phaseId?: string
  resumePhaseId?: string
}

export type Plan = {
  summary: string
  body?: string
  steps?: PlanStep[]
  attachments?: PlanAttachment[]
  source?: string
  chatCode?: string
  approved?: boolean
}

export type BriefAction = {
  label: string
  target: string
  kind?: string
}

export type ChatImageRef = {
  id: string
  name?: string
  mime?: string
  url?: string
}

export type BranchPrefixMode = "" | "recur" | "github" | "custom" | "none"

export type CompatibleModel = {
  id: string
  label: string
  contextWindow?: number
  flags?: CompatibleFlagDim[]
}

export type MCPServer = {
  id: string
  name: string
  preset?: string
  enabled: boolean
  transport?: string
  url?: string
  headers?: { [_ in string]?: string }
  command?: string
  args?: string[]
  env?: { [_ in string]?: string }
}

export type CompatibleProvider = {
  id: string
  name: string
  baseUrl: string
  apiKey?: string
  extraHeaders?: { [_ in string]?: string }
  models?: CompatibleModel[]
}

export type ThemePreviewInfo = {
  bg?: string
  fg?: string
  fn?: string
  name?: string
  string?: string
}

export type PlanStep = {
  id: string
  title: string
  status: string
  note?: string
  files?: string[]
  dependsOn?: string[]
  workspace?: string
}

export type PlanAttachment = {
  id: string
  kind: string
  title?: string
  path: string
}

export type CompatibleFlagDim = {
  id: string
  label: string
  wire?: string
  options?: CompatibleFlagOption[]
}

export type CompatibleFlagOption = {
  value: string
  label: string
  default?: boolean
}
