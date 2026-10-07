package bundles_test

import (
	"os"
	"path/filepath"
	"strings"
	"testing"

	"recur/internal/bundles"
)

func writeLoopBundle(t *testing.T, dir, toml string) {
	t.Helper()
	if err := os.WriteFile(filepath.Join(dir, "bundle.toml"), []byte(toml), 0o644); err != nil {
		t.Fatal(err)
	}
}

const loopFixture = `
[bundle]
name = "Loop"
id = "recur.loop"

[[role]]
id = "recur.roles.maestro"
name = "Maestro"
code = "MA"
mode = "loop"

[[role]]
id = "recur.roles.planner"
name = "Planner"
code = "PL"
mode = "plan"

[[role]]
id = "recur.roles.coder"
name = "Coder"
code = "CO"
mode = "agent"
tools = ["update_plan_step"]

[[role]]
id = "recur.roles.reviewer"
name = "Reviewer"
code = "RV"
mode = "review"

[[policy]]
id = "recur.policies.balanced"
name = "Balanced"
max_rounds = 4
on_max_rounds = "escalate"
findings_threshold = { critical = 0, high = 0, medium = 0 }
plan_approval = "if_structured"
extend_rounds = 2

[[loop]]
id = "recur.loops.pr"
name = "To a PR"
conductor = "recur.roles.maestro"
entry = "planning"
reenter = ["planning", "converge"]

[[loop.phase]]
id = "planning"
uses = "plan"
role = "recur.roles.planner"
prompt = "/review later"
gate = "plan_approval"
next = "converge"

[[loop.phase]]
id = "converge"
uses = "converge"
next = "pr"

[loop.phase.roles]
implementer = "recur.roles.coder"
reviewer = "recur.roles.reviewer"

[loop.phase.prompts]
review = "/review"
fix = "/fix-findings"
rereview = "/re-review"

[[loop.phase]]
id = "pr"
uses = "ensure_pr"

[[loop.phase]]
id = "security"
uses = "converge"
dispatch = "on_demand"
max_rounds = 2
on_max_rounds = "proceed"
on_fail = "skip"

[loop.phase.roles]
implementer = "recur.roles.coder"
reviewer = "recur.roles.reviewer"
`

func TestParseManifestLoop(t *testing.T) {
	dir := t.TempDir()
	writeLoopBundle(t, dir, loopFixture)
	data, err := os.ReadFile(filepath.Join(dir, "bundle.toml"))
	if err != nil {
		t.Fatal(err)
	}
	parsed, err := bundles.ParseManifestFull(dir, data)
	if err != nil {
		t.Fatal(err)
	}
	if len(parsed.Roles) != 4 {
		t.Fatalf("roles: got %d", len(parsed.Roles))
	}
	if len(parsed.Policies) != 1 || parsed.Policies[0].MaxRounds != 4 {
		t.Fatalf("policy: %+v", parsed.Policies)
	}
	if parsed.Policies[0].FindingsThreshold.Levels["medium"] != 0 {
		t.Fatalf("balanced should name medium=0: %+v", parsed.Policies[0].FindingsThreshold)
	}
	if len(parsed.Loops) != 1 {
		t.Fatalf("loops: got %d", len(parsed.Loops))
	}
	lp := parsed.Loops[0]
	if lp.Roles["conductor"] != "recur.roles.maestro" {
		t.Fatalf("conductor normalize: %+v", lp.Roles)
	}
	plan, ok := lp.PhaseByID("planning")
	if !ok || plan.Roles["planner"] != "recur.roles.planner" {
		t.Fatalf("planning role sugar: %+v", plan.Roles)
	}
	sec, ok := lp.PhaseByID("security")
	if !ok || sec.Dispatch != "on_demand" {
		t.Fatalf("security: %+v", sec)
	}
}

func TestParseLoopRejectsNextCycle(t *testing.T) {
	dir := t.TempDir()
	toml := `
[bundle]
name = "X"
id = "x"
[[role]]
id = "recur.roles.maestro"
name = "Maestro"
code = "MA"
mode = "loop"
[[role]]
id = "recur.roles.coder"
name = "Coder"
code = "CO"
mode = "agent"
[[role]]
id = "recur.roles.reviewer"
name = "Reviewer"
code = "RV"
mode = "review"
[[loop]]
id = "x.loops.cycle"
name = "Cycle"
conductor = "recur.roles.maestro"
entry = "a"
[[loop.phase]]
id = "a"
uses = "converge"
next = "b"
[loop.phase.roles]
implementer = "recur.roles.coder"
reviewer = "recur.roles.reviewer"
[[loop.phase]]
id = "b"
uses = "converge"
next = "a"
[loop.phase.roles]
implementer = "recur.roles.coder"
reviewer = "recur.roles.reviewer"
`
	writeLoopBundle(t, dir, toml)
	data, _ := os.ReadFile(filepath.Join(dir, "bundle.toml"))
	_, err := bundles.ParseManifestFull(dir, data)
	if err == nil || !strings.Contains(err.Error(), "cycle") {
		t.Fatalf("want cycle error, got %v", err)
	}
}

func TestParseLoopRejectsOnDemandReenter(t *testing.T) {
	dir := t.TempDir()
	toml := `
[bundle]
name = "X"
id = "x"
[[role]]
id = "recur.roles.maestro"
name = "Maestro"
code = "MA"
mode = "loop"
[[role]]
id = "recur.roles.coder"
name = "Coder"
code = "CO"
mode = "agent"
[[role]]
id = "recur.roles.reviewer"
name = "Reviewer"
code = "RV"
mode = "review"
[[loop]]
id = "x.loops.x"
name = "X"
conductor = "recur.roles.maestro"
entry = "a"
reenter = ["sec"]
[[loop.phase]]
id = "a"
uses = "converge"
[loop.phase.roles]
implementer = "recur.roles.coder"
reviewer = "recur.roles.reviewer"
[[loop.phase]]
id = "sec"
uses = "converge"
dispatch = "on_demand"
[loop.phase.roles]
implementer = "recur.roles.coder"
reviewer = "recur.roles.reviewer"
`
	writeLoopBundle(t, dir, toml)
	data, _ := os.ReadFile(filepath.Join(dir, "bundle.toml"))
	_, err := bundles.ParseManifestFull(dir, data)
	if err == nil || !strings.Contains(err.Error(), "on-demand") {
		t.Fatalf("want on-demand reenter error, got %v", err)
	}
}

func TestParseLoopPromptPathMissing(t *testing.T) {
	dir := t.TempDir()
	toml := `
[bundle]
name = "X"
id = "x"
[[role]]
id = "recur.roles.maestro"
name = "Maestro"
code = "MA"
mode = "loop"
[[role]]
id = "recur.roles.planner"
name = "Planner"
code = "PL"
mode = "plan"
[[loop]]
id = "x.loops.x"
name = "X"
conductor = "recur.roles.maestro"
entry = "planning"
[[loop.phase]]
id = "planning"
uses = "plan"
role = "recur.roles.planner"
prompt = "prompts/planing.md"
`
	writeLoopBundle(t, dir, toml)
	data, _ := os.ReadFile(filepath.Join(dir, "bundle.toml"))
	_, err := bundles.ParseManifestFull(dir, data)
	if err == nil || !strings.Contains(err.Error(), "missing path") {
		t.Fatalf("want missing path, got %v", err)
	}
}

func TestSameOrStricter(t *testing.T) {
	pol := bundles.FindingsThreshold{Levels: map[string]int{"critical": 0, "high": 0, "medium": 0}}
	if err := bundles.SameOrStricter(bundles.FindingsThreshold{AllZero: true}, pol); err != nil {
		t.Fatalf("all-zero is stricter: %v", err)
	}
	loose := bundles.FindingsThreshold{Levels: map[string]int{"critical": 3}}
	if err := bundles.SameOrStricter(loose, pol); err == nil {
		t.Fatal("raising critical and dropping medium/high must be looser")
	}
}

func TestLooksLikePromptPath(t *testing.T) {
	cases := []struct {
		s    string
		want bool
	}{
		{"/review", false},
		{"@docs/spec.md", false},
		{"use the /skill to do x", false},
		{"prompts/plan.md", true},
		{"readme.md", true},
		{"just prose", false},
	}
	for _, c := range cases {
		if got := bundles.LooksLikePromptPath(c.s); got != c.want {
			t.Errorf("LooksLikePromptPath(%q)=%v want %v", c.s, got, c.want)
		}
	}
}

func TestFindingsBlock(t *testing.T) {
	bal := bundles.FindingsThreshold{Levels: map[string]int{"critical": 0, "high": 0, "medium": 0}}
	if bundles.FindingsBlock(bal, map[string]int{"low": 3}) {
		t.Fatal("leftover low must not block Balanced")
	}
	if !bundles.FindingsBlock(bal, map[string]int{"medium": 1}) {
		t.Fatal("medium must block Balanced")
	}
	zero := bundles.FindingsThreshold{AllZero: true}
	if !bundles.FindingsBlock(zero, map[string]int{"info": 1}) {
		t.Fatal("all-zero blocks info")
	}
}

func TestDiscoverLoopUserWins(t *testing.T) {
	def := t.TempDir()
	user := t.TempDir()
	writeBundle(t, def, "Loop", loopFixture)
	userTOML := `
[bundle]
name = "Loop"
id = "recur.loop"
[[policy]]
id = "recur.policies.balanced"
name = "Ship it"
max_rounds = 9
on_max_rounds = "proceed"
plan_approval = "never"
`
	writeBundle(t, user, "Loop", userTOML)
	cat, err := bundles.DiscoverCatalog(bundles.DiscoverRoots{UserDir: user, DefaultDir: def})
	if err != nil {
		t.Fatal(err)
	}
	p, ok := bundles.FindPolicy(cat.Policies, "recur.policies.balanced")
	if !ok || p.Name != "Ship it" || p.MaxRounds != 9 {
		t.Fatalf("user policy should win: %+v ok=%v", p, ok)
	}
}

func TestParseShippedLoopPack(t *testing.T) {
	dir := filepath.Join("..", "..", "bundles", "Loop")
	data, err := os.ReadFile(filepath.Join(dir, "bundle.toml"))
	if err != nil {
		t.Skip(err)
	}
	parsed, err := bundles.ParseManifestFull(dir, data)
	if err != nil {
		t.Fatal(err)
	}
	lp, ok := bundles.FindLoop(parsed.Loops, bundles.DefaultLoopID)
	if !ok {
		t.Fatal("missing recur.loops.pr")
	}
	if !strings.Contains(lp.Prompts["mission"], "Ship a correct") {
		t.Fatalf("mission path should load prompts/pr.md, got %q", lp.Prompts["mission"])
	}
	if _, ok := lp.PhaseByID("pr"); !ok {
		t.Fatal("To a PR missing pr phase")
	}
	if _, ok := lp.PhaseByID("security"); !ok {
		t.Fatal("To a PR missing security phase")
	}
	cvPh, ok := lp.PhaseByID("converge")
	if !ok || cvPh.Next != "pr" {
		t.Errorf("To a PR converge next = %q, want pr", cvPh.Next)
	}
	cv, ok := bundles.FindLoop(parsed.Loops, bundles.ConvergeLoopID)
	if !ok {
		t.Fatal("missing recur.loops.converge")
	}
	if cv.Entry != "converge" {
		t.Errorf("converge loop entry = %q, want converge", cv.Entry)
	}
	ph, ok := cv.PhaseByID("converge")
	if !ok || ph.Start != "review" || ph.Next != "" {
		t.Errorf("converge phase: start=%q next=%q, want start=review and no next", ph.Start, ph.Next)
	}
	if !strings.Contains(cv.Prompts["mission"], "already wrote") {
		t.Errorf("converge mission should load prompts/converge.md, got %q", cv.Prompts["mission"])
	}
	cvPR, ok := bundles.FindLoop(parsed.Loops, bundles.ConvergePRLoopID)
	if !ok {
		t.Fatal("missing recur.loops.converge-pr")
	}
	if cvPR.Entry != "converge" {
		t.Errorf("converge-pr entry = %q, want converge", cvPR.Entry)
	}
	cvPRPh, ok := cvPR.PhaseByID("converge")
	if !ok || cvPRPh.Start != "review" || cvPRPh.Next != "pr" {
		t.Errorf("converge-pr phase: start=%q next=%q, want start=review next=pr", cvPRPh.Start, cvPRPh.Next)
	}
	if _, ok := cvPR.PhaseByID("pr"); !ok {
		t.Fatal("converge-pr missing pr phase")
	}
	if !strings.Contains(cvPR.Prompts["mission"], "then open a PR") {
		t.Errorf("converge-pr mission should load prompts/converge-pr.md, got %q", cvPR.Prompts["mission"])
	}
	docs, ok := bundles.FindLoop(parsed.Loops, bundles.DocsLoopID)
	if !ok {
		t.Fatal("missing recur.loops.docs")
	}
	if docs.Entry != "planning" {
		t.Errorf("docs entry = %q, want planning", docs.Entry)
	}
	if !containsString(docs.Reenter, "planning") || !containsString(docs.Reenter, "converge") {
		t.Errorf("docs reenter = %v, want planning and converge", docs.Reenter)
	}
	if !strings.Contains(docs.Prompts["mission"], "Do not open a PR") {
		t.Errorf("docs mission should load prompts/docs.md, got %q", docs.Prompts["mission"])
	}
	docsConv, ok := docs.PhaseByID("converge")
	if !ok || docsConv.Findings == nil || !docsConv.Findings.AllZero || docsConv.Next != "done" {
		t.Errorf("docs converge: threshold=%+v next=%q", docsConv.Findings, docsConv.Next)
	}
	if docsConv.Roles["implementer"] != "recur.roles.writer" || docsConv.Roles["reviewer"] != "recur.roles.editor" {
		t.Errorf("docs converge roles = %+v", docsConv.Roles)
	}
	if done, ok := docs.PhaseByID("done"); !ok || done.Uses != bundles.VerbArtifact {
		t.Errorf("docs done phase: %+v ok=%v", done, ok)
	}
	if _, ok := docs.PhaseByID("pr"); ok {
		t.Error("docs loop must not have a pr phase")
	}
	if _, ok := bundles.FindLoop(parsed.Loops, bundles.StepLoopID); ok {
		t.Error("the fan-out step loop is engine-synthesized from the parent phase, not shipped in the bundle")
	}
	secLoop, ok := bundles.FindLoop(parsed.Loops, bundles.SecurityLoopID)
	if !ok {
		t.Fatal("missing recur.loops.security")
	}
	if secLoop.Entry != "converge" {
		t.Errorf("security entry = %q, want converge", secLoop.Entry)
	}
	if !strings.Contains(secLoop.Prompts["mission"], "Do not reopen product scope") {
		t.Errorf("security mission should load prompts/security.md, got %q", secLoop.Prompts["mission"])
	}
	secPh, ok := secLoop.PhaseByID("converge")
	if !ok || secPh.Start != "review" || secPh.Next != "" {
		t.Errorf("security converge: start=%q next=%q", secPh.Start, secPh.Next)
	}
	if secPh.Roles["implementer"] != "recur.roles.coder" || secPh.Roles["reviewer"] != "recur.roles.security-reviewer" {
		t.Errorf("security converge roles = %+v", secPh.Roles)
	}
	if _, ok := secLoop.PhaseByID("pr"); ok {
		t.Error("security loop must not have a pr phase")
	}
	if _, ok := bundles.FindRole(parsed.Roles, "recur.roles.writer"); !ok {
		t.Fatal("missing recur.roles.writer")
	}
	if _, ok := bundles.FindRole(parsed.Roles, "recur.roles.editor"); !ok {
		t.Fatal("missing recur.roles.editor")
	}
	if _, ok := bundles.FindPolicy(parsed.Policies, bundles.DefaultPolicyID); !ok {
		t.Fatal("missing balanced policy")
	}
	if len(parsed.Roles) < 8 {
		t.Fatalf("roles: %d", len(parsed.Roles))
	}
}

func containsString(xs []string, want string) bool {
	for _, x := range xs {
		if x == want {
			return true
		}
	}
	return false
}

func TestPhaseFindingsIntegerNLints(t *testing.T) {
	dir := t.TempDir()
	toml := `
[bundle]
name = "X"
id = "x"
[[role]]
id = "recur.roles.maestro"
name = "Maestro"
code = "MA"
mode = "loop"
[[role]]
id = "recur.roles.coder"
name = "Coder"
code = "CO"
mode = "agent"
[[role]]
id = "recur.roles.reviewer"
name = "Reviewer"
code = "RV"
mode = "review"
[[loop]]
id = "x.loops.x"
name = "X"
conductor = "recur.roles.maestro"
entry = "c"
[[loop.phase]]
id = "c"
uses = "converge"
findings_threshold = 3
[loop.phase.roles]
implementer = "recur.roles.coder"
reviewer = "recur.roles.reviewer"
`
	writeLoopBundle(t, dir, toml)
	data, _ := os.ReadFile(filepath.Join(dir, "bundle.toml"))
	_, err := bundles.ParseManifestFull(dir, data)
	if err == nil || !strings.Contains(err.Error(), "must be 0") {
		t.Fatalf("want integer N lint, got %v", err)
	}
}
