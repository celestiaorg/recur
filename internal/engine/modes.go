package engine

import (
	"recur/internal/domain"
)

// Chat modes: the mode is a property of the chat
// session, not the workspace. A mode shapes the turn's tools and prompt only —
// permissions are machine-global (Settings.BypassPermissions), so Ask is
// read-only because it allowlists no edit tools and ClaudeRunner disallows the
// edit builtins, not because it runs under a stricter permission mode.

// askTools is the read-only chat tool set: explore + read-only git inspection.
// No Edit/Write/NotebookEdit and no general Bash.
var askTools = append(append([]string{}, readExploreTools...), gitReadTools...)

// reviewTools is Ask's set (explore + scoped git) plus bare Bash for lint/test.
// The scoped git rules stay so those reads are pre-approved and don't prompt
// when BypassPermissions is off. disallowedEditTools removes Edit/Write.
var reviewTools = append(append([]string{}, askTools...), "Bash")

// askAppendPrompt is appended to Claude Code's own system prompt (not replacing
// it) for Ask-mode turns. "Read-only" means the git worktree — artifacts write
// outside it (see artifactsOKAppendPrompt).
const askAppendPrompt = "This chat is in ASK mode: read-only on the git worktree. You may read files, search, " +
	"and run read-only git commands (diff/log/show). Do not modify repository files or attempt edits. " +
	"Answer questions and explain code. If the user asks you to change the repository, tell them " +
	"to switch this chat to Agent mode."

// planAppendPrompt shapes a Plan-mode turn: investigate read-only, produce a
// concrete plan. Structured plan capture (mcp__recur__submit_plan) is wired by
// the engine when available; the prompt asks for it only when the tool is
// allowlisted on the turn.
const planAppendPrompt = "This chat is in PLAN mode: read-only on the git worktree. Investigate the repository " +
	"(read files, search, read-only git) and produce a thorough, concrete implementation plan " +
	"for what the user asks — specific files, functions, steps, edge cases, and a definition " +
	"of done. Do not modify repository files — plan, don't edit."

// planSubmitAppendPrompt extends planAppendPrompt when the submit_plan MCP tool
// is wired: the DETAILED plan must be filed on the tool (not left only in chat)
// so the Plan panel, later agents, approval, and the loop consume it.
const planSubmitAppendPrompt = planAppendPrompt + " When the plan is ready, call " +
	"mcp__recur__submit_plan with: summary (one or two sentence headline), body (the FULL " +
	"detailed plan as markdown — relevant files, approach, edge cases, definition of done; " +
	"other agents execute from body, not from this chat), steps (ordered checklist, " +
	"each one actionable sentence), and attachments (ids of any create_artifact files or " +
	"user images ⟦attachment:id⟧ that belong to the plan). Then give the user a readable " +
	"version in chat. Prefer mcp__recur__submit_plan over any built-in create-plan tool — " +
	"only submit_plan updates Recur's plan panel and is what later agents see."

// skillsToolsAppendPrompt is a short pointer to the skills MCP tools — the
// catalog itself is NOT inlined (token cost); list_skills / activate_skill load
// on demand. User `/skill-name` activations are already injected host-side.
const skillsToolsAppendPrompt = "The following skills provide specialized instructions for specific tasks. " +
	"When a task matches a skill's description, call mcp__recur__list_skills for the catalog, " +
	"then mcp__recur__activate_skill with the skill's name to load its full instructions. " +
	"If the user message already includes <skill_content>, follow that and do not re-activate."

// skillMCPTools are allowlisted wherever workspace chat can use skills.
var skillMCPTools = []string{mcpListSkills, mcpActivateSkill}

// artifactsOKAppendPrompt tells read-only modes that create_artifact is allowed:
// it writes under the workspace data dir, not the git worktree. Without this,
// "do not modify files" makes models refuse HTML reports and similar deliverables.
// Ask also prefers artifacts over a large chat dump. Plan/Review must not: that
// clause would compete with submit_plan / report_finding, which those panels consume.
const artifactsOKAppendPrompt = "You may create standalone artifacts (HTML reports, diagrams, tables) with " +
	"mcp__recur__create_artifact — they are stored under the workspace data dir, not the git worktree."
const artifactsPreferAskAppendPrompt = artifactsOKAppendPrompt +
	" Prefer an HTML artifact over dumping a large report into chat, then call mcp__recur__present_artifact."
const artifactsBesidePlanAppendPrompt = artifactsOKAppendPrompt +
	" Artifacts are in addition to, never instead of, mcp__recur__submit_plan."
const artifactsBesideReviewAppendPrompt = artifactsOKAppendPrompt +
	" Artifacts are in addition to, never instead of, mcp__recur__report_finding."

// interactiveMCPTools are available on PA* chats (not Maestro — Maestro has its own set).
// rename_branch and open_pr are Agent-only: they mutate git / open a remote
// PR and must not appear in read-only modes. Open PR is a prompt action —
// the human already chose it — so open_pr is auto-approved like /commit.
// create/list/read/present_artifact write outside the worktree, so they stay on
// Ask/Plan/Review (same allowlist as Agent, minus git/PR mutations).
var interactiveMCPTools = []string{
	mcpAskUser, mcpSwitchMode,
	mcpCreateArtifact, mcpListArtifacts, mcpReadArtifact, mcpPresentArtifact,
	mcpRenameSession, mcpRenameWorkspace,
	mcpListTerminals, mcpReadTerminal,
}

// agentAppendPrompt is the identity note for Agent-mode turns. Ask/Plan/Review
// each lead with "This chat is in X mode"; without the same for Agent, models
// (especially Cursor, which also has its own Ask/Plan SwitchMode) treat the
// mode-literacy "flip the dock to Agent" clause as applying to them and stall.
const agentAppendPrompt = "This chat is in AGENT mode: you can read and edit. Do not ask the " +
	"user to switch to Agent — you already have edit tools. Proceed with the task. " +
	"Strongly prefer child workspaces for anything beyond a very simple task (one-file tweak, " +
	"one-line fix, short answer). Subagents in Recur are child workspaces: call " +
	"mcp__recur__create_workspace (one child per task, an isolated worktree off this branch). Your turn " +
	"does not wait for it: when the child finishes you get a new turn with its report, then " +
	"read_child, merge_child (land its commits here), or archive_child (close it). " +
	"mcp__recur__message_child (workspace_id + message) redirects a running child, or gives a finished (unmerged) child a new turn instead of replacing it. " +
	"Native subagent, Task, or worktree tools are unavailable in Recur — do not look for them."

// agentChildAppendPrompt is Agent identity for a create_workspace child.
const agentChildAppendPrompt = "This chat is in AGENT mode: you can read and edit. Do not ask the " +
	"user to switch to Agent — you already have edit tools. Proceed with the task. " +
	"You are a child workspace: execute the assigned task here. Native subagent, " +
	"Task, or worktree tools are unavailable in Recur — do not look for them."

const agentChildNoAskPrompt = "Do not call mcp__recur__ask_user — pick a reasonable default and finish the assigned task."

// commentHygieneAppendPrompt is always-on for write-capable turns. Recur does
// not replace Claude/Cursor/Codex system prompts, so this overlay is what
// actually reaches those models. Ask/Plan/Review never get it (no Edit/Write).
const commentHygieneNeedle = "changelogs, reasoning, or internal dialogue"
const commentHygieneAppendPrompt = "Code that explains itself needs no comments. Comment only when the code is genuinely complex or the intent would not be obvious to a future reader. " +
	"Do not put notes, changelogs, reasoning, or internal dialogue in code comments: not line comments, block comments, or new docstrings. " +
	"Restating the next line is not explaining intent. No \"changed\", \"updated to\", \"previously\", \"NOTE:\", \"I considered\", \"per the ticket\". " +
	"If you added one of those in this edit, delete it before you finish. Keep comments that were already in the file unless the user asked to remove them."

// modeLiteracyAppendPrompt briefly explains chat modes and switch_mode rules.
// The no-upgrade clause is keyed on the from-mode (matching switch_mode's tool
// description and switchModeAllowed). A successful switch_mode parks this turn
// and remounts with the destination chatModeShape, so the new tools are live
// without waiting for the next user send. ask_user restraint lives here (PA*
// chats only), not on the shared tool spec that Maestro also sees.
const modeLiteracyAppendPrompt = "Chat modes: Ask (read-only Q&A), Plan (read-only → structured plan), " +
	"Review (read-only review + findings), Agent (read+edit coding). The dock mode is user-controlled. " +
	"You may call mcp__recur__switch_mode only within privilege rules: Agent may switch to ask/plan/review; " +
	"ask/plan/review may switch among each other; never switch ask/plan/review up to Agent — ask the user " +
	"to flip the dock instead. A successful switch_mode remounts this turn immediately with the new tools. " +
	"Do not use Cursor's built-in SwitchMode, do not ask the user to switch, and do not keep working with " +
	"the old toolset after the call. Prefer acting on reasonable defaults; call mcp__recur__ask_user only when " +
	"blocked on a decision only the human can make (ends this turn; answer arrives later — not escalate). " +
	"Never use Claude's AskUserQuestion — it cannot collect answers in Recur; use mcp__recur__ask_user."

// firstWorkspaceRenamePrompt is appended on the first PA* message in a workspace
// so the agent names the workspace, branch, and chat via MCP instead of a
// separate auto-rename turn.
const firstWorkspaceRenamePrompt = "This is the first session in this workspace. After you understand the request, " +
	"call mcp__recur__rename_workspace (short title, at most 8 words, sentence case), " +
	"mcp__recur__rename_branch (kebab-case topic, e.g. add-dark-mode), and " +
	"mcp__recur__rename_session (3–5 word chat title, sentence case). No confirmation."

// firstWorkspaceRenamePromptNoBranch is the Ask/Plan/Review variant: those modes
// do not allowlist rename_branch (git mutation).
const firstWorkspaceRenamePromptNoBranch = "This is the first session in this workspace. After you understand the request, " +
	"call mcp__recur__rename_workspace (short title, at most 8 words, sentence case) and " +
	"mcp__recur__rename_session (3–5 word chat title, sentence case). No confirmation."

// firstChatRenamePrompt is appended on the first message of a later chat tab.
const firstChatRenamePrompt = "This is the first message in this chat. Call mcp__recur__rename_session once " +
	"with a 3–5 word topic title in sentence case. No confirmation."

// reviewAppendPrompt shapes a Review-mode turn: a read-only review pass that
// files structured findings via the Recur MCP.
const reviewAppendPrompt = "This chat is in REVIEW mode: read-only on the git worktree. Review what the user " +
	"points you at (default: the looking-at subject in the surface note — a worktree vs " +
	"its base, or the open pull request). You may run Bash for lint/test/typecheck. For EACH real problem — " +
	"a bug, a missing case, a regression, a quality issue — call the mcp__recur__report_finding " +
	"tool with the file, line (optional end_line for a range), severity " +
	"(info/low/medium/high/critical), a clear message, and optional summary (markdown shown " +
	"to the end user in the turn digest; omit to use message). On a re-review, call " +
	"mcp__recur__update_finding to mark a prior finding resolved (fixed) or invalid " +
	"(wrong/out of scope — reason required). Do NOT restate or list filed " +
	"findings in your closing reply — the app summarizes still-open filings for the user; " +
	"keep closing prose brief (e.g. \"Filed N findings\" or \"No issues\"). If the code is " +
	"sound, say so and file no findings. Do not modify repository files — review, don't fix."

// chatTurnShape is how one chat mode shapes an agent turn.
type chatTurnShape struct {
	tools        []string
	permission   string // --permission-mode for the turn
	appendPrompt string // --append-system-prompt (keeps the provider's own prompt)
	mcp          bool   // wire the per-agent Recur MCP endpoint into the turn
}

// chatModeShape maps a normalized chat mode onto its turn shape. The permission
// mode is machine-global (Settings) and identical for every mode — a chat mode
// shapes which tools the turn gets, not whether permissions are enforced.
// ChatModeLoop never reaches here. child is true when the workspace has a parent.
func chatModeShape(mode string, set domain.Settings, child bool) chatTurnShape {
	permission := set.PermissionMode()
	interactive := interactiveMCPTools
	if child {
		interactive = omitTool(interactiveMCPTools, mcpAskUser)
	}
	switch domain.NormalizeChatMode(mode) {
	case domain.ChatModeAsk:
		return chatTurnShape{
			// list_findings is read-only; any MCP-wired chat may list open rows.
			tools: append(append(append(append([]string{}, askTools...), mcpListFindings),
				skillMCPTools...), interactive...),
			permission:   permission,
			appendPrompt: joinAppendPrompt(joinAppendPrompt(joinAppendPrompt(askAppendPrompt, artifactsPreferAskAppendPrompt), skillsToolsAppendPrompt), modeLiteracyAppendPrompt),
			// Ask needs MCP for skills tools and for permission_request when bypass is off.
			mcp: true,
		}
	case domain.ChatModePlan:
		return chatTurnShape{
			tools: append(append(append(append([]string{}, askTools...), mcpSubmitPlan, mcpListFindings),
				skillMCPTools...), interactive...),
			permission:   permission,
			appendPrompt: joinAppendPrompt(joinAppendPrompt(joinAppendPrompt(planSubmitAppendPrompt, artifactsBesidePlanAppendPrompt), skillsToolsAppendPrompt), modeLiteracyAppendPrompt),
			mcp:          true,
		}
	case domain.ChatModeReview:
		return chatTurnShape{
			tools: append(append(append(append([]string{}, reviewTools...), mcpReportFinding, mcpListFindings, mcpUpdateFinding),
				skillMCPTools...), interactive...),
			permission:   permission,
			appendPrompt: joinAppendPrompt(joinAppendPrompt(joinAppendPrompt(reviewAppendPrompt, artifactsBesideReviewAppendPrompt), skillsToolsAppendPrompt), modeLiteracyAppendPrompt),
			mcp:          true,
		}
	default: // agent — coding chat + findings checklist MCP (list/update).
		// No done: that signal is for loop specialists (Coder/Tester) to hand
		// back to the Maestro. Interactive PA* chats talk to a human — calling
		// done is meaningless and models keep doing it when the tool is visible.
		// No findings append prompt: a checklist note made models call
		// list_findings on every new turn. Tools stay allowlisted; the Fix
		// findings CTA sends the checklist as the user turn
		// (list → fix/dispute → update_finding). Skills tip stays — short and
		// load-on-demand via list_skills / activate_skill. Agent identity note
		// must lead — without it, mode literacy's dock-flip clause confuses models.
		tools := append(append(append(append([]string{}, pairTools...),
			mcpListFindings, mcpUpdateFinding), skillMCPTools...), interactive...)
		tools = append(tools, mcpRenameBranch)
		agentPrompt := agentAppendPrompt
		appendPrompt := ""
		if child {
			agentPrompt = agentChildAppendPrompt
			appendPrompt = joinAppendPrompt(joinAppendPrompt(joinAppendPrompt(agentPrompt, skillsToolsAppendPrompt),
				modeLiteracyAppendPrompt), agentChildNoAskPrompt)
		} else {
			tools = append(tools, mcpOpenPR, mcpCreateWorkspace, mcpMergeChild, mcpReadChild, mcpArchiveChild, mcpMessageChild)
			appendPrompt = joinAppendPrompt(joinAppendPrompt(agentPrompt, skillsToolsAppendPrompt),
				modeLiteracyAppendPrompt)
		}
		return chatTurnShape{
			tools:        tools,
			permission:   permission,
			appendPrompt: appendPrompt,
			mcp:          true,
		}
	}
}

func omitTool(tools []string, name string) []string {
	out := make([]string, 0, len(tools))
	for _, t := range tools {
		if t != name {
			out = append(out, t)
		}
	}
	return out
}
