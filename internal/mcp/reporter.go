// Package mcp hosts Recur's MCP server (PRD §8.3, DECISIONS D7): a small HTTP
// server the headless `claude -p` agents connect to. Its tools are NOT a way to
// run code — each tool call becomes a TYPED ENGINE CALLBACK (not prose parsing),
// so structured signals flow back into the engine as events. The engine owns the
// server, so it also enforces rules at the tool boundary (PRD §8.3, §9).
//
// Transport: a minimal, correct hand-rolled JSON-RPC 2.0 handler over the MCP
// "streamable HTTP" transport (POST /<token>/mcp), sufficient for `claude -p` to
// initialize, list tools, and call tools. We hand-roll rather than vendor an SDK
// to keep the dependency surface minimal (pure-Go constraint) and to expose a
// clean callback seam the convergence tests drive through the SAME path the real
// agent uses. See DECISIONS D27.
package mcp

import (
	"context"
	"encoding/json"
	"fmt"

	"recur/internal/agenttools"
	"recur/internal/domain"
)

// Reporter is the engine callback seam (PRD §8.3). Host tools live in
// agenttools; MCP HTTP wraps Execute. Identical to agenttools.Host so
// engineReporter implements both.
type Reporter = agenttools.Host

type Finding = agenttools.Finding
type Evidence = agenttools.Evidence
type EscalationRequest = agenttools.EscalationRequest
type EscalationOption = agenttools.EscalationOption
type AskUserRequest = agenttools.AskUserRequest
type CreateArtifactRequest = agenttools.CreateArtifactRequest
type CreateWorkspaceRequest = agenttools.CreateWorkspaceRequest
type PlanSubmission = agenttools.PlanSubmission
type PlanStepMeta = agenttools.PlanStepMeta
type PermissionAsk = agenttools.PermissionAsk
type PermissionVerdict = agenttools.PermissionVerdict
type TerminalInfo = agenttools.TerminalInfo

// noopReporter discards every callback; used as a safe default so a handler is
// never nil-dereferenced if a caller forgets to set a Reporter.
type noopReporter struct{}

// RequestPermission refuses by default: a Recur without an engine attached has
// no human to ask, and silently allowing would be the unsafe direction.
func (noopReporter) RequestPermission(context.Context, string, string, PermissionAsk) (PermissionVerdict, error) {
	return PermissionVerdict{Reason: "no permission broker is attached"}, nil
}

func (noopReporter) ReportFinding(string, string, Finding) domain.Finding {
	return domain.Finding{}
}
func (noopReporter) Done(string, string, string, string) error { return nil }
func (noopReporter) ListFindings(string, string, string) ([]domain.Finding, error) {
	return nil, nil
}
func (noopReporter) UpdateFinding(string, string, string, string, string) (domain.Finding, error) {
	return domain.Finding{}, nil
}
func (noopReporter) ReportResult(string, string, bool, string)            {}
func (noopReporter) Plan(string, string, string)                          {}
func (noopReporter) Escalate(string, string, EscalationRequest)           {}
func (noopReporter) NoteToolArgs(string, string, string, json.RawMessage) {}
func (noopReporter) AskUser(string, string, AskUserRequest) string {
	return "ask_user unavailable"
}
func (noopReporter) SwitchMode(string, string, string) string { return "ignored" }
func (noopReporter) CreateArtifact(string, string, CreateArtifactRequest) (domain.Artifact, error) {
	return domain.Artifact{}, fmt.Errorf("no artifacts")
}
func (noopReporter) ListArtifacts(string, string) ([]domain.Artifact, error) { return nil, nil }
func (noopReporter) ReadArtifact(string, string, string) (string, error) {
	return "", fmt.Errorf("no artifacts")
}
func (noopReporter) PresentArtifact(string, string, string) string       { return "ignored" }
func (noopReporter) RenameSession(string, string, string, string) string { return "ignored" }
func (noopReporter) RenameWorkspace(string, string, string) string       { return "ignored" }
func (noopReporter) RenameBranch(string, string, string) string          { return "ignored" }
func (noopReporter) OpenPR(string, string, string, string, bool) string  { return "ignored" }
func (noopReporter) MergeChild(string, string, string) string            { return "" }
func (noopReporter) ReadChild(string, string, string) string             { return "" }
func (noopReporter) ArchiveChild(string, string, string) string          { return "" }
func (noopReporter) MessageChild(string, string, string, string) string  { return "" }
func (noopReporter) ReadChildArtifact(string, string, string, string) (string, error) {
	return "", nil
}
func (noopReporter) CreateWorkspace(string, string, CreateWorkspaceRequest) string {
	return "ignored"
}
func (noopReporter) AttachEvidence(string, string, Evidence)                  {}
func (noopReporter) SendBack(string, string, string) string                   { return "ignored" }
func (noopReporter) EditPlan(string, string, string) string                   { return "ignored" }
func (noopReporter) RerunReview(string, string) string                        { return "ignored" }
func (noopReporter) DispatchSpecialist(string, string, string, string) string { return "ignored" }
func (noopReporter) ExtendIterations(string, string, int) string              { return "ignored" }
func (noopReporter) ExtendRounds(string, string, int) string                  { return "ignored" }
func (noopReporter) ReenterPhase(string, string, string) string               { return "ignored" }
func (noopReporter) RunPhase(string, string, string) string                   { return "ignored" }
func (noopReporter) ResumeLoop(string, string) string                         { return "ignored" }
func (noopReporter) SubmitPlan(string, string, PlanSubmission) string         { return "ignored" }
func (noopReporter) UpdatePlanStep(string, string, string, string, string) string {
	return "ignored"
}
func (noopReporter) ListSkills(string, string) ([]domain.AgentSkill, error) { return nil, nil }
func (noopReporter) ActivateSkill(string, string, string) (string, error) {
	return "", fmt.Errorf("no skills")
}
func (noopReporter) ListTerminals(string, string) []TerminalInfo { return nil }
func (noopReporter) ReadTerminal(string, string, string, int) (string, error) {
	return "", fmt.Errorf("no terminals")
}
func (noopReporter) ListWorkspaceBriefs() domain.BriefingSnapshot { return domain.BriefingSnapshot{} }
func (noopReporter) WorkspaceContext(string) (domain.WorkspaceContext, error) {
	return domain.WorkspaceContext{}, fmt.Errorf("no briefing")
}
func (noopReporter) PresentWorkspace(string) string { return "ignored" }
