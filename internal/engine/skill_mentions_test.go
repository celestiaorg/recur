package engine

import (
	"os"
	"path/filepath"
	"strings"
	"testing"

	"recur/internal/domain"
)

func writeTestSkill(t *testing.T, dir, name, body string) string {
	t.Helper()
	p := filepath.Join(dir, name)
	if err := os.MkdirAll(p, 0o755); err != nil {
		t.Fatal(err)
	}
	path := filepath.Join(p, "SKILL.md")
	if err := os.WriteFile(path, []byte(body), 0o644); err != nil {
		t.Fatal(err)
	}
	return path
}

func TestCollectSkillMentions(t *testing.T) {
	got := collectSkillMentions("/go-code-review please and /go-code-review again then /other")
	if len(got) != 2 || got[0] != "go-code-review" || got[1] != "other" {
		t.Fatalf("got %#v", got)
	}
	// Absolute paths must not match.
	if got := collectSkillMentions("see /Users/josh/file.go"); len(got) != 0 {
		t.Fatalf("path matched: %#v", got)
	}
}

func TestWrapActivatedSkills(t *testing.T) {
	wt := t.TempDir()
	home := t.TempDir()
	t.Setenv("HOME", home)

	skillDir := filepath.Join(home, ".agents", "skills", "go-code-review")
	writeTestSkill(t, filepath.Join(home, ".agents", "skills"), "go-code-review", `---
name: go-code-review
description: Review Go changes.
---
# Go review

Check error handling.
`)
	if err := os.MkdirAll(filepath.Join(skillDir, "scripts"), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(skillDir, "scripts", "lint.sh"), []byte("#!/bin/sh\n"), 0o644); err != nil {
		t.Fatal(err)
	}

	in := "/go-code-review look at internal/engine"
	out := wrapActivatedSkills(in, wt, nil)
	if !strings.Contains(out, `<skill_content name="go-code-review">`) {
		t.Fatalf("missing skill_content: %s", out)
	}
	if !strings.Contains(out, "Check error handling.") {
		t.Fatalf("missing body: %s", out)
	}
	if !strings.Contains(out, "<file>scripts/lint.sh</file>") {
		t.Fatalf("missing resources: %s", out)
	}
	// Slash stripped from model body so Claude won't also expand it.
	// (Skill directory paths may still contain /go-code-review as a folder name.)
	if i := strings.LastIndex(out, "</skill_content>"); i >= 0 {
		body := out[i:]
		if strings.Contains(body, "/go-code-review") {
			t.Fatalf("slash token should be stripped from user body: %s", body)
		}
	}
	if !strings.Contains(out, "look at internal/engine") {
		t.Fatalf("user text lost: %s", out)
	}
	// Unknown skill left alone.
	unk := wrapActivatedSkills("/no-such-skill hi", wt, nil)
	if unk != "/no-such-skill hi" {
		t.Fatalf("unknown skill changed prompt: %q", unk)
	}
	// Settings → Skills disable skips host slash activation and strips the
	// token so Claude Code cannot natively expand it.
	dis := wrapActivatedSkills(in, wt, func(name string) bool { return name != "go-code-review" })
	if strings.Contains(dis, `<skill_content name="go-code-review">`) {
		t.Fatalf("disabled skill should not host-activate: %q", dis)
	}
	if strings.Contains(dis, "/go-code-review") {
		t.Fatalf("disabled slash token should be stripped: %q", dis)
	}
	if !strings.Contains(dis, "disabled") {
		t.Fatalf("expected disabled note: %q", dis)
	}
	if !strings.Contains(dis, "look at internal/engine") {
		t.Fatalf("user text lost: %q", dis)
	}
}

func TestWrapActivatedSkillsAdjacentMentions(t *testing.T) {
	wt := t.TempDir()
	home := t.TempDir()
	t.Setenv("HOME", home)
	root := filepath.Join(home, ".agents", "skills")
	writeTestSkill(t, root, "solidity-security", `---
name: solidity-security
description: Solidity security.
---
SOLIDITY BODY
`)
	writeTestSkill(t, root, "smart-contract-audit", `---
name: smart-contract-audit
description: Smart contract audit.
---
AUDIT BODY
`)
	writeTestSkill(t, root, "go-code-review", `---
name: go-code-review
description: Review Go changes.
---
GO REVIEW BODY
`)

	bodies := map[string]string{
		"solidity-security":    "SOLIDITY BODY",
		"smart-contract-audit": "AUDIT BODY",
		"go-code-review":       "GO REVIEW BODY",
	}
	tests := []struct {
		name, in string
		want     []string
	}{
		{
			name: "two",
			in:   "review vs origin/main /solidity-security /smart-contract-audit",
			want: []string{"solidity-security", "smart-contract-audit"},
		},
		{
			name: "three",
			in:   "review vs origin/main /solidity-security /smart-contract-audit /go-code-review",
			want: []string{"solidity-security", "smart-contract-audit", "go-code-review"},
		},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			out := wrapActivatedSkills(tt.in, wt, nil)
			wantSet := map[string]bool{}
			for _, name := range tt.want {
				wantSet[name] = true
				if !strings.Contains(out, `<skill_content name="`+name+`">`) {
					t.Errorf("wrapActivatedSkills(%q) missing %s wrap: %s", tt.in, name, out)
				}
				if !strings.Contains(out, bodies[name]) {
					t.Errorf("wrapActivatedSkills(%q) missing %s body %q: %s", tt.in, name, bodies[name], out)
				}
			}
			for name, body := range bodies {
				if wantSet[name] {
					continue
				}
				if strings.Contains(out, `<skill_content name="`+name+`">`) || strings.Contains(out, body) {
					t.Errorf("wrapActivatedSkills(%q) injected unmentioned skill %s: %s", tt.in, name, out)
				}
			}
			if !strings.Contains(out, "review vs origin/main") {
				t.Errorf("wrapActivatedSkills(%q) lost user text: %s", tt.in, out)
			}
			if i := strings.LastIndex(out, "</skill_content>"); i >= 0 {
				user := out[i:]
				for _, name := range tt.want {
					if strings.Contains(user, "/"+name) {
						t.Errorf("wrapActivatedSkills(%q) left /%s in user body: %s", tt.in, name, user)
					}
				}
			}
		})
	}
}

func TestWrapActivatedSkillsWorkspaceWins(t *testing.T) {
	wt := t.TempDir()
	home := t.TempDir()
	t.Setenv("HOME", home)

	writeTestSkill(t, filepath.Join(home, ".claude", "skills"), "design-audit", `---
name: design-audit
description: user copy
---
USER BODY
`)
	writeTestSkill(t, filepath.Join(wt, ".claude", "skills"), "design-audit", `---
name: design-audit
description: workspace copy
---
WORKSPACE BODY
`)

	out := wrapActivatedSkills("/design-audit", wt, nil)
	if !strings.Contains(out, "WORKSPACE BODY") {
		t.Fatalf("want workspace body: %s", out)
	}
	if strings.Contains(out, "USER BODY") {
		t.Fatalf("user skill should be shadowed: %s", out)
	}
}

func TestSlashActivatesDisableModelInvocation(t *testing.T) {
	wt := t.TempDir()
	home := t.TempDir()
	t.Setenv("HOME", home)

	writeTestSkill(t, filepath.Join(home, ".agents", "skills"), "create-rule", `---
name: create-rule
description: Make a rule
disable-model-invocation: true
---
# Create
`)
	// User slash still works; model-driven activate_skill refuses separately.
	act := wrapActivatedSkills("/create-rule do it", wt, nil)
	if !strings.Contains(act, `<skill_content name="create-rule">`) {
		t.Fatalf("slash should still activate: %s", act)
	}
}

func TestJoinAppendPrompt(t *testing.T) {
	if got := joinAppendPrompt("a", "b"); got != "a\n\nb" {
		t.Fatalf("got %q", got)
	}
	if got := joinAppendPrompt("", "b"); got != "b" {
		t.Fatalf("got %q", got)
	}
}

func TestApplySkillGuidance(t *testing.T) {
	spec := AgentSpec{AllowedTools: []string{mcpListSkills}}
	applySkillGuidance(&spec)
	if !strings.Contains(spec.AppendSystemPrompt, "mcp__recur__list_skills") {
		t.Fatalf("missing skill guidance: %q", spec.AppendSystemPrompt)
	}
	dup := spec
	applySkillGuidance(&dup)
	if strings.Count(dup.AppendSystemPrompt, "mcp__recur__list_skills") != 1 {
		t.Fatalf("duplicated skill guidance: %q", dup.AppendSystemPrompt)
	}
	none := AgentSpec{}
	applySkillGuidance(&none)
	if none.AppendSystemPrompt != "" {
		t.Fatalf("no skill tools, got %q", none.AppendSystemPrompt)
	}
}

func TestStripActivatedSkillMentionsPreservesIndentation(t *testing.T) {
	in := "/go-code-review please\n\ndef main():\n    print('hi')\n"
	got := stripActivatedSkillMentions(in, []string{"go-code-review"})
	if !strings.Contains(got, "    print('hi')") {
		t.Fatalf("indentation collapsed: %q", got)
	}
	if strings.Contains(got, "/go-code-review") {
		t.Fatalf("slash not stripped: %q", got)
	}
}

// Adjacent `/a /b` share one space. Eating it as both the first token's
// trailer and the second's leader used to panic (slice [last:cutStart] with
// cutStart < last) and abort SendWorkspaceMessage before the user row landed.
func TestStripActivatedSkillMentionsAdjacent(t *testing.T) {
	names := []string{"solidity-security", "smart-contract-audit", "go-code-review"}
	tests := []struct {
		in, want string
	}{
		{"/solidity-security /smart-contract-audit", ""},
		{"review vs origin/main /solidity-security /smart-contract-audit", "review vs origin/main"},
		{"/solidity-security /smart-contract-audit\nkeep", "keep"},
		{"/solidity-security please /smart-contract-audit", "please"},
		{"x /solidity-security /smart-contract-audit y", "x y"},
		{"/solidity-security /smart-contract-audit /go-code-review", ""},
		{"review vs origin/main /solidity-security /smart-contract-audit /go-code-review", "review vs origin/main"},
		{"/solidity-security /smart-contract-audit /go-code-review\nkeep", "keep"},
		{"x /solidity-security /smart-contract-audit /go-code-review y", "x y"},
	}
	for _, tt := range tests {
		got := stripActivatedSkillMentions(tt.in, names)
		if got != tt.want {
			t.Errorf("stripActivatedSkillMentions(%q) = %q, want %q", tt.in, got, tt.want)
		}
	}
}

func TestWrapActivatedSkillsSkipsNonInvocable(t *testing.T) {
	wt := t.TempDir()
	home := t.TempDir()
	t.Setenv("HOME", home)

	writeTestSkill(t, filepath.Join(home, ".agents", "skills"), "hidden", `---
name: hidden
description: model only
user-invocable: false
---
# Hidden
`)
	in := "/hidden please"
	out := wrapActivatedSkills(in, wt, nil)
	if out != in {
		t.Fatalf("non-invocable skill should not host-activate: %q", out)
	}
}

// TestSendWorkspaceMessageSkillWrapDisplayText: /skill send stores full wire in
// content_json text and the composer string in display_text; flatten shows composer.
func TestSendWorkspaceMessageSkillWrapDisplayText(t *testing.T) {
	home := t.TempDir()
	t.Setenv("HOME", home)
	writeTestSkill(t, filepath.Join(home, ".agents", "skills"), "go-code-review", `---
name: go-code-review
description: Review Go changes.
---
# Go review

Check error handling.
`)

	runner := &captureRunner{
		linesFor: func(call int) [][]byte { return maestroFixture("skill-sess", "Reviewed.") },
	}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "scratch", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}

	composer := "/go-code-review look at auth"
	_ = eng.SendWorkspaceMessage(ws.ID, "", composer, "sonnet", nil, nil, "", "")

	s0, ok := runner.specWithRole(domain.RoleCoder)
	if !ok {
		t.Fatal("no coding-agent chat turn")
	}
	if !strings.Contains(s0.Prompt, `<skill_content name="go-code-review">`) {
		t.Fatalf("Prompt missing skill wire: %s", s0.Prompt)
	}
	if !RequiresDisplayText(s0.Prompt) {
		t.Fatal("wrapped Prompt must require display_text")
	}
	if s0.DisplayText != composer {
		t.Fatalf("DisplayText=%q, want composer %q", s0.DisplayText, composer)
	}

	sess, ok := st.SessionForLoopAgent(ws.ID, pairAgentCode)
	if !ok {
		t.Fatal("no chat session")
	}
	msgs, err := st.SessionTranscript(sess.ID)
	if err != nil || len(msgs) == 0 {
		t.Fatalf("transcript: %v", err)
	}
	p, ok := msgs[0].Content[0].AsText()
	if !ok {
		t.Fatalf("want text block: %+v", msgs[0].Content)
	}
	if p.Text != s0.Prompt {
		t.Fatalf("stored text should equal wire Prompt")
	}
	if p.DisplayText != composer {
		t.Fatalf("stored display_text=%q, want %q", p.DisplayText, composer)
	}
	items := eng.WorkspaceChatItems(ws.ID, "")
	if len(items) == 0 || items[0].Role != "user" || items[0].Text != composer {
		t.Fatalf("flatten ChatItem=%+v, want user text %q", items, composer)
	}
}

func TestSendWorkspaceMessageAdjacentSkills(t *testing.T) {
	home := t.TempDir()
	t.Setenv("HOME", home)
	root := filepath.Join(home, ".agents", "skills")
	writeTestSkill(t, root, "solidity-security", `---
name: solidity-security
description: Solidity security.
---
SOLIDITY BODY
`)
	writeTestSkill(t, root, "smart-contract-audit", `---
name: smart-contract-audit
description: Smart contract audit.
---
AUDIT BODY
`)
	writeTestSkill(t, root, "go-code-review", `---
name: go-code-review
description: Review Go changes.
---
GO REVIEW BODY
`)

	bodies := map[string]string{
		"solidity-security":    "SOLIDITY BODY",
		"smart-contract-audit": "AUDIT BODY",
		"go-code-review":       "GO REVIEW BODY",
	}
	tests := []struct {
		name, composer string
		want           []string
	}{
		{
			name:     "two",
			composer: "review vs origin/main /solidity-security /smart-contract-audit",
			want:     []string{"solidity-security", "smart-contract-audit"},
		},
		{
			name:     "three",
			composer: "review vs origin/main /solidity-security /smart-contract-audit /go-code-review",
			want:     []string{"solidity-security", "smart-contract-audit", "go-code-review"},
		},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			defer func() {
				if r := recover(); r != nil {
					t.Fatalf("SendWorkspaceMessage(%q) panicked: %v", tt.composer, r)
				}
			}()
			runner := &captureRunner{
				linesFor: func(call int) [][]byte { return maestroFixture("adj-skill-sess", "Reviewed.") },
			}
			eng, _, _, _ := newTestEngine(t, runner)
			projID := addProject(t, eng.Store, initRepo(t))
			ws, err := eng.CreateWorkspace(projID, "scratch", "")
			if err != nil {
				t.Fatalf("CreateWorkspace: %v", err)
			}

			_ = eng.SendWorkspaceMessage(ws.ID, "", tt.composer, "sonnet", nil, nil, "", "")

			s0, ok := runner.specWithRole(domain.RoleCoder)
			if !ok {
				t.Fatal("no coding-agent chat turn")
			}
			wantSet := map[string]bool{}
			for _, name := range tt.want {
				wantSet[name] = true
				if !strings.Contains(s0.Prompt, `<skill_content name="`+name+`">`) {
					t.Errorf("Prompt missing %s wrap: %s", name, s0.Prompt)
				}
				if !strings.Contains(s0.Prompt, bodies[name]) {
					t.Errorf("Prompt missing %s body %q: %s", name, bodies[name], s0.Prompt)
				}
			}
			for name, body := range bodies {
				if wantSet[name] {
					continue
				}
				if strings.Contains(s0.Prompt, `<skill_content name="`+name+`">`) || strings.Contains(s0.Prompt, body) {
					t.Errorf("Prompt injected unmentioned skill %s: %s", name, s0.Prompt)
				}
			}

			items := eng.WorkspaceChatItems(ws.ID, "")
			if len(items) == 0 || items[0].Role != "user" || items[0].Text != tt.composer {
				t.Errorf("flatten ChatItem=%+v, want user text %q", items, tt.composer)
			}
		})
	}
}

func TestSendWorkspaceMessageRetryReplaysStoredWire(t *testing.T) {
	home := t.TempDir()
	t.Setenv("HOME", home)
	writeTestSkill(t, filepath.Join(home, ".agents", "skills"), "go-code-review", `---
name: go-code-review
description: Review Go changes.
---
# Go review

Check error handling.
`)

	runner := &captureRunner{
		linesFor: func(call int) [][]byte { return maestroFixture("skill-retry", "Reviewed.") },
	}
	eng, st, _, _ := newTestEngine(t, runner)
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "scratch", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}

	composer := "/go-code-review look at auth"
	_ = eng.SendWorkspaceMessage(ws.ID, "", composer, "sonnet", nil, nil, "", "")
	_ = eng.SendWorkspaceMessage(ws.ID, "", "should-not-be-used", "sonnet", nil, nil, "", chatRetryDisplaySentinel)

	coders := runner.specsWithRole(domain.RoleCoder)
	if len(coders) < 2 {
		t.Fatalf("coder turns=%d, want 2", len(coders))
	}
	if coders[1].Prompt != coders[0].Prompt {
		t.Fatalf("retry Prompt=%q, want stored wire %q", coders[1].Prompt, coders[0].Prompt)
	}
	if !strings.Contains(coders[1].Prompt, `<skill_content name="go-code-review">`) {
		t.Fatalf("retry lost skill wrap: %s", coders[1].Prompt)
	}
	if !coders[1].SkipRecordUser {
		t.Fatal("retry must SkipRecordUser")
	}
	users := 0
	for _, it := range contentItems(eng.WorkspaceChatItems(ws.ID, "")) {
		if it.Role == "user" && it.Kind == "text" {
			users++
		}
	}
	if users != 1 {
		t.Fatalf("user bubbles=%d, want 1", users)
	}
}
