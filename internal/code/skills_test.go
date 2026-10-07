package code

import (
	"context"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"unicode/utf8"
)

func TestListSkillsUnionRoots(t *testing.T) {
	wt := t.TempDir()
	home := t.TempDir()
	t.Setenv("HOME", home)

	writeSkill := func(dir, name, body string) {
		t.Helper()
		p := filepath.Join(dir, name)
		if err := os.MkdirAll(p, 0o755); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(filepath.Join(p, "SKILL.md"), []byte(body), 0o644); err != nil {
			t.Fatal(err)
		}
	}

	writeSkill(filepath.Join(wt, ".claude", "skills"), "design-audit", `---
name: design-audit
description: >-
  Audit UI polish.
---
# Design
`)
	writeSkill(filepath.Join(home, ".cursor", "skills"), "create-rule", `---
name: create-rule
description: "Make a rule"
---
`)
	writeSkill(filepath.Join(home, ".claude", "skills"), "design-audit", `---
name: design-audit
description: user copy
---
`)
	writeSkill(filepath.Join(wt, ".cursor", "skills"), "BadName", `---
name: BadName
description: no
---
`)
	writeSkill(filepath.Join(wt, ".cursor", "skills"), "land-it", `---
name: land-it
description: Ship the PR
---
`)

	got, err := ListSkills(context.Background(), wt)
	if err != nil {
		t.Fatal(err)
	}
	names := map[string]string{}
	for _, s := range got {
		names[s.Name] = s.Scope
	}
	if names["design-audit"] != "workspace" {
		t.Fatalf("design-audit=%q, want workspace: %+v", names["design-audit"], got)
	}
	if names["create-rule"] != "user" {
		t.Fatalf("create-rule missing: %+v", got)
	}
	if names["land-it"] != "workspace" {
		t.Fatalf("land-it missing: %+v", got)
	}
	if _, ok := names["BadName"]; ok {
		t.Fatal("uppercase name should be skipped")
	}
	for _, s := range got {
		if s.Name != "design-audit" {
			continue
		}
		if s.Description != "Audit UI polish." {
			t.Fatalf("desc=%q", s.Description)
		}
		if s.Path == "" || !strings.HasSuffix(s.Path, "SKILL.md") {
			t.Fatalf("path=%q", s.Path)
		}
		return
	}
	t.Fatal("design-audit missing from result")
}

func TestParseFrontmatterDescription(t *testing.T) {
	if got := parseFrontmatterDescription("description: hello world\n"); got != "hello world" {
		t.Fatalf("got %q", got)
	}
	fm := "description: >\n  line one\n  line two\nname: x\n"
	if got := parseFrontmatterDescription(fm); got != "line one line two" {
		t.Fatalf("folded=%q", got)
	}
}

func TestReadSkillBodyStripsFrontmatter(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "SKILL.md")
	if err := os.WriteFile(path, []byte("---\nname: x\ndescription: d\n---\n\n# Hello\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	if got := ReadSkillBody(path); got != "# Hello" {
		t.Fatalf("body=%q", got)
	}
}

func TestListSkillsKeepsModelOnly(t *testing.T) {
	wt := t.TempDir()
	home := t.TempDir()
	t.Setenv("HOME", home)
	p := filepath.Join(home, ".claude", "skills", "hidden")
	if err := os.MkdirAll(p, 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(p, "SKILL.md"), []byte(`---
name: hidden
description: nope
user-invocable: false
---
# Hidden
`), 0o644); err != nil {
		t.Fatal(err)
	}
	got, err := ListSkills(context.Background(), wt)
	if err != nil {
		t.Fatal(err)
	}
	if len(got) != 1 || got[0].Name != "hidden" || got[0].UserInvocable {
		t.Fatalf("discovery should keep model-only skill: %+v", got)
	}
	menu, err := ListSkillsForMenu(context.Background(), wt)
	if err != nil {
		t.Fatal(err)
	}
	if len(menu) != 0 {
		t.Fatalf("menu should hide model-only: %+v", menu)
	}
}

func TestTruncateSkillDescUTF8(t *testing.T) {
	// Em-dash and accented chars must not be sliced mid-rune.
	in := strings.Repeat("é", 200)
	got := truncateSkillDesc(in, 160)
	if !utf8.ValidString(got) {
		t.Fatalf("invalid utf8: %q", got)
	}
	if !strings.HasSuffix(got, "…") {
		t.Fatalf("want ellipsis: %q", got)
	}
}
