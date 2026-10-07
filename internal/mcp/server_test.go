package mcp

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"net/http"
	"strings"
	"sync"
	"testing"
	"time"

	"recur/internal/domain"
)

// captureReporter records every typed callback so tests can assert the MCP
// handler turns a tools/call into the right engine callback (NOT prose parsing).
// Safe for concurrent use (the server may call it from many goroutines).
type captureReporter struct {
	mu          sync.Mutex
	findings    []recordedFinding
	dones       []recorded
	results     []recordedResult
	plans       []recorded
	escalations []recordedEscalation
	evidence    []recordedEvidence
	sendBacks   []recorded
	planEdits   []recorded
	reruns      []recorded
	dispatches  []recorded
	extensions  []recordedResult
	resumes     []recorded

	submittedPlans []recorded
	doneNotes      []string
	lastPlan       PlanSubmission
	stepUpdates    []recorded

	asks []recordedAsk
	// onAsk answers a permission request. Nil means allow immediately; a test
	// that needs to observe blocking supplies one that waits.
	onAsk func(context.Context, PermissionAsk) (PermissionVerdict, error)

	renameOwner string
}

type recordedAsk struct {
	workspaceID, agentCode string
	ask                    PermissionAsk
}

func (c *captureReporter) RequestPermission(ctx context.Context, workspaceID, agentCode string, ask PermissionAsk) (PermissionVerdict, error) {
	c.mu.Lock()
	c.asks = append(c.asks, recordedAsk{workspaceID, agentCode, ask})
	fn := c.onAsk
	c.mu.Unlock()
	if fn == nil {
		return PermissionVerdict{Allow: true}, nil
	}
	return fn(ctx, ask)
}

type recordedEvidence struct {
	workspaceID, agentCode string
	e                      Evidence
}

type recordedEscalation struct {
	workspaceID, agentCode string
	req                    EscalationRequest
}

type recordedFinding struct {
	workspaceID, agentCode string
	f                      Finding
}
type recorded struct{ workspaceID, agentCode, summary string }
type recordedResult struct {
	workspaceID, agentCode string
	passed                 bool
	summary                string
}

func (c *captureReporter) ReportFinding(workspaceID, agentCode string, f Finding) domain.Finding {
	c.mu.Lock()
	defer c.mu.Unlock()
	c.findings = append(c.findings, recordedFinding{workspaceID, agentCode, f})
	return domain.Finding{
		ID:       "F-test-" + f.File,
		File:     f.File,
		Line:     f.Line,
		EndLine:  f.EndLine,
		Severity: f.Severity,
		Message:  f.Message,
		Summary:  f.Summary,
		Status:   string(domain.FindingOpen),
	}
}
func (c *captureReporter) Done(workspaceID, agentCode, summary, notes string) error {
	c.mu.Lock()
	defer c.mu.Unlock()
	c.dones = append(c.dones, recorded{workspaceID, agentCode, summary})
	c.doneNotes = append(c.doneNotes, notes)
	return nil
}
func (c *captureReporter) ListFindings(workspaceID, agentCode, status string) ([]domain.Finding, error) {
	return nil, nil
}
func (c *captureReporter) UpdateFinding(workspaceID, agentCode, findingID, status, reason string) (domain.Finding, error) {
	return domain.Finding{ID: findingID, Status: status, StatusReason: reason}, nil
}
func (c *captureReporter) ReportResult(workspaceID, agentCode string, passed bool, summary string) {
	c.mu.Lock()
	defer c.mu.Unlock()
	c.results = append(c.results, recordedResult{workspaceID, agentCode, passed, summary})
}
func (c *captureReporter) Plan(workspaceID, agentCode, summary string) {
	c.mu.Lock()
	defer c.mu.Unlock()
	c.plans = append(c.plans, recorded{workspaceID, agentCode, summary})
}
func (c *captureReporter) Escalate(workspaceID, agentCode string, req EscalationRequest) {
	c.mu.Lock()
	defer c.mu.Unlock()
	c.escalations = append(c.escalations, recordedEscalation{workspaceID, agentCode, req})
}
func (c *captureReporter) AttachEvidence(workspaceID, agentCode string, e Evidence) {
	c.mu.Lock()
	defer c.mu.Unlock()
	c.evidence = append(c.evidence, recordedEvidence{workspaceID, agentCode, e})
}
func (c *captureReporter) SendBack(workspaceID, agentCode, instruction string) string {
	c.mu.Lock()
	defer c.mu.Unlock()
	c.sendBacks = append(c.sendBacks, recorded{workspaceID, agentCode, instruction})
	return "Sent back to the loop."
}
func (c *captureReporter) EditPlan(workspaceID, agentCode, plan string) string {
	c.mu.Lock()
	defer c.mu.Unlock()
	c.planEdits = append(c.planEdits, recorded{workspaceID, agentCode, plan})
	return "Plan updated."
}
func (c *captureReporter) RerunReview(workspaceID, agentCode string) string {
	c.mu.Lock()
	defer c.mu.Unlock()
	c.reruns = append(c.reruns, recorded{workspaceID, agentCode, ""})
	return "Reviewer dispatched."
}
func (c *captureReporter) DispatchSpecialist(workspaceID, agentCode, role, instruction string) string {
	c.mu.Lock()
	defer c.mu.Unlock()
	c.dispatches = append(c.dispatches, recorded{workspaceID, agentCode, role + ": " + instruction})
	return "Specialist dispatched."
}
func (c *captureReporter) ResumeLoop(workspaceID, agentCode string) string {
	c.mu.Lock()
	defer c.mu.Unlock()
	c.resumes = append(c.resumes, recorded{workspaceID, agentCode, ""})
	return "Loop restarting."
}
func (c *captureReporter) ExtendIterations(workspaceID, agentCode string, n int) string {
	c.mu.Lock()
	defer c.mu.Unlock()
	c.extensions = append(c.extensions, recordedResult{workspaceID, agentCode, true, fmt.Sprintf("%d", n)})
	return "Extension granted."
}
func (c *captureReporter) ExtendRounds(workspaceID, agentCode string, n int) string {
	return c.ExtendIterations(workspaceID, agentCode, n)
}
func (c *captureReporter) ReenterPhase(workspaceID, agentCode, phaseID string) string {
	return "Reentered " + phaseID
}
func (c *captureReporter) RunPhase(workspaceID, agentCode, phaseID string) string {
	return "Running " + phaseID
}
func (c *captureReporter) SubmitPlan(workspaceID, agentCode string, p PlanSubmission) string {
	c.mu.Lock()
	defer c.mu.Unlock()
	c.submittedPlans = append(c.submittedPlans, recorded{workspaceID, agentCode,
		p.Summary + " | " + strings.Join(p.Steps, "; ")})
	c.lastPlan = p
	return "Plan recorded."
}

func TestSubmitPlanObjectSteps(t *testing.T) {
	rep := &captureReporter{}
	s := New(rep)
	b := binding{workspaceID: "L3", agentCode: "MA"}
	res := call(t, s, b, toolSubmitPlan, map[string]any{
		"summary": "Importer", "body": "Parser and writer.",
		"steps": []any{
			map[string]any{"title": "Write the parser", "files": []string{"parser.go", " "}},
			"Plain step",
			map[string]any{"title": "Wire it", "files": []string{"main.go"}, "depends_on": []string{"S1", "S2"}},
			map[string]any{"title": "   "},
		},
	})
	if res.IsError {
		t.Fatalf("submit_plan object steps should succeed, got %+v", res)
	}
	p := rep.lastPlan
	if strings.Join(p.Steps, ";") != "Write the parser;Plain step;Wire it" {
		t.Fatalf("steps = %v", p.Steps)
	}
	if len(p.StepMeta) != 3 || strings.Join(p.StepMeta[0].Files, ",") != "parser.go" || len(p.StepMeta[1].Files) != 0 ||
		strings.Join(p.StepMeta[2].DependsOn, ",") != "S1,S2" {
		t.Fatalf("step meta = %+v", p.StepMeta)
	}
	if res := call(t, s, b, toolSubmitPlan, map[string]any{
		"summary": "bad", "body": "x", "steps": []any{map[string]any{"name": "wrong key"}},
	}); !res.IsError {
		t.Fatal("an object step without title should be a tool error")
	}
}
func (c *captureReporter) UpdatePlanStep(workspaceID, agentCode, stepID, status, note string) string {
	c.mu.Lock()
	defer c.mu.Unlock()
	c.stepUpdates = append(c.stepUpdates, recorded{workspaceID, agentCode, stepID + "→" + status})
	return "Step updated."
}
func (c *captureReporter) ListSkills(workspaceID, agentCode string) ([]domain.AgentSkill, error) {
	return []domain.AgentSkill{{
		Name: "demo-skill", Description: "A demo", Scope: "user",
		Path: "/tmp/demo-skill/SKILL.md",
	}}, nil
}
func (c *captureReporter) ActivateSkill(workspaceID, agentCode, name string) (string, error) {
	if name != "demo-skill" {
		return "", fmt.Errorf("unknown skill %q", name)
	}
	return `<skill_content name="demo-skill">demo body</skill_content>`, nil
}
func (c *captureReporter) ListTerminals(workspaceID, agentCode string) []TerminalInfo {
	return []TerminalInfo{{ID: "term-1", Title: "zsh", Cwd: "/tmp", Active: true}}
}
func (c *captureReporter) ReadTerminal(workspaceID, agentCode, id string, tailLines int) (string, error) {
	_ = workspaceID
	_ = agentCode
	_ = tailLines
	if id != "" && id != "term-1" {
		return "", fmt.Errorf("terminal %q not found", id)
	}
	return "error: boom\n", nil
}
func (c *captureReporter) NoteToolArgs(workspaceID, agentCode, toolName string, args json.RawMessage) {
}
func (c *captureReporter) AskUser(workspaceID, agentCode string, req AskUserRequest) string {
	return "Question delivered to the user. Stop now and wait."
}
func (c *captureReporter) SwitchMode(workspaceID, agentCode, mode string) string {
	return "Switched to " + mode
}
func (c *captureReporter) CreateArtifact(workspaceID, agentCode string, req CreateArtifactRequest) (domain.Artifact, error) {
	return domain.Artifact{ID: "a1", WorkspaceID: workspaceID, Title: req.Title, Kind: domain.ArtifactKind(req.Kind)}, nil
}
func (c *captureReporter) ListArtifacts(workspaceID, agentCode string) ([]domain.Artifact, error) {
	return nil, nil
}
func (c *captureReporter) ReadArtifact(workspaceID, agentCode, artifactID string) (string, error) {
	if artifactID == "" || artifactID == "missing" {
		return "", fmt.Errorf("artifact not found: %s", artifactID)
	}
	return `{"id":"` + artifactID + `","title":"Notes","kind":"md","content":"# hi\n"}`, nil
}
func (c *captureReporter) PresentArtifact(workspaceID, agentCode, artifactID string) string {
	return "Presented " + artifactID
}
func (c *captureReporter) RenameSession(workspaceID, agentCode, ownerChat, title string) string {
	c.mu.Lock()
	c.renameOwner = ownerChat
	c.mu.Unlock()
	return "Renamed to " + title
}
func (c *captureReporter) RenameWorkspace(workspaceID, agentCode, title string) string {
	return "Renamed workspace to " + title
}
func (c *captureReporter) RenameBranch(workspaceID, agentCode, name string) string {
	return "Renamed branch to " + name
}
func (c *captureReporter) MergeChild(string, string, string) string   { return "merged" }
func (c *captureReporter) ReadChild(string, string, string) string    { return "report" }
func (c *captureReporter) ArchiveChild(string, string, string) string { return "archived" }
func (c *captureReporter) MessageChild(string, string, string, string) string {
	return "steered"
}
func (c *captureReporter) ReadChildArtifact(string, string, string, string) (string, error) {
	return "{}", nil
}
func (c *captureReporter) CreateWorkspace(string, string, CreateWorkspaceRequest) string {
	return "created"
}

func (c *captureReporter) OpenPR(workspaceID, agentCode, title, body string, draft bool) string {
	if draft {
		return "Opened draft PR: " + title
	}
	return "Opened PR: " + title
}
func (c *captureReporter) ListWorkspaceBriefs() domain.BriefingSnapshot {
	return domain.BriefingSnapshot{
		Counts:     domain.BriefingCounts{Needs: 1},
		Workspaces: []domain.WorkspaceBrief{{WorkspaceID: "WS-1", Title: "Needs a look", Bucket: domain.BriefBucketNeeds}},
	}
}
func (c *captureReporter) WorkspaceContext(id string) (domain.WorkspaceContext, error) {
	if id == "" {
		return domain.WorkspaceContext{}, fmt.Errorf("workspace_id is required")
	}
	return domain.WorkspaceContext{Brief: domain.WorkspaceBrief{WorkspaceID: id, Title: "ctx"}}, nil
}
func (c *captureReporter) PresentWorkspace(target string) string {
	b, _ := json.Marshal(domain.PresentWorkspaceResult{OK: true, Target: target})
	return string(b)
}

func (c *captureReporter) snapshot() (int, int, int, int) {
	c.mu.Lock()
	defer c.mu.Unlock()
	return len(c.findings), len(c.dones), len(c.results), len(c.plans)
}

// call dispatches a tools/call directly through dispatch (no HTTP) so the
// protocol framing + handler are tested in isolation.
func call(t *testing.T, s *Server, b binding, name string, args any) callToolResult {
	t.Helper()
	return callCtx(t, context.Background(), s, b, name, args)
}

// callCtx is call with a caller-supplied context, for the one tool that blocks
// on a human and therefore has to be cancellable.
func callCtx(t *testing.T, ctx context.Context, s *Server, b binding, name string, args any) callToolResult {
	t.Helper()
	raw, _ := json.Marshal(args)
	params, _ := json.Marshal(callToolParams{Name: name, Arguments: raw})
	req := rpcRequest{JSONRPC: "2.0", ID: json.RawMessage(`1`), Method: "tools/call", Params: params}
	resp, isNotif := s.dispatch(ctx, req, b)
	if isNotif {
		t.Fatalf("tools/call should not be a notification")
	}
	if resp.Error != nil {
		t.Fatalf("tools/call %s returned JSON-RPC error: %+v", name, resp.Error)
	}
	res, ok := resp.Result.(callToolResult)
	if !ok {
		t.Fatalf("tools/call %s result is not a callToolResult: %T", name, resp.Result)
	}
	return res
}

func TestReportFindingFiresTypedCallback(t *testing.T) {
	rep := &captureReporter{}
	s := New(rep)
	b := binding{workspaceID: "L1", agentCode: "RV"}

	res := call(t, s, b, toolReportFinding, map[string]any{
		"file": "main.go", "line": 42, "severity": "high", "message": "nil deref",
		"summary": "nil deref on boot path",
	})
	if res.IsError {
		t.Fatalf("expected success result, got error: %+v", res)
	}
	if nf, _, _, _ := rep.snapshot(); nf != 1 {
		t.Fatalf("expected 1 finding recorded, got %d", nf)
	}
	got := rep.findings[0]
	if got.workspaceID != "L1" || got.agentCode != "RV" {
		t.Errorf("finding attribution: got loop=%q agent=%q, want L1/RV", got.workspaceID, got.agentCode)
	}
	if got.f.File != "main.go" || got.f.Line != 42 || got.f.Severity != domain.SevHigh || got.f.Message != "nil deref" {
		t.Errorf("finding payload mismatch: %+v", got.f)
	}
	if got.f.Summary != "nil deref on boot path" {
		t.Errorf("summary: got %q", got.f.Summary)
	}
	var out struct {
		ID      string `json:"id"`
		File    string `json:"file"`
		Summary string `json:"summary"`
		Status  string `json:"status"`
	}
	if err := json.Unmarshal([]byte(res.Content[0].Text), &out); err != nil {
		t.Fatalf("result JSON: %v (%q)", err, res.Content[0].Text)
	}
	if out.ID == "" || out.File != "main.go" || out.Summary != "nil deref on boot path" || out.Status != "open" {
		t.Errorf("result payload: %+v", out)
	}
}

func TestReportFindingRange(t *testing.T) {
	rep := &captureReporter{}
	s := New(rep)
	b := binding{workspaceID: "L1", agentCode: "RV"}

	res := call(t, s, b, toolReportFinding, map[string]any{
		"file": "main.go", "line": 10, "end_line": 20, "severity": "medium", "message": "span",
	})
	if res.IsError {
		t.Fatalf("expected success, got %+v", res)
	}
	got := rep.findings[0].f
	if got.Line != 10 || got.EndLine != 20 {
		t.Fatalf("range: got line=%d end=%d", got.Line, got.EndLine)
	}
}

func TestDoneAndReportResultAndPlan(t *testing.T) {
	rep := &captureReporter{}
	s := New(rep)
	b := binding{workspaceID: "L2", agentCode: "CO"}

	call(t, s, b, toolDone, map[string]any{"summary": "implemented feature", "notes": "S2 was too big — split it"})
	call(t, s, b, toolReportResult, map[string]any{"passed": true, "summary": "all green"})
	call(t, s, binding{workspaceID: "L2", agentCode: "MA"}, toolPlan, map[string]any{"summary": "do X then Y"})

	_, nd, nr, np := rep.snapshot()
	if nd != 1 || nr != 1 || np != 1 {
		t.Fatalf("expected 1 done/1 result/1 plan, got %d/%d/%d", nd, nr, np)
	}
	if rep.dones[0].summary != "implemented feature" || rep.dones[0].agentCode != "CO" {
		t.Errorf("done mismatch: %+v", rep.dones[0])
	}
	if rep.doneNotes[0] != "S2 was too big — split it" {
		t.Errorf("done notes = %q, want the handoff", rep.doneNotes[0])
	}
	if !rep.results[0].passed || rep.results[0].summary != "all green" {
		t.Errorf("result mismatch: %+v", rep.results[0])
	}
	if rep.plans[0].summary != "do X then Y" {
		t.Errorf("plan mismatch: %+v", rep.plans[0])
	}
}

func TestReportResultFalseRecorded(t *testing.T) {
	rep := &captureReporter{}
	s := New(rep)
	call(t, s, binding{workspaceID: "L", agentCode: "TE"}, toolReportResult, map[string]any{"passed": false, "summary": "tests failed"})
	if _, _, nr, _ := rep.snapshot(); nr != 1 {
		t.Fatalf("expected 1 result, got %d", nr)
	}
	if rep.results[0].passed {
		t.Error("expected passed=false")
	}
}

// TestEscalateFiresTypedCallback covers the escalate MCP tool (PRD §8.3, §13): a
// well-formed escalate call turns into a typed Reporter.Escalate with the question,
// context, and reasoned options attributed to the right loop+agent.
func TestEscalateFiresTypedCallback(t *testing.T) {
	rep := &captureReporter{}
	s := New(rep)
	b := binding{workspaceID: "L9", agentCode: "MA"}

	res := call(t, s, b, toolEscalate, map[string]any{
		"question": "When a client goes over the rate limit, what should happen?",
		"context":  "The ticket doesn't define over-limit behavior; I won't guess on the public contract.",
		"options": []map[string]any{
			{"text": "Reject with 429", "tag": "simplest", "desc": "Clear but harsh."},
			{"text": "Token bucket", "tag": "recommended", "desc": "Fair, standard."},
		},
	})
	if res.IsError {
		t.Fatalf("expected success, got error: %+v", res)
	}
	rep.mu.Lock()
	defer rep.mu.Unlock()
	if len(rep.escalations) != 1 {
		t.Fatalf("expected 1 escalation recorded, got %d", len(rep.escalations))
	}
	got := rep.escalations[0]
	if got.workspaceID != "L9" || got.agentCode != "MA" {
		t.Errorf("escalation attribution: got loop=%q agent=%q, want L9/MA", got.workspaceID, got.agentCode)
	}
	if got.req.Question == "" || len(got.req.Options) != 2 {
		t.Errorf("escalation payload: %+v", got.req)
	}
	if got.req.Options[1].Text != "Token bucket" || got.req.Options[1].Tag != "recommended" {
		t.Errorf("option payload mismatch: %+v", got.req.Options)
	}
}

// TestEscalateRequiresQuestion: an escalate with no question is rejected as a
// tool-level IsError result (the model self-corrects) and fires no callback.
func TestEscalateRequiresQuestion(t *testing.T) {
	rep := &captureReporter{}
	s := New(rep)
	res := call(t, s, binding{workspaceID: "L", agentCode: "MA"}, toolEscalate, map[string]any{"context": "x"})
	if !res.IsError {
		t.Errorf("escalate with no question should be a tool error, got: %+v", res)
	}
	rep.mu.Lock()
	defer rep.mu.Unlock()
	if len(rep.escalations) != 0 {
		t.Errorf("rejected escalate must fire no callback, got %d", len(rep.escalations))
	}
}

// TestAttachEvidenceFiresTypedCallback covers the attach_evidence MCP tool (PRD
// §8.3, §10): a well-formed call turns into a typed Reporter.AttachEvidence with
// the step, screenshot path, and pass/fail, attributed to the right loop+agent.
func TestAttachEvidenceFiresTypedCallback(t *testing.T) {
	rep := &captureReporter{}
	s := New(rep)
	b := binding{workspaceID: "L5", agentCode: "QA"}

	res := call(t, s, b, toolAttachEvidence, map[string]any{
		"step":            "Opened /checkout and signed in with Apple",
		"screenshot_path": "/data/workspaces/L5/screenshots/01.png",
		"passed":          true,
	})
	if res.IsError {
		t.Fatalf("expected success result, got error: %+v", res)
	}
	rep.mu.Lock()
	defer rep.mu.Unlock()
	if len(rep.evidence) != 1 {
		t.Fatalf("expected 1 evidence recorded, got %d", len(rep.evidence))
	}
	got := rep.evidence[0]
	if got.workspaceID != "L5" || got.agentCode != "QA" {
		t.Errorf("evidence attribution: got loop=%q agent=%q, want L5/QA", got.workspaceID, got.agentCode)
	}
	if got.e.Step != "Opened /checkout and signed in with Apple" {
		t.Errorf("evidence step mismatch: %q", got.e.Step)
	}
	if got.e.ScreenshotPath != "/data/workspaces/L5/screenshots/01.png" {
		t.Errorf("evidence screenshot mismatch: %q", got.e.ScreenshotPath)
	}
	if !got.e.Passed {
		t.Error("evidence should be passed=true")
	}
}

// TestAttachEvidenceFailingStep: a passed=false evidence step is recorded with
// Passed=false (so the engine can block the PR on it). screenshot_path is optional.
func TestAttachEvidenceFailingStep(t *testing.T) {
	rep := &captureReporter{}
	s := New(rep)
	res := call(t, s, binding{workspaceID: "L6", agentCode: "QA"}, toolAttachEvidence, map[string]any{
		"step":   "Sign in with Google returned a 500",
		"passed": false,
	})
	if res.IsError {
		t.Fatalf("expected success result, got error: %+v", res)
	}
	rep.mu.Lock()
	defer rep.mu.Unlock()
	if len(rep.evidence) != 1 {
		t.Fatalf("expected 1 evidence recorded, got %d", len(rep.evidence))
	}
	if rep.evidence[0].e.Passed {
		t.Error("expected passed=false")
	}
	if rep.evidence[0].e.ScreenshotPath != "" {
		t.Errorf("screenshot should be empty when omitted, got %q", rep.evidence[0].e.ScreenshotPath)
	}
}

func TestBadInputRejected(t *testing.T) {
	rep := &captureReporter{}
	s := New(rep)
	b := binding{workspaceID: "L", agentCode: "RV"}

	cases := []struct {
		name string
		tool string
		args map[string]any
	}{
		{"missing file", toolReportFinding, map[string]any{"severity": "high", "message": "x"}},
		{"missing message", toolReportFinding, map[string]any{"file": "a.go", "severity": "high"}},
		{"bad severity", toolReportFinding, map[string]any{"file": "a.go", "severity": "fatal", "message": "x"}},
		{"negative line", toolReportFinding, map[string]any{"file": "a.go", "line": -1, "severity": "low", "message": "x"}},
		{"end before start", toolReportFinding, map[string]any{"file": "a.go", "line": 10, "end_line": 5, "severity": "low", "message": "x"}},
		{"end without start", toolReportFinding, map[string]any{"file": "a.go", "end_line": 5, "severity": "low", "message": "x"}},
		{"unknown field", toolReportFinding, map[string]any{"file": "a.go", "severity": "low", "message": "x", "bogus": 1}},
		{"done no summary", toolDone, map[string]any{}},
		{"result no passed", toolReportResult, map[string]any{"summary": "x"}},
		{"plan no summary", toolPlan, map[string]any{}},
		{"escalate no question", toolEscalate, map[string]any{"context": "x"}},
		{"escalate empty option text", toolEscalate, map[string]any{
			"question": "q", "options": []map[string]any{{"text": "  "}},
		}},
		{"evidence no step", toolAttachEvidence, map[string]any{"passed": true}},
		{"evidence no passed", toolAttachEvidence, map[string]any{"step": "did a thing"}},
		{"evidence unknown field", toolAttachEvidence, map[string]any{"step": "x", "passed": true, "bogus": 1}},
		{"send_back no instruction", toolSendBack, map[string]any{}},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			res := call(t, s, b, tc.tool, tc.args)
			if !res.IsError {
				t.Errorf("expected tool-level IsError result for %s, got: %+v", tc.name, res)
			}
		})
	}
	// No callback should have fired for any rejected call.
	if nf, nd, nr, np := rep.snapshot(); nf+nd+nr+np != 0 {
		t.Errorf("rejected calls must fire no callbacks; got %d/%d/%d/%d", nf, nd, nr, np)
	}
	rep.mu.Lock()
	ne := len(rep.evidence)
	rep.mu.Unlock()
	if ne != 0 {
		t.Errorf("rejected attach_evidence calls must fire no callbacks; got %d", ne)
	}
}

func TestUnknownToolIsRPCError(t *testing.T) {
	s := New(&captureReporter{})
	params, _ := json.Marshal(callToolParams{Name: "no_such_tool", Arguments: json.RawMessage(`{}`)})
	req := rpcRequest{JSONRPC: "2.0", ID: json.RawMessage(`7`), Method: "tools/call", Params: params}
	resp, _ := s.dispatch(context.Background(), req, binding{workspaceID: "L", agentCode: "RV"})
	if resp.Error == nil || resp.Error.Code != codeMethodNotFound {
		t.Fatalf("expected method-not-found error, got %+v", resp)
	}
}

// text pulls the single text payload out of a tool result.
func text(t *testing.T, res callToolResult) string {
	t.Helper()
	if len(res.Content) != 1 {
		t.Fatalf("expected one content block, got %d", len(res.Content))
	}
	return res.Content[0].Text
}

// The wire shapes below are what Claude Code 2.1 actually sends to, and expects
// back from, a --permission-prompt-tool; they were captured from a live run
// against a probe MCP server. Getting them wrong fails open or hangs the agent,
// and neither is visible from a unit test of the broker alone.
func TestPermissionRequestAllowEchoesInput(t *testing.T) {
	rep := &captureReporter{}
	s := New(rep)
	input := json.RawMessage(`{"file_path":"/tmp/x.go","content":"package x"}`)
	res := call(t, s, binding{workspaceID: "L", agentCode: "CD"}, toolPermissionRequest, map[string]any{
		"tool_name":   "Write",
		"input":       input,
		"tool_use_id": "toolu_1",
	})
	if res.IsError {
		t.Fatalf("allow should not be a tool error: %s", text(t, res))
	}

	var got struct {
		Behavior     string          `json:"behavior"`
		UpdatedInput json.RawMessage `json:"updatedInput"`
	}
	if err := json.Unmarshal([]byte(text(t, res)), &got); err != nil {
		t.Fatalf("result text must be the permission JSON document: %v", err)
	}
	if got.Behavior != "allow" {
		t.Errorf("behavior = %q, want allow", got.Behavior)
	}
	// updatedInput is required on allow, and Recur never rewrites arguments.
	if string(got.UpdatedInput) != string(input) {
		t.Errorf("updatedInput = %s, want the original input %s", got.UpdatedInput, input)
	}

	if len(rep.asks) != 1 || rep.asks[0].ask.Tool != "Write" || rep.asks[0].agentCode != "CD" {
		t.Errorf("reporter got %+v", rep.asks)
	}
	if rep.asks[0].ask.ToolUseID != "toolu_1" {
		t.Errorf("tool_use_id should reach the reporter, got %q", rep.asks[0].ask.ToolUseID)
	}
}

func TestPermissionRequestDenyCarriesReason(t *testing.T) {
	rep := &captureReporter{onAsk: func(context.Context, PermissionAsk) (PermissionVerdict, error) {
		return PermissionVerdict{Reason: "not on this branch"}, nil
	}}
	res := call(t, New(rep), binding{workspaceID: "L", agentCode: "CD"}, toolPermissionRequest, map[string]any{
		"tool_name": "Bash",
		"input":     map[string]any{"command": "rm -rf /"},
	})
	// A refusal is a successful verdict, not a tool error: the model should read
	// the reason and adapt rather than see the broker as broken.
	if res.IsError {
		t.Fatalf("a denial is still a successful call: %s", text(t, res))
	}
	var got struct {
		Behavior string `json:"behavior"`
		Message  string `json:"message"`
	}
	if err := json.Unmarshal([]byte(text(t, res)), &got); err != nil {
		t.Fatalf("bad result text: %v", err)
	}
	if got.Behavior != "deny" || got.Message != "not on this branch" {
		t.Errorf("got %+v, want a deny carrying the reason", got)
	}
}

func TestPermissionRequestBlocksAndHonorsCancellation(t *testing.T) {
	entered := make(chan struct{})
	rep := &captureReporter{onAsk: func(ctx context.Context, _ PermissionAsk) (PermissionVerdict, error) {
		close(entered)
		<-ctx.Done() // the agent went away while the human deliberated
		return PermissionVerdict{}, ctx.Err()
	}}
	ctx, cancel := context.WithCancel(context.Background())
	done := make(chan callToolResult, 1)
	go func() {
		done <- callCtx(t, ctx, New(rep), binding{workspaceID: "L", agentCode: "CD"}, toolPermissionRequest, map[string]any{
			"tool_name": "Bash",
			"input":     map[string]any{"command": "sleep 1"},
		})
	}()

	<-entered
	select {
	case <-done:
		t.Fatal("the handler returned without waiting for a verdict")
	case <-time.After(20 * time.Millisecond):
	}

	cancel()
	select {
	case res := <-done:
		if !res.IsError {
			t.Error("an unreachable verdict should surface as a tool error")
		}
	case <-time.After(2 * time.Second):
		t.Fatal("cancelling the request did not release the handler")
	}
}

func TestPermissionRequestRejectsMissingToolName(t *testing.T) {
	res := call(t, New(&captureReporter{}), binding{workspaceID: "L", agentCode: "CD"}, toolPermissionRequest, map[string]any{
		"input": map[string]any{},
	})
	if !res.IsError {
		t.Error("a permission request with no tool_name is unanswerable")
	}
}

func TestPermissionRequestToleratesUnknownFields(t *testing.T) {
	// The payload comes from the CLI, not the model, so a field added by a
	// future Claude Code must not turn every permission request into an error.
	res := call(t, New(&captureReporter{}), binding{workspaceID: "L", agentCode: "CD"}, toolPermissionRequest, map[string]any{
		"tool_name":     "Read",
		"input":         map[string]any{"file_path": "/tmp/x"},
		"permission_id": "something-new",
	})
	if res.IsError {
		t.Errorf("unknown fields should be ignored, got error: %s", text(t, res))
	}
}

func TestInitializeAndToolsList(t *testing.T) {
	s := New(&captureReporter{})

	// initialize echoes the client's requested protocol version.
	ip, _ := json.Marshal(initializeParams{ProtocolVersion: "2025-06-18"})
	resp, _ := s.dispatch(context.Background(), rpcRequest{JSONRPC: "2.0", ID: json.RawMessage(`1`), Method: "initialize", Params: ip}, binding{})
	ir, ok := resp.Result.(initializeResult)
	if !ok {
		t.Fatalf("initialize result type: %T", resp.Result)
	}
	if ir.ProtocolVersion != "2025-06-18" {
		t.Errorf("protocol version: got %q", ir.ProtocolVersion)
	}
	if ir.ServerInfo.Name != serverName {
		t.Errorf("server name: got %q", ir.ServerInfo.Name)
	}
	if ir.Capabilities.Tools == nil {
		t.Error("expected tools capability advertised")
	}

	// tools/list returns the four-tool schema.
	resp2, _ := s.dispatch(context.Background(), rpcRequest{JSONRPC: "2.0", ID: json.RawMessage(`2`), Method: "tools/list"}, binding{})
	lr, ok := resp2.Result.(listToolsResult)
	if !ok {
		t.Fatalf("tools/list result type: %T", resp2.Result)
	}
	names := map[string]bool{}
	for _, tl := range lr.Tools {
		names[tl.name()] = true
		if len(tl.InputSchema) == 0 {
			t.Errorf("tool %s has empty input schema", tl.name())
		}
		// schema must be valid JSON.
		var probe map[string]any
		if err := json.Unmarshal(tl.InputSchema, &probe); err != nil {
			t.Errorf("tool %s schema is not valid JSON: %v", tl.name(), err)
		}
	}
	for _, want := range []string{
		toolReportFinding, toolListFindings, toolUpdateFinding, toolDone,
		toolReportResult, toolPlan, toolEscalate, toolAttachEvidence, toolSendBack,
		toolListSkills, toolActivateSkill, toolPermissionRequest,
		toolExtendRounds, toolReenterPhase, toolRunPhase,
	} {
		if !names[want] {
			t.Errorf("tools/list missing %q", want)
		}
	}
}

func TestNamesFromAllowedTools(t *testing.T) {
	got := NamesFromAllowedTools([]string{
		"Read", "Edit", "mcp__recur__list_findings", "mcp__recur__update_finding",
		"mcp__recur__list_skills", "mcp__playwright__browser_navigate",
	})
	want := map[string]bool{
		toolListFindings:  true,
		toolUpdateFinding: true,
		toolListSkills:    true,
	}
	if len(got) != len(want) {
		t.Fatalf("NamesFromAllowedTools = %v, want %d entries (no permission_request)", got, len(want))
	}
	for _, name := range got {
		if !want[name] {
			t.Errorf("unexpected name %q in %v", name, got)
		}
		if name == toolPermissionRequest {
			t.Error("NamesFromAllowedTools must not inject permission_request (engine does, Claude-only)")
		}
		if name == toolDone {
			t.Error("done must not be derived from an agent-chat allowlist")
		}
	}
	withPerm := AppendPermissionRequest(got)
	found := false
	for _, n := range withPerm {
		if n == toolPermissionRequest {
			found = true
			break
		}
	}
	if !found {
		t.Fatalf("AppendPermissionRequest missing permission_request: %v", withPerm)
	}
	if len(AppendPermissionRequest(withPerm)) != len(withPerm) {
		t.Error("AppendPermissionRequest should be idempotent")
	}

	// No mcp__recur__ names → non-nil empty (scope to nothing), not nil (unrestricted).
	empty := NamesFromAllowedTools([]string{"Read", "Glob", "Grep"})
	if empty == nil {
		t.Fatal("NamesFromAllowedTools must return non-nil empty when no Recur tools match")
	}
	if len(empty) != 0 {
		t.Fatalf("NamesFromAllowedTools(no mcp) = %v, want empty", empty)
	}
}

func TestRegisterScopesToolsListAndCall(t *testing.T) {
	agentTools := []string{
		"Read", "Edit",
		"mcp__recur__list_findings", "mcp__recur__update_finding",
		"mcp__recur__list_skills", "mcp__recur__activate_skill",
	}
	listNames := func(t *testing.T, s *Server, b binding) map[string]bool {
		t.Helper()
		resp, _ := s.dispatch(context.Background(), rpcRequest{
			JSONRPC: "2.0", ID: json.RawMessage(`1`), Method: "tools/list",
		}, b)
		lr, ok := resp.Result.(listToolsResult)
		if !ok {
			t.Fatalf("tools/list result type: %T", resp.Result)
		}
		names := map[string]bool{}
		for _, tl := range lr.Tools {
			names[tl.name()] = true
		}
		return names
	}

	t.Run("claude binding advertises and allows permission_request", func(t *testing.T) {
		rep := &captureReporter{}
		s := New(rep)
		// Engine appends permission_request for Claude Code turns.
		token, _ := s.Register("W1", "PA", AppendPermissionRequest(NamesFromAllowedTools(agentTools)))
		b, ok := s.lookup(token)
		if !ok {
			t.Fatal("token not registered")
		}
		names := listNames(t, s, b)
		for _, want := range []string{
			toolListFindings, toolUpdateFinding,
			toolListSkills, toolActivateSkill, toolPermissionRequest,
		} {
			if !names[want] {
				t.Errorf("Claude tools/list missing %q; got %v", want, names)
			}
		}
		for _, banned := range []string{toolDone, toolEscalate, toolPlan, toolReportFinding} {
			if names[banned] {
				t.Errorf("Claude tools/list must not advertise %q", banned)
			}
		}
		if !b.allows(toolPermissionRequest) {
			t.Error("Claude binding must allow permission_request")
		}
		res := call(t, s, b, toolDone, map[string]any{"summary": "should not work"})
		if !res.IsError {
			t.Fatalf("done call should fail on PA binding, got %+v", res)
		}
		if len(rep.dones) != 0 {
			t.Errorf("Done reporter must not run for scoped-out tool; got %+v", rep.dones)
		}
	})

	t.Run("cursor binding omits permission_request", func(t *testing.T) {
		rep := &captureReporter{}
		s := New(rep)
		// Cursor: NamesFromAllowedTools only — no AppendPermissionRequest.
		token, _ := s.Register("W2", "PA", NamesFromAllowedTools(agentTools))
		b, ok := s.lookup(token)
		if !ok {
			t.Fatal("token not registered")
		}
		names := listNames(t, s, b)
		if names[toolPermissionRequest] {
			t.Error("Cursor tools/list must not advertise permission_request")
		}
		if b.allows(toolPermissionRequest) {
			t.Error("Cursor binding must not allow permission_request")
		}
		res := call(t, s, b, toolPermissionRequest, map[string]any{
			"tool_name": "Write", "input": map[string]any{},
		})
		if !res.IsError {
			t.Fatalf("permission_request call should fail on Cursor binding, got %+v", res)
		}
		for _, want := range []string{toolListFindings, toolUpdateFinding, toolListSkills} {
			if !names[want] {
				t.Errorf("Cursor tools/list missing %q; got %v", want, names)
			}
		}
	})

	t.Run("empty non-nil scope advertises nothing and refuses calls", func(t *testing.T) {
		// Cursor PR-responder shape: readExploreTools only → NamesFromAllowedTools
		// returns non-nil empty; must NOT fall through to unrestricted.
		rep := &captureReporter{}
		s := New(rep)
		empty := NamesFromAllowedTools([]string{"Read", "Glob", "Grep"})
		token, _ := s.Register("W3", "PR", empty)
		b, ok := s.lookup(token)
		if !ok {
			t.Fatal("token not registered")
		}
		if b.allowed == nil {
			t.Fatal("empty scope must set allowed map (restricted), not nil (unrestricted)")
		}
		names := listNames(t, s, b)
		if len(names) != 0 {
			t.Fatalf("empty scope tools/list = %v, want empty", names)
		}
		res := call(t, s, b, toolDone, map[string]any{"summary": "leak"})
		if !res.IsError {
			t.Fatalf("empty scope must refuse tools/call, got %+v", res)
		}
		if len(rep.dones) != 0 {
			t.Errorf("Done must not run under empty scope; got %+v", rep.dones)
		}
	})
}

func TestListAndActivateSkills(t *testing.T) {
	rep := &captureReporter{}
	s := New(rep)
	b := binding{workspaceID: "L1", agentCode: "PA"}

	res := call(t, s, b, toolListSkills, map[string]any{})
	if res.IsError {
		t.Fatalf("list_skills error: %+v", res)
	}
	text := ""
	if len(res.Content) > 0 {
		text = res.Content[0].Text
	}
	if !strings.Contains(text, "<available_skills>") ||
		!strings.Contains(text, "<name>demo-skill</name>") ||
		!strings.Contains(text, "<description>A demo</description>") {
		t.Fatalf("catalog shape: %q", text)
	}
	if strings.Contains(text, "<location>") {
		t.Fatalf("location should be omitted when activate_skill exists: %q", text)
	}

	res = call(t, s, b, toolActivateSkill, map[string]any{"name": "demo-skill"})
	if res.IsError {
		t.Fatalf("activate_skill error: %+v", res)
	}
	text = ""
	if len(res.Content) > 0 {
		text = res.Content[0].Text
	}
	if !strings.Contains(text, `<skill_content name="demo-skill">`) {
		t.Fatalf("activate body: %q", text)
	}

	res = call(t, s, b, toolActivateSkill, map[string]any{"name": "missing"})
	if !res.IsError {
		t.Fatal("unknown skill should error")
	}
}

func TestListAndReadTerminals(t *testing.T) {
	rep := &captureReporter{}
	s := New(rep)
	b := binding{workspaceID: "L1", agentCode: "PA"}

	res := call(t, s, b, toolListTerminals, map[string]any{})
	if res.IsError {
		t.Fatalf("list_terminals error: %+v", res)
	}
	text := ""
	if len(res.Content) > 0 {
		text = res.Content[0].Text
	}
	if !strings.Contains(text, `"id":"term-1"`) || !strings.Contains(text, `"title":"zsh"`) {
		t.Fatalf("list_terminals: %q", text)
	}

	res = call(t, s, b, toolReadTerminal, map[string]any{})
	if res.IsError {
		t.Fatalf("read_terminal focused: %+v", res)
	}
	text = ""
	if len(res.Content) > 0 {
		text = res.Content[0].Text
	}
	if !strings.Contains(text, "error: boom") {
		t.Fatalf("read_terminal body: %q", text)
	}

	res = call(t, s, b, toolReadTerminal, map[string]any{"id": "missing"})
	if !res.IsError {
		t.Fatal("unknown terminal should error")
	}
}

func TestReadArtifact(t *testing.T) {
	rep := &captureReporter{}
	s := New(rep)
	b := binding{workspaceID: "L1", agentCode: "PA"}

	res := call(t, s, b, toolReadArtifact, map[string]any{"id": "a1"})
	if res.IsError {
		t.Fatalf("read_artifact: %+v", res)
	}
	text := ""
	if len(res.Content) > 0 {
		text = res.Content[0].Text
	}
	if !strings.Contains(text, `"id":"a1"`) || !strings.Contains(text, "# hi") {
		t.Fatalf("read_artifact body: %q", text)
	}

	res = call(t, s, b, toolReadArtifact, map[string]any{"id": "missing"})
	if !res.IsError {
		t.Fatal("unknown artifact should error")
	}
}

func TestSendBackFiresTypedCallback(t *testing.T) {
	rep := &captureReporter{}
	s := New(rep)
	b := binding{workspaceID: "L-sb", agentCode: "MA"}
	res := call(t, s, b, toolSendBack, map[string]any{"instruction": "rebase onto main and fix conflicts"})
	if res.IsError {
		t.Fatalf("send_back returned error: %+v", res)
	}
	if len(rep.sendBacks) != 1 {
		t.Fatalf("SendBack calls: got %d, want 1", len(rep.sendBacks))
	}
	got := rep.sendBacks[0]
	if got.workspaceID != "L-sb" || got.agentCode != "MA" || got.summary != "rebase onto main and fix conflicts" {
		t.Errorf("SendBack payload: %+v", got)
	}
}

func TestSendBackRequiresInstruction(t *testing.T) {
	s := New(&captureReporter{})
	res := call(t, s, binding{workspaceID: "L", agentCode: "MA"}, toolSendBack, map[string]any{})
	if !res.IsError {
		t.Fatal("send_back with no instruction should be a tool-level error")
	}
}

// tool.name accessor for the test (field is unexported but in-package).
func (t tool) name() string { return t.Name }

func TestNotificationGetsNoResponse(t *testing.T) {
	s := New(&captureReporter{})
	_, isNotif := s.dispatch(context.Background(), rpcRequest{JSONRPC: "2.0", Method: "notifications/initialized"}, binding{})
	if !isNotif {
		t.Error("notifications/initialized should be treated as a notification (no response)")
	}
}

// TestHTTPRoundTripWithTokenAttribution exercises the full HTTP path: Start the
// real server, Register a token, POST a tools/call to /<token>/mcp, and assert the
// finding is attributed to the token's loop+agent. Also asserts an unknown token
// 404s, Unregister 404s until re-Register, and Drop revokes a token.
func TestHTTPRoundTripWithTokenAttribution(t *testing.T) {
	rep := &captureReporter{}
	s := New(rep)
	if err := s.Start(); err != nil {
		t.Fatalf("Start: %v", err)
	}
	defer s.Close()

	token, url := s.Register("LOOP-9", "RV", nil)

	body := mustJSON(callToolEnvelope("report_finding", map[string]any{
		"file": "x.go", "line": 1, "severity": "medium", "message": "smell",
	}))
	resp, err := http.Post(url, "application/json", bytes.NewReader(body))
	if err != nil {
		t.Fatalf("POST: %v", err)
	}
	defer resp.Body.Close()
	if resp.StatusCode != http.StatusOK {
		t.Fatalf("status: %d", resp.StatusCode)
	}
	var out rpcResponse
	if err := json.NewDecoder(resp.Body).Decode(&out); err != nil {
		t.Fatalf("decode: %v", err)
	}
	if out.Error != nil {
		t.Fatalf("rpc error: %+v", out.Error)
	}
	if nf, _, _, _ := rep.snapshot(); nf != 1 {
		t.Fatalf("expected 1 finding via HTTP, got %d", nf)
	}
	if rep.findings[0].workspaceID != "LOOP-9" || rep.findings[0].agentCode != "RV" {
		t.Errorf("HTTP attribution wrong: %+v", rep.findings[0])
	}

	// Unknown token → 404.
	bad := s.Port()
	resp2, err := http.Post("http://127.0.0.1:"+itoa(bad)+"/deadbeef/mcp", "application/json", bytes.NewReader(body))
	if err != nil {
		t.Fatalf("POST unknown: %v", err)
	}
	resp2.Body.Close()
	if resp2.StatusCode != http.StatusNotFound {
		t.Errorf("unknown token status: got %d, want 404", resp2.StatusCode)
	}

	// Unregister deactivates the token (404) until the next Register.
	s.Unregister(token)
	respKeep, err := http.Post(url, "application/json", bytes.NewReader(body))
	if err != nil {
		t.Fatalf("POST after Unregister: %v", err)
	}
	respKeep.Body.Close()
	if respKeep.StatusCode != http.StatusNotFound {
		t.Errorf("Unregister should 404 the token: got %d, want 404", respKeep.StatusCode)
	}
	tok2, url2 := s.Register("LOOP-9", "RV", nil)
	if tok2 != token || url2 != url {
		t.Errorf("Register after Unregister reused token = %q %q, want %q %q", tok2, url2, token, url)
	}
	respReuse, err := http.Post(url, "application/json", bytes.NewReader(body))
	if err != nil {
		t.Fatalf("POST after re-Register: %v", err)
	}
	respReuse.Body.Close()
	if respReuse.StatusCode != http.StatusOK {
		t.Errorf("re-Register should reactivate the token: got %d, want 200", respReuse.StatusCode)
	}

	// Drop revokes it.
	s.Drop(token)
	resp3, err := http.Post(url, "application/json", bytes.NewReader(body))
	if err != nil {
		t.Fatalf("POST after Drop: %v", err)
	}
	resp3.Body.Close()
	if resp3.StatusCode != http.StatusNotFound {
		t.Errorf("dropped token status: got %d, want 404", resp3.StatusCode)
	}
}

func TestRegisterReusesTokenForSameAgent(t *testing.T) {
	s := New(&captureReporter{})
	if err := s.Start(); err != nil {
		t.Fatalf("Start: %v", err)
	}
	defer s.Close()

	tok1, url1 := s.Register("W1", "PA", []string{"list_skills"})
	s.Unregister(tok1)
	tok2, url2 := s.Register("W1", "PA", []string{"list_skills", "submit_plan"})
	if tok1 != tok2 || url1 != url2 {
		t.Errorf("Register(W1, PA) reused token = %q %q, want same %q %q", tok2, url2, tok1, url1)
	}
	b, ok := s.lookup(tok2)
	if !ok || !b.active || !b.allows("submit_plan") {
		t.Errorf("re-Register should reactivate and replace allowlist: active=%v allows(submit_plan)=%v", b.active, b.allows("submit_plan"))
	}
	other, _ := s.Register("W1", "PA2", []string{"list_skills"})
	if other == tok1 {
		t.Errorf("different agent must not share token %q", tok1)
	}
	s.DropAgent("W1", "PA")
	tok3, _ := s.Register("W1", "PA", []string{"list_skills"})
	if tok3 == tok1 {
		t.Errorf("DropAgent should mint a new token, got %q", tok3)
	}
}

func TestRegisterOverlapMintsDistinctToken(t *testing.T) {
	s := New(&captureReporter{})
	if err := s.Start(); err != nil {
		t.Fatalf("Start: %v", err)
	}
	defer s.Close()

	tokChat, _ := s.Register("W1", "MA", []string{"ask_user"})
	tokPlan, _ := s.Register("W1", "MA", []string{"escalate"})
	if tokChat == tokPlan {
		t.Fatalf("overlapping Register(W1, MA) = %q, want a distinct extra token", tokPlan)
	}
	chat, ok := s.lookup(tokChat)
	if !ok {
		t.Fatal("lookup(chat token) ok=false")
	}
	plan, ok := s.lookup(tokPlan)
	if !ok {
		t.Fatal("lookup(plan token) ok=false")
	}
	if !chat.allows("ask_user") || chat.allows("escalate") {
		t.Errorf("chat binding allows ask_user=%v escalate=%v, want ask_user only", chat.allows("ask_user"), chat.allows("escalate"))
	}
	if !plan.allows("escalate") || plan.allows("ask_user") {
		t.Errorf("plan binding allows escalate=%v ask_user=%v, want escalate only", plan.allows("escalate"), plan.allows("ask_user"))
	}

	wid, code, ok := s.Unregister(tokPlan)
	if !ok || wid != "W1" || code != "MA" {
		t.Errorf("Unregister(plan) = %q %q ok=%v, want W1 MA true", wid, code, ok)
	}
	if _, live := s.lookup(tokPlan); live {
		t.Error("overlapping extra token should be dropped on Unregister")
	}
	s.Unregister(tokChat)
	chat, ok = s.lookup(tokChat)
	if !ok || chat.active {
		t.Errorf("canonical token after Unregister: ok=%v active=%v, want inactive but kept", ok, chat.active)
	}
	tok3, _ := s.Register("W1", "MA", []string{"ask_user"})
	if tok3 != tokChat {
		t.Errorf("Register after Unregister reused token = %q, want %q", tok3, tokChat)
	}
}

// TestConcurrentToolCallsRaceClean fires many concurrent report_finding calls
// across two tokens and asserts the reporter recorded all of them with correct
// attribution. Run under -race to shake the registry + handler.
func TestConcurrentToolCallsRaceClean(t *testing.T) {
	rep := &captureReporter{}
	s := New(rep)
	if err := s.Start(); err != nil {
		t.Fatalf("Start: %v", err)
	}
	defer s.Close()

	_, urlA := s.Register("LA", "RV", nil)
	_, urlB := s.Register("LB", "RV", nil)

	const perURL = 25
	var wg sync.WaitGroup
	post := func(url string) {
		defer wg.Done()
		body := mustJSON(callToolEnvelope("report_finding", map[string]any{
			"file": "f.go", "severity": "low", "message": "x",
		}))
		resp, err := http.Post(url, "application/json", bytes.NewReader(body))
		if err == nil {
			resp.Body.Close()
		}
	}
	for i := 0; i < perURL; i++ {
		wg.Add(2)
		go post(urlA)
		go post(urlB)
	}
	wg.Wait()

	nf, _, _, _ := rep.snapshot()
	if nf != perURL*2 {
		t.Fatalf("expected %d findings, got %d", perURL*2, nf)
	}
	la, lb := 0, 0
	rep.mu.Lock()
	for _, f := range rep.findings {
		switch f.workspaceID {
		case "LA":
			la++
		case "LB":
			lb++
		default:
			t.Errorf("unexpected loop attribution: %q", f.workspaceID)
		}
	}
	rep.mu.Unlock()
	if la != perURL || lb != perURL {
		t.Errorf("attribution split wrong: LA=%d LB=%d (want %d each)", la, lb, perURL)
	}
}

// ---- helpers ----

func callToolEnvelope(name string, args map[string]any) rpcRequest {
	raw, _ := json.Marshal(args)
	params, _ := json.Marshal(callToolParams{Name: name, Arguments: raw})
	return rpcRequest{JSONRPC: "2.0", ID: json.RawMessage(`1`), Method: "tools/call", Params: params}
}

func mustJSON(v any) []byte {
	b, err := json.Marshal(v)
	if err != nil {
		panic(err)
	}
	return b
}

func itoa(n int) string {
	if n == 0 {
		return "0"
	}
	neg := n < 0
	if neg {
		n = -n
	}
	var buf [20]byte
	i := len(buf)
	for n > 0 {
		i--
		buf[i] = byte('0' + n%10)
		n /= 10
	}
	if neg {
		i--
		buf[i] = '-'
	}
	return string(buf[i:])
}

// TestSubmitPlanAndUpdateStep: the structured-plan tools validate their args and
// fire the typed callbacks.
func TestSubmitPlanAndUpdateStep(t *testing.T) {
	rep := &captureReporter{}
	s := New(rep)
	b := binding{workspaceID: "L3", agentCode: "PA"}

	res := call(t, s, b, toolSubmitPlan, map[string]any{
		"summary": "Add dark mode",
		"body":    "Add a toggle in Settings.svelte and persist the preference.",
		"steps":   []string{"Add the toggle to Settings.svelte", "  ", "Persist the preference"},
	})
	if res.IsError {
		t.Fatalf("submit_plan should succeed, got %+v", res)
	}
	if len(rep.submittedPlans) != 1 {
		t.Fatalf("expected 1 submitted plan, got %d", len(rep.submittedPlans))
	}
	got := rep.submittedPlans[0]
	if got.workspaceID != "L3" || got.agentCode != "PA" {
		t.Errorf("attribution: %q/%q", got.workspaceID, got.agentCode)
	}
	// Blank steps are dropped before the callback.
	if !strings.Contains(got.summary, "Add the toggle") || strings.Contains(got.summary, "  ;") {
		t.Errorf("steps payload: %q", got.summary)
	}

	// Missing body → tool-level error, no extra callback.
	if res := call(t, s, b, toolSubmitPlan, map[string]any{
		"summary": "no body", "body": "  ", "steps": []string{"one"},
	}); !res.IsError {
		t.Fatal("submit_plan with empty body should be a tool error")
	}
	if len(rep.submittedPlans) != 1 {
		t.Fatal("failed submit_plan must not fire the callback")
	}

	// Missing steps → tool-level error, no callback.
	if res := call(t, s, b, toolSubmitPlan, map[string]any{"summary": "no steps", "steps": []string{}}); !res.IsError {
		t.Fatal("submit_plan with no steps should be a tool error")
	}
	if len(rep.submittedPlans) != 1 {
		t.Fatal("failed submit_plan must not fire the callback")
	}

	if res := call(t, s, b, toolUpdatePlanStep, map[string]any{"step_id": "S1", "status": "done"}); res.IsError {
		t.Fatalf("update_plan_step should succeed, got %+v", res)
	}
	if len(rep.stepUpdates) != 1 || rep.stepUpdates[0].summary != "S1→done" {
		t.Fatalf("step update payload: %+v", rep.stepUpdates)
	}
	if res := call(t, s, b, toolUpdatePlanStep, map[string]any{"step_id": "S1", "status": "finished"}); !res.IsError {
		t.Fatal("bad status vocabulary should be a tool error")
	}
}

func TestRenameWorkspaceAndBranchTools(t *testing.T) {
	rep := &captureReporter{}
	s := New(rep)
	b := binding{workspaceID: "L1", agentCode: "PA"}

	res := call(t, s, b, toolRenameWorkspace, map[string]any{"title": "Dark mode toggle"})
	if res.IsError {
		t.Fatalf("rename_workspace: %+v", res)
	}
	if !strings.Contains(text(t, res), "Dark mode toggle") {
		t.Errorf("rename_workspace result %q", text(t, res))
	}

	res = call(t, s, b, toolRenameBranch, map[string]any{"name": "add-dark-mode"})
	if res.IsError {
		t.Fatalf("rename_branch: %+v", res)
	}
	if !strings.Contains(text(t, res), "add-dark-mode") {
		t.Errorf("rename_branch result %q", text(t, res))
	}

	if res := call(t, s, b, toolRenameWorkspace, map[string]any{}); !res.IsError {
		t.Fatal("rename_workspace without title should error")
	}
	if res := call(t, s, b, toolRenameBranch, map[string]any{}); !res.IsError {
		t.Fatal("rename_branch without name should error")
	}

	pl := binding{workspaceID: "L1", agentCode: "PL", chatCode: "PA2"}
	res = call(t, s, pl, toolRenameSession, map[string]any{"title": "Dark mode"})
	if res.IsError {
		t.Fatalf("rename_session: %+v", res)
	}
	if rep.renameOwner != "PA2" {
		t.Errorf("rename_session ownerChat = %q, want PA2", rep.renameOwner)
	}
}

func TestOpenPRTool(t *testing.T) {
	rep := &captureReporter{}
	s := New(rep)
	b := binding{workspaceID: "L1", agentCode: "PA"}

	res := call(t, s, b, toolOpenPR, map[string]any{
		"title": "Agent-written GitHub PR descriptions",
		"body":  "The titlebar Open PR action now invokes /open-pr.",
	})
	if res.IsError {
		t.Fatalf("open_pr: %+v", res)
	}
	if !strings.Contains(text(t, res), "Agent-written GitHub PR descriptions") {
		t.Errorf("open_pr result %q", text(t, res))
	}

	if res := call(t, s, b, toolOpenPR, map[string]any{"title": "Only title"}); !res.IsError {
		t.Fatal("open_pr without body should error")
	}
	if res := call(t, s, b, toolOpenPR, map[string]any{"body": "Only body"}); !res.IsError {
		t.Fatal("open_pr without title should error")
	}

	draft := call(t, s, b, toolOpenPR, map[string]any{
		"title": "WIP gateway",
		"body":  "Not ready for review.",
		"draft": true,
	})
	if draft.IsError {
		t.Fatalf("open_pr draft: %+v", draft)
	}
	if !strings.Contains(text(t, draft), "draft") {
		t.Errorf("open_pr(draft=true) result %q, want draft", text(t, draft))
	}
}
