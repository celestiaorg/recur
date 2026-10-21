package engine

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"strings"

	"recur/internal/code"
	"recur/internal/domain"
	gitpkg "recur/internal/git"
	"recur/internal/store"
)

// ForkWorkspaceChat branches a coding-agent chat at an exact message (the turn
// footer the UI sends). It opens a new PA* tab whose transcript is the parent's
// messages through that fork point. Every fork starts a fresh
// provider session; the copied transcript is injected on the first send.
func (e *Engine) ForkWorkspaceChat(workspaceID, code, messageID string) domain.WorkspaceChatTab {
	if strings.TrimSpace(messageID) == "" {
		return domain.WorkspaceChatTab{}
	}
	code = strings.TrimSpace(code)
	if code == "" {
		code = pairAgentCode
	}
	if code != maestroAgentCode && !isPairChatCode(code) {
		return domain.WorkspaceChatTab{}
	}
	unlock, ok := e.lockChatLife(workspaceID, code)
	defer unlock()
	if !ok {
		return domain.WorkspaceChatTab{}
	}
	ws, ok := e.Store.Workspace(workspaceID)
	if !ok {
		return domain.WorkspaceChatTab{}
	}
	parent, ok := e.Store.SessionForLoopAgent(workspaceID, code)
	if !ok {
		return domain.WorkspaceChatTab{}
	}

	if !ws.HasRun() && len(e.pairChatAgents(workspaceID)) == 0 {
		e.ensurePairAgent(workspaceID, pairAgentCode, "")
	}
	newCode := e.nextPairChatCode(workspaceID)
	tab, ok := e.materializeChatFork(workspaceID, workspaceID, newCode, parent, messageID)
	if !ok {
		return domain.WorkspaceChatTab{}
	}
	return tab
}

// ForkWorkspaceChatToWorkspace creates a sibling hand-driven workspace in the
// same project and copies this chat through messageID onto its default PA tab.
// The git worktree allocates in the background (same as New workspace), from
// the source branch HEAD, then overlays uncommitted changes.
func (e *Engine) ForkWorkspaceChatToWorkspace(workspaceID, code, messageID string) (domain.Workspace, error) {
	if strings.TrimSpace(messageID) == "" {
		return domain.Workspace{}, fmt.Errorf("fork message is required")
	}
	ws, ok := e.Store.Workspace(workspaceID)
	if !ok {
		return domain.Workspace{}, fmt.Errorf("workspace %q not found", workspaceID)
	}
	code = strings.TrimSpace(code)
	if code == "" {
		code = pairAgentCode
	}
	if code != maestroAgentCode && !isPairChatCode(code) {
		return domain.Workspace{}, fmt.Errorf("cannot fork agent %q", code)
	}
	parent, ok := e.Store.SessionForLoopAgent(workspaceID, code)
	if !ok {
		return domain.Workspace{}, fmt.Errorf("no chat session to fork")
	}
	if !e.Store.SessionContainsMessage(parent.ID, messageID) {
		return domain.Workspace{}, fmt.Errorf("fork message %q is not in this chat", messageID)
	}

	if ws.IsPRChatHost() {
		return domain.Workspace{}, fmt.Errorf("create a workspace from this pull request before forking the chat into a new worktree")
	}
	name := e.chatTabTitle(workspaceID, code)
	if name == "" || name == "New chat" {
		name = strings.TrimSpace(ws.Short)
	}
	if name == "" {
		name = defaultWorkspaceTitle
	}
	start := strings.TrimSpace(ws.Branch)
	if start == "" && !ws.IsMain {
		start = ws.BaseBranch
	}
	// Persist first, copy the session, then allocate. Starting git before the
	// store fork succeeds left rollbackCreating racing startAllocate.
	dest, err := e.persistCreatingWorkspace(ws.ProjectID, name, start)
	if err != nil {
		return domain.Workspace{}, err
	}
	if _, ok := e.materializeChatFork(workspaceID, dest.ID, pairAgentCode, parent, messageID); !ok {
		_ = e.DeleteWorkspace(dest.ID)
		return domain.Workspace{}, fmt.Errorf("could not copy chat into the new workspace")
	}
	e.forkOverlay.Store(dest.ID, ws)
	e.startAllocate(dest)
	if saved, ok := e.Store.Workspace(dest.ID); ok {
		return saved, nil
	}
	return dest, nil
}

// materializeChatFork copies parent through messageID onto destWorkspaceID /
// destCode and marks the child so the first send injects the transcript.
func (e *Engine) materializeChatFork(srcWorkspaceID, destWorkspaceID, destCode string, parent domain.Session, messageID string) (domain.WorkspaceChatTab, bool) {
	var parentCfg agentConfig
	_ = json.Unmarshal([]byte(parent.ConfigJSON), &parentCfg)
	childCfg := parentCfg
	childCfg.Code = destCode
	childCfg.Name = "Agent"
	childCfg.ForkInject = true
	childCfg.NativeHistoryAfter = ""
	if domain.NormalizeChatMode(childCfg.Mode) == domain.ChatModeLoop {
		childCfg.Mode = domain.ChatModeAgent
	}
	cfgJSON, err := json.Marshal(childCfg)
	if err != nil {
		return domain.WorkspaceChatTab{}, false
	}

	var child domain.Session
	if destWorkspaceID == srcWorkspaceID {
		child, err = e.Store.ForkSession(parent.ID, messageID, string(domain.RoleCoder), string(cfgJSON))
	} else {
		child, err = e.Store.ForkSessionToWorkspace(parent.ID, messageID, destWorkspaceID, string(domain.RoleCoder), string(cfgJSON))
	}
	if err != nil {
		return domain.WorkspaceChatTab{}, false
	}

	model := strings.TrimSpace(childCfg.Model)
	e.ensurePairAgent(destWorkspaceID, destCode, model)
	if model != "" {
		e.Store.UpdateAgent(destWorkspaceID, destCode, func(a *domain.Agent) { a.Model = model })
	}

	return domain.WorkspaceChatTab{
		Code:      destCode,
		Title:     e.chatTabTitle(destWorkspaceID, destCode),
		UpdatedAt: child.UpdatedAt,
		Mode:      childCfg.Mode,
	}, true
}

// applyChatFork sanitizes a foreign resume id and, when needed, prepends the
// copied transcript. Returns true when this spec carries the preamble so
// finishChatFork can clear ForkInject only then. A restart turn (resumeTurn)
// with a usable resume id --resumes and leaves ForkInject for a later user
// send.
//
// A Retry runs through the same path so a retry after a provider switch (the
// old turn failed on Cursor/Codex, the human retries on Claude) both drops the
// now-foreign resume id AND replays the transcript — otherwise the new provider
// resumes an id it does not own (Cursor agent-<uuid> crashes claude --resume;
// a Claude↔Codex UUID resumes the wrong thread) or, worse, runs with no
// context at all. The only thing retry must not do is inject a SECOND preamble
// when the replayed wire already carries one — guarded below by scanning the
// prompt for the preamble lead.
func (e *Engine) applyChatFork(workspaceID, code string, spec *AgentSpec, resumeTurn bool) bool {
	if spec == nil {
		return false
	}
	dest := domain.ParseModelRef(spec.Model).Provider
	cleared := false
	if !resumeFitsProvider(spec.ResumeSession, dest) {
		// Cursor agent ids look like agent-<uuid>. Passing one to claude -p
		// --resume fails: "is not a UUID and does not match any session title".
		spec.ResumeSession = ""
		spec.ForkSession = false
		e.clearAgentSession(workspaceID, code)
		cleared = true
	}
	if resumeTurn && spec.ResumeSession != "" {
		return false
	}
	sess, ok := e.Store.SessionForLoopAgent(workspaceID, code)
	if !ok {
		return false
	}
	var cfg agentConfig
	_ = json.Unmarshal([]byte(sess.ConfigJSON), &cfg)
	if !resumeTurn && cfg.ForkInject && spec.ResumeSession != "" {
		// A dock switch between two UUID providers (Claude↔Codex) leaves a
		// resume id that shape alone cannot flag as foreign; ForkInject marks
		// it stale so it is dropped rather than resumed on the wrong provider.
		spec.ResumeSession = ""
		spec.ForkSession = false
		e.clearAgentSession(workspaceID, code)
		cleared = true
	}
	if spec.ResumeSession != "" {
		return false
	}
	if e.settings().UsesNativeHarness(dest) {
		// App-mode harness rebuilds Responses input from the copied transcript.
		return false
	}
	if !cfg.ForkInject && !cleared {
		return false
	}
	if strings.HasPrefix(spec.Prompt, forkPreambleLead) {
		// The wire already OPENS with a fork preamble — a retry replaying a wire
		// that an earlier send injected. Do not prepend a second header (nesting),
		// but still treat this turn as carrying the preamble: ride stdin (the wire
		// can be large — argv would risk ARG_MAX and leak history into the process
		// args) and report injected=true so finishChatFork consumes ForkInject once
		// a session id lands. Otherwise the still-armed flag would make the next
		// ordinary send discard the newly valid session and replay history again.
		// HasPrefix, not Contains: a prompt that merely quotes the lead (this file
		// attached, say) must not be mistaken for an injected wire and skip replay.
		spec.PromptOnStdin = true
		if !cfg.ForkInject {
			_ = e.Store.PatchSessionConfigMerge(sess.ID, `{"forkInject":true}`)
		}
		return true
	}
	msgs, err := e.Store.SessionTranscript(sess.ID)
	if err != nil || len(msgs) == 0 {
		return false
	}
	var ctx *domain.SessionContext
	if c, ok := e.Store.ActiveSessionContext(sess.ID); ok {
		ctx = &c
	}
	preamble := forkHistoryPreambleCtx(msgs, ctx)
	if preamble == "" {
		return false
	}
	if spec.DisplayText == "" {
		spec.DisplayText = spec.Prompt
	}
	spec.Prompt = preamble + spec.Prompt
	spec.PromptOnStdin = true
	if !cfg.ForkInject {
		_ = e.Store.PatchSessionConfigMerge(sess.ID, `{"forkInject":true}`)
	}
	return true
}

// finishChatFork clears ForkInject after a turn that actually carried the
// preamble also recorded a provider session id. A failed or stopped first
// send keeps the flag so the next new message still injects. A restart
// --resume that did not inject must not consume the flag.
func (e *Engine) finishChatFork(workspaceID, code string, injected bool, res Result) {
	if !injected {
		return
	}
	newID := strings.TrimSpace(res.SessionID)
	if a, ok := e.Store.Agent(workspaceID, code); ok && strings.TrimSpace(a.SessionID) != "" {
		newID = a.SessionID
	}
	if newID == "" {
		return
	}
	e.consumeForkInject(workspaceID, code)
}

func (e *Engine) consumeForkInject(workspaceID, code string) {
	sess, ok := e.Store.SessionForLoopAgent(workspaceID, code)
	if !ok {
		return
	}
	var cfg agentConfig
	_ = json.Unmarshal([]byte(sess.ConfigJSON), &cfg)
	if !cfg.ForkInject {
		return
	}
	_ = e.Store.PatchSessionConfigMerge(sess.ID, `{"forkInject":null}`)
}

const forkPreambleLead = "This chat was forked from an earlier conversation."

// forkHistoryPreamble reconstructs a forked transcript so a new provider
// session can continue from the fork point. The body is domain.TranscriptXML
// — every content block except thinking, not just AsText — so tool
// calls/results survive a mid-history fork or a later provider switch
// without replaying another model's chain-of-thought. Tool input/result
// bodies are clipped so one huge Read/Bash payload cannot blow the first
// send; chat prose is left intact. User messages that already carry this
// preamble (a failed first send) are omitted so a retry-as-new-message
// does not nest the header.
//
// When the copied thread has a compaction summary, messages before that
// watermark are dropped and the summary stands in for them. session_context
// / active_context_id is the source of truth. CompactBlock
// on the transcript is only the inline UI chip; it is a fallback when the
// context row is missing (older sessions, or compact with no summary).
func forkHistoryPreamble(msgs []domain.Message) string {
	return forkHistoryPreambleCtx(msgs, nil)
}

func forkHistoryPreambleCtx(msgs []domain.Message, ctx *domain.SessionContext) string {
	opts := []domain.TranscriptOption{domain.TranscriptOmitThinking(), domain.TranscriptClipTools()}
	if root, err := store.DataDir(); err == nil {
		opts = append(opts, domain.TranscriptImageRoot(root))
	}
	body := domain.TranscriptXML(forkReplayMessages(msgs, ctx), opts...)
	if body == "" {
		return ""
	}
	return forkPreambleLead + " Continue from the <transcript> below. Do not re-greet; pick up where it left off.\n\n" + body + "\n"
}

// forkReplayMessages drops prior fork-inject rows, then cuts to the compact
// watermark so the inject is summary + live tail instead of the full
// uncompacted thread.
func forkReplayMessages(msgs []domain.Message, ctx *domain.SessionContext) []domain.Message {
	msgs = skipForkPreambleMessages(msgs)
	if replay, ok := forkReplayFromContext(msgs, ctx); ok {
		return replay
	}
	return forkReplayFromCompactBlocks(msgs)
}

func forkReplayFromContext(msgs []domain.Message, ctx *domain.SessionContext) ([]domain.Message, bool) {
	if ctx == nil || strings.TrimSpace(ctx.Summary) == "" || ctx.LastCompactedMessageID == "" {
		return nil, false
	}
	cut := -1
	for i, m := range msgs {
		if m.ID == ctx.LastCompactedMessageID {
			cut = i
			break
		}
	}
	if cut < 0 {
		return nil, false
	}
	head := msgs[cut]
	block := -1
	var payload domain.CompactPayload
	for j, b := range head.Content {
		if p, ok := b.AsCompact(); ok && p.Status == domain.CompactDone && strings.TrimSpace(p.Summary) != "" {
			block, payload = j, p
		}
	}
	if block >= 0 {
		if block > 0 {
			head.Content = append([]domain.ContentBlock(nil), head.Content[block:]...)
		}
		trigger := payload.Trigger
		if trigger == "" {
			trigger = "auto"
		}
		head.Content[0] = domain.CompactBlock(domain.CompactDone, trigger, ctx.Summary, payload.PreTokens)
		out := make([]domain.Message, 0, 1+len(msgs)-cut-1)
		out = append(out, head)
		return append(out, msgs[cut+1:]...), true
	}
	synth := domain.Message{
		Role: "assistant",
		Content: []domain.ContentBlock{
			domain.CompactBlock(domain.CompactDone, "auto", ctx.Summary, 0),
		},
	}
	out := make([]domain.Message, 0, 1+len(msgs)-cut-1)
	out = append(out, synth)
	return append(out, msgs[cut+1:]...), true
}

func forkReplayFromCompactBlocks(msgs []domain.Message) []domain.Message {
	cutMsg, cutBlock := -1, -1
	for i, m := range msgs {
		for j, b := range m.Content {
			p, ok := b.AsCompact()
			if ok && p.Status == domain.CompactDone && strings.TrimSpace(p.Summary) != "" {
				cutMsg, cutBlock = i, j
			}
		}
	}
	if cutMsg < 0 {
		return msgs
	}
	head := msgs[cutMsg]
	if cutBlock > 0 {
		head.Content = head.Content[cutBlock:]
	}
	if len(head.Content) == 0 {
		return msgs[cutMsg+1:]
	}
	out := make([]domain.Message, 0, 1+len(msgs)-cutMsg-1)
	out = append(out, head)
	return append(out, msgs[cutMsg+1:]...)
}

func skipForkPreambleMessages(msgs []domain.Message) []domain.Message {
	out := make([]domain.Message, 0, len(msgs))
	for _, m := range msgs {
		if messageHasForkPreamble(m) {
			continue
		}
		out = append(out, m)
	}
	return out
}

func messageHasForkPreamble(m domain.Message) bool {
	if m.Role != "user" {
		return false
	}
	for _, b := range m.Content {
		if p, ok := b.AsText(); ok && strings.Contains(p.Text, forkPreambleLead) {
			return true
		}
	}
	return false
}

// resumeFitsProvider reports whether id can be passed to provider as a resume
// handle. Each provider has an explicit allowlist — a foreign id must not
// survive a dock switch (Claude UUID handed to Codex, Cursor agent- to
// Claude, …). Empty id always fits (nothing to resume).
func resumeFitsProvider(id, provider string) bool {
	id = strings.TrimSpace(id)
	if id == "" {
		return true
	}
	switch provider {
	case domain.ProviderCursor:
		return strings.HasPrefix(id, "agent-")
	case domain.ProviderClaudeCode, domain.ProviderCodex:
		// Both persist UUIDs; shape cannot tell them apart. A Claude↔Codex
		// dock switch is handled by rememberPairChatSelection (old provider
		// ≠ dest → drop the id and set ForkInject).
		return !strings.HasPrefix(id, "agent-")
	default:
		return false
	}
}

// copyWorktreeDirty overlays src's dirty/untracked files (and deletions)
// onto dest. Tracked commits already travelled with the branch checkout.
func copyWorktreeDirty(srcRoot, destRoot string) error {
	if srcRoot == "" || destRoot == "" || srcRoot == destRoot {
		return nil
	}
	st := gitpkg.StatusMap(context.Background(), srcRoot)
	var errs []error
	for rel, letter := range st {
		dst, err := code.SafeJoin(destRoot, rel)
		if err != nil {
			errs = append(errs, err)
			continue
		}
		if letter == "d" {
			if err := os.RemoveAll(dst); err != nil && !os.IsNotExist(err) {
				errs = append(errs, err)
			}
			continue
		}
		src, err := code.SafeJoin(srcRoot, rel)
		if err != nil {
			errs = append(errs, err)
			continue
		}
		info, err := os.Lstat(src)
		if os.IsNotExist(err) {
			continue
		}
		if err != nil || info.Mode()&os.ModeSymlink != 0 || info.IsDir() {
			if err != nil {
				errs = append(errs, err)
			}
			continue
		}
		if err := os.MkdirAll(filepath.Dir(dst), 0o755); err != nil {
			errs = append(errs, err)
			continue
		}
		if err := copyFileOverwrite(src, dst, info.Mode()); err != nil {
			errs = append(errs, err)
		}
	}
	return errors.Join(errs...)
}

func copyFileOverwrite(from, to string, mode os.FileMode) error {
	in, err := os.Open(from)
	if err != nil {
		return err
	}
	defer in.Close()
	out, err := os.OpenFile(to, os.O_CREATE|os.O_TRUNC|os.O_WRONLY, mode.Perm())
	if err != nil {
		return err
	}
	_, copyErr := io.Copy(out, in)
	closeErr := out.Close()
	if copyErr != nil {
		return copyErr
	}
	return closeErr
}
