package engine

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

// convergeRunner is a TEST DOUBLE that drives the convergence loop through the
// REAL MCP path: when it runs as the Reviewer it POSTs report_finding tool calls
// to the loop's live MCP URL (spec.MCPURL) — the exact HTTP → JSON-RPC → tool
// handler → engineReporter → collectFinding path the production claude agent uses.
// The findings-per-review-iteration schedule is given by findingsPerIter. When it
// runs as the Coder it makes a tiny edit so the engine commit has something to
// commit. This exercises the engine's real convergence logic, not a mock.
type convergeRunner struct {
	findingsPerIter []int  // index i → number of findings the reviewer files on review iteration i+1
	commitText      string // reply to role-less one-shots (the commit-message turn)

	mu          sync.Mutex
	coderRuns   int
	reviewRuns  int
	reviewIter  int         // 1-based review iteration counter
	sawReviewer []string    // agent ids that ran as the reviewer
	sawCoder    []string    // agent ids that ran as the coder
	specs       []AgentSpec // every turn, for whole-loop invariant assertions
}

// capturedSpecs returns every AgentSpec this runner was handed.
func (r *convergeRunner) capturedSpecs() []AgentSpec {
	r.mu.Lock()
	defer r.mu.Unlock()
	return append([]AgentSpec(nil), r.specs...)
}

func (r *convergeRunner) Run(ctx context.Context, spec AgentSpec, emit func(ParsedEvent)) (Result, error) {
	r.mu.Lock()
	r.specs = append(r.specs, spec)
	r.mu.Unlock()

	switch spec.Role {
	case domain.RoleCoder:
		r.mu.Lock()
		r.coderRuns++
		r.sawCoder = append(r.sawCoder, spec.AgentID)
		r.mu.Unlock()
		// A real edit each coding turn so the engine commit lands a diff.
		sr := &ScriptedRunner{Edits: map[string]string{"work.txt": "iteration edit\n"}}
		if err := sr.applyEdits(spec.Worktree); err != nil {
			return Result{}, err
		}
		if emit != nil {
			emit(ParsedEvent{Kind: KindStep, Tool: "Edit", ToolInput: "work.txt"})
		}
		// Close open findings via the real MCP path (coder checklist before done).
		if spec.MCPURL != "" {
			if err := closeOpenFindingsMCP(spec.MCPURL); err != nil {
				return Result{}, err
			}
		}
		return Result{Subtype: "success", SessionID: spec.AgentID}, nil

	case domain.RoleReviewer:
		r.mu.Lock()
		r.reviewRuns++
		r.reviewIter++
		iter := r.reviewIter
		r.sawReviewer = append(r.sawReviewer, spec.AgentID)
		r.mu.Unlock()

		n := 0
		if iter-1 < len(r.findingsPerIter) {
			n = r.findingsPerIter[iter-1]
		}
		// File n findings through the REAL MCP HTTP endpoint.
		for i := 0; i < n; i++ {
			if err := postFinding(spec.MCPURL, "work.txt", i+1, "medium", "issue"); err != nil {
				return Result{}, err
			}
		}
		return Result{Subtype: "success", SessionID: spec.AgentID}, nil

	default:
		return Result{Subtype: "success", Text: r.commitText}, nil
	}
}

// postFinding POSTs a report_finding tools/call to the MCP URL and waits for the
// (success) response, so by the time the reviewer turn returns the engine has
// recorded the finding (the production parser reads the tool_result similarly).
func postFinding(mcpURL, file string, line int, severity, message string) error {
	return postMCPTool(mcpURL, "report_finding", map[string]any{
		"file": file, "line": line, "severity": severity, "message": message,
	})
}

// closeOpenFindingsMCP lists open findings and marks each resolved (coder checklist).
func closeOpenFindingsMCP(mcpURL string) error {
	text, err := postMCPToolResult(mcpURL, "list_findings", map[string]any{"status": "open"})
	if err != nil {
		return err
	}
	for _, id := range parseFindingIDs(text) {
		if err := postMCPTool(mcpURL, "update_finding", map[string]any{
			"finding_id": id, "status": "resolved",
		}); err != nil {
			return err
		}
	}
	return nil
}

func postMCPTool(mcpURL, name string, args map[string]any) error {
	_, err := postMCPToolResult(mcpURL, name, args)
	return err
}

func postMCPToolResult(mcpURL, name string, args map[string]any) (string, error) {
	argJSON, _ := json.Marshal(args)
	params, _ := json.Marshal(map[string]any{"name": name, "arguments": json.RawMessage(argJSON)})
	body, _ := json.Marshal(map[string]any{
		"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": json.RawMessage(params),
	})
	resp, err := http.Post(mcpURL, "application/json", bytes.NewReader(body))
	if err != nil {
		return "", err
	}
	defer resp.Body.Close()
	var out struct {
		Error  *struct{ Message string } `json:"error"`
		Result *struct {
			IsError bool `json:"isError"`
			Content []struct {
				Text string `json:"text"`
			} `json:"content"`
		} `json:"result"`
	}
	if err := json.NewDecoder(resp.Body).Decode(&out); err != nil {
		return "", err
	}
	if out.Error != nil {
		return "", fmt.Errorf("mcp %s: %s", name, out.Error.Message)
	}
	text := ""
	if out.Result != nil && len(out.Result.Content) > 0 {
		text = out.Result.Content[0].Text
	}
	if out.Result != nil && out.Result.IsError {
		return text, fmt.Errorf("mcp %s: %s", name, text)
	}
	return text, nil
}

func parseFindingIDs(listText string) []string {
	var out struct {
		Findings []struct {
			ID string `json:"id"`
		} `json:"findings"`
	}
	if err := json.Unmarshal([]byte(listText), &out); err == nil {
		ids := make([]string, 0, len(out.Findings))
		for _, f := range out.Findings {
			if f.ID != "" {
				ids = append(ids, f.ID)
			}
		}
		return ids
	}
	// Legacy prose format (id=…).
	var ids []string
	for _, line := range strings.Split(listText, "\n") {
		const marker = "id="
		i := strings.Index(line, marker)
		if i < 0 {
			continue
		}
		rest := line[i+len(marker):]
		end := strings.IndexAny(rest, " \t")
		if end < 0 {
			ids = append(ids, rest)
		} else {
			ids = append(ids, rest[:end])
		}
	}
	return ids
}

// TestConvergenceLoopThreeOneZero is the headline M3 test (PRD §7.3, §19 M3): a
// loop where the scripted Reviewer files 3 findings (iter1) → 1 (iter2) → 0
// (iter3). It asserts the loop runs EXACTLY 3 coding↔review iterations, the
// convergence series is [3,1,0], the loop reaches converged (pr_open), the
// Reviewer is a DIFFERENT agent than the Coder (rail #2), and the ENGINE — not
// the Coder — declared convergence. Drives findings through the real MCP path.
func TestConvergenceLoopThreeOneZero(t *testing.T) {
	repo := initRepo(t)
	runner := &convergeRunner{findingsPerIter: []int{3, 1, 0}}
	eng, st, sink, _ := newTestEngine(t, runner)
	projID := addProject(t, st, repo)

	id, err := eng.StartWorkspace(projID, "Build the converging feature", "", "")
	if err != nil {
		t.Fatalf("StartWorkspace: %v", err)
	}

	waitFor(t, 10*time.Second, "loop to converge to pr_open", func() bool {
		l, ok := st.Workspace(id)
		return ok && l.State == domain.StatePROpen
	})

	// Exactly 3 coding↔review iterations.
	runner.mu.Lock()
	coderRuns, reviewRuns := runner.coderRuns, runner.reviewRuns
	sawCoder := append([]string(nil), runner.sawCoder...)
	sawReviewer := append([]string(nil), runner.sawReviewer...)
	runner.mu.Unlock()
	if coderRuns != 3 {
		t.Errorf("coder ran %d times, want 3", coderRuns)
	}
	if reviewRuns != 3 {
		t.Errorf("reviewer ran %d times, want 3", reviewRuns)
	}

	// The convergence series recorded on the loop is exactly [3,1,0].
	l, _ := st.Workspace(id)
	var series []int
	for _, c := range l.Convergence {
		series = append(series, c.Findings)
	}
	if !equalInts(series, []int{3, 1, 0}) {
		t.Errorf("convergence series: got %v, want [3 1 0]", series)
	}
	// Iterations are 1,2,3 in order.
	for i, c := range l.Convergence {
		if c.Iteration != i+1 {
			t.Errorf("convergence point %d has iteration %d, want %d", i, c.Iteration, i+1)
		}
	}

	// RAIL #2: the reviewer is a DIFFERENT agent than the coder on every iteration.
	for _, cid := range sawCoder {
		for _, rid := range sawReviewer {
			if cid == rid {
				t.Errorf("rail #2 violated: reviewer and coder share agent id %q", cid)
			}
		}
	}
	// And the agent codes differ structurally.
	if roles[domain.RoleReviewer].code == roles[domain.RoleCoder].code {
		t.Error("rail #2: reviewer and coder must have distinct agent codes")
	}

	// The ENGINE (not the coder) declared convergence: the loop reached pr_open and
	// the Maestro emitted a convergence event — the coder never opens a PR or sets
	// pr_open itself (it only edits files + calls done). Assert the engine-authored
	// convergence feed line exists.
	foundConvergedEvent := false
	for _, ev := range sink.byName("workspace.event") {
		m := ev.Data.(map[string]any)
		item, _ := m["event"].(domain.FeedItem)
		if item.Name == "Maestro" && contains(item.Text, "Converged") {
			foundConvergedEvent = true
		}
	}
	if !foundConvergedEvent {
		t.Error("expected an engine/Maestro-authored 'Converged' event (engine decides convergence, PRD §7.3)")
	}

	// Convergence chart data flows through loop.updated with the convergence[] series.
	sawConvergencePayload := false
	for _, ev := range sink.byName("workspace.updated") {
		m := ev.Data.(map[string]any)
		if cps, ok := m["convergence"].([]domain.ConvPoint); ok && len(cps) > 0 {
			sawConvergencePayload = true
		}
	}
	if !sawConvergencePayload {
		t.Error("expected loop.updated to carry the convergence[] series for the chart")
	}

	// Findings persisted (PRD §14): 3 + 1 = 4 findings total were filed.
	allFindings := st.Findings(id)
	if len(allFindings) != 4 {
		t.Errorf("expected 4 persisted findings (3+1+0), got %d", len(allFindings))
	}
	// Iteration-1 and iteration-2 findings should be marked resolved by the time the
	// later coding turns addressed them; the final review filed none.
	openCount := st.OpenFindingsCount(id)
	if openCount != 0 {
		t.Errorf("converged loop should have 0 open findings, got %d", openCount)
	}
}

// TestConvergenceCapStopsNoInfiniteLoop covers the rail-#4-OFF cap fallback: if the
// reviewer NEVER reports zero findings AND rail #4 (escalate-when-looping) is OFF,
// the loop must stop at the autonomy cap (NOT loop forever, NOT open a PR with open
// findings — rail #3, NOT escalate). Balanced autonomy caps at 4 iterations. With
// rail #4 ON (the default), the loop escalates instead — see
// TestEscalationRoundTripAtCap.
func TestConvergenceCapStopsNoInfiniteLoop(t *testing.T) {
	repo := initRepo(t)
	// Always 2 findings — never converges.
	runner := &convergeRunner{findingsPerIter: []int{2, 2, 2, 2, 2, 2, 2, 2}}
	eng, st, _, _ := newTestEngine(t, runner)
	installFixturePack(t)
	setDefaultPolicy(t, st, "test.policies.fail")
	projID := addProject(t, st, repo)

	id, err := eng.StartWorkspace(projID, "never converges", "", "")
	if err != nil {
		t.Fatalf("StartWorkspace: %v", err)
	}

	// It must terminate (NOT hang): Balanced cap is 4 → stops at stopped state.
	waitFor(t, 15*time.Second, "non-converging loop to fail at the cap", func() bool {
		l, ok := st.Workspace(id)
		return ok && l.State == domain.StateFailed
	})

	l, _ := st.Workspace(id)
	if l.State == domain.StatePROpen {
		t.Fatal("non-converging loop must NOT open a PR (rail #3)")
	}
	// Exactly the cap number of iterations ran (4 for Balanced).
	runner.mu.Lock()
	coderRuns, reviewRuns := runner.coderRuns, runner.reviewRuns
	runner.mu.Unlock()
	if coderRuns != 4 || reviewRuns != 4 {
		t.Errorf("cap iterations: coder=%d reviewer=%d, want 4 each", coderRuns, reviewRuns)
	}
	// The convergence series has 4 points, all non-zero.
	if len(l.Convergence) != 4 {
		t.Errorf("expected 4 convergence points at the cap, got %d", len(l.Convergence))
	}
	for _, c := range l.Convergence {
		if c.Findings == 0 {
			t.Errorf("a non-converging cap series must have no zero points, got %v", l.Convergence)
		}
	}
}

// ---- helpers ----

func equalInts(a, b []int) bool {
	if len(a) != len(b) {
		return false
	}
	for i := range a {
		if a[i] != b[i] {
			return false
		}
	}
	return true
}

func contains(s, sub string) bool { return indexOf(s, sub) >= 0 }
