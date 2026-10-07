package engine

import (
	"context"
	"errors"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"recur/internal/bundles"
	"recur/internal/store"
)

func writePromptBundle(t *testing.T, dataDir, slash, body, id string) {
	t.Helper()
	dir := filepath.Join(dataDir, "Bundles", "ChatTest")
	if err := os.MkdirAll(filepath.Join(dir, "src"), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(dir, "src", "act.sh"), []byte(body), 0o755); err != nil {
		t.Fatal(err)
	}
	toml := "[bundle]\nname = \"ChatTest\"\nid = \"recur.chattest\"\nversion = \"1.0.0\"\n\n" +
		"[[prompt]]\nid = \"" + id + "\"\nname = \"Act\"\nslash = \"" + slash + "\"\n" +
		"interpreter = \"shebang\"\nscript = \"src/act.sh\"\n"
	if err := os.WriteFile(filepath.Join(dir, "bundle.toml"), []byte(toml), 0o644); err != nil {
		t.Fatal(err)
	}
}

func TestExpandComposerWrapsMentions(t *testing.T) {
	eng, _, _, _ := newTestEngine(t, &ScriptedRunner{})
	root := t.TempDir()
	if err := os.WriteFile(filepath.Join(root, "README.md"), []byte("hi"), 0o644); err != nil {
		t.Fatal(err)
	}
	got, err := eng.ExpandComposer(context.Background(), "please read @README.md", expandOpts{worktree: root})
	if err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(got.Prompt, "<system_instruction>") {
		t.Fatalf("expected @file wrap: %q", got.Prompt)
	}
	if got.RanAction {
		t.Fatal("plain text must not run a prompt action")
	}
}

func TestExpandComposerRunsShebangAndWraps(t *testing.T) {
	home := t.TempDir()
	t.Setenv("HOME", home)
	eng, _, _, dataDir := newTestEngine(t, &ScriptedRunner{})
	body := "#!/bin/sh\nprintf '%s\\n' '{\"prompt\":\"wire @README.md\",\"display\":\"Act\"}'\n"
	writePromptBundle(t, dataDir, "act", body, "recur.chattest.act")
	root := t.TempDir()
	if err := os.WriteFile(filepath.Join(root, "README.md"), []byte("hi"), 0o644); err != nil {
		t.Fatal(err)
	}
	got, err := eng.ExpandComposer(context.Background(), "/act", expandOpts{
		worktree: root, ignoreDisabled: true,
	})
	if err != nil {
		t.Fatal(err)
	}
	if !got.RanAction {
		t.Fatal("expected prompt action")
	}
	if got.Display != "Act" {
		t.Fatalf("display=%q", got.Display)
	}
	if !strings.Contains(got.Prompt, "<system_instruction>") {
		t.Fatalf("action result should wrap @file: %q", got.Prompt)
	}
	if !strings.Contains(got.Prompt, "wire @README.md") {
		t.Fatalf("action wire lost: %q", got.Prompt)
	}
}

func TestExpandComposerEmptyActionErrors(t *testing.T) {
	home := t.TempDir()
	t.Setenv("HOME", home)
	eng, _, _, dataDir := newTestEngine(t, &ScriptedRunner{})
	body := "#!/bin/sh\nprintf '%s\\n' '{\"prompt\":\"\",\"display\":\"Done\"}'\n"
	writePromptBundle(t, dataDir, "empty", body, "recur.chattest.empty")
	_, err := eng.ExpandComposer(context.Background(), "/empty", expandOpts{ignoreDisabled: true})
	if !errors.Is(err, ErrEmptyPromptAction) {
		t.Fatalf("err=%v, want ErrEmptyPromptAction", err)
	}
}

func TestExpandComposerHonorsDisabledUnlessIgnored(t *testing.T) {
	home := t.TempDir()
	t.Setenv("HOME", home)
	eng, st, _, dataDir := newTestEngine(t, &ScriptedRunner{})
	body := "#!/bin/sh\nprintf '%s\\n' '{\"prompt\":\"WIRE\",\"display\":\"Act\"}'\n"
	writePromptBundle(t, dataDir, "off", body, "recur.chattest.off")
	set := st.Settings()
	set.DisabledBundlePrompts = []string{"recur.chattest.off"}
	st.SetSettings(set)

	skipped, err := eng.ExpandComposer(context.Background(), "/off", expandOpts{})
	if err != nil {
		t.Fatal(err)
	}
	if skipped.RanAction || skipped.Prompt != "/off" {
		t.Fatalf("disabled action should stay literal: %+v", skipped)
	}

	ran, err := eng.ExpandComposer(context.Background(), "/off", expandOpts{ignoreDisabled: true})
	if err != nil {
		t.Fatal(err)
	}
	if !ran.RanAction || ran.Prompt != "WIRE" {
		t.Fatalf("loop ignore-disabled: %+v", ran)
	}
}

func TestExpandComposerTrailingTextDoesNotFire(t *testing.T) {
	home := t.TempDir()
	t.Setenv("HOME", home)
	eng, _, _, dataDir := newTestEngine(t, &ScriptedRunner{})
	body := "#!/bin/sh\nprintf '%s\\n' '{\"prompt\":\"WIRE\",\"display\":\"Act\"}'\n"
	writePromptBundle(t, dataDir, "act", body, "recur.chattest.act")
	got, err := eng.ExpandComposer(context.Background(), "/act now", expandOpts{ignoreDisabled: true})
	if err != nil {
		t.Fatal(err)
	}
	if got.RanAction {
		t.Fatal("trailing text must not fire the action (same as composer / menu)")
	}
	if got.Prompt != "/act now" {
		t.Fatalf("got %q", got.Prompt)
	}
}

func TestLeadingPromptSlash(t *testing.T) {
	if s, ok := leadingPromptSlash("  /review  "); !ok || s != "review" {
		t.Fatalf("got %q %v", s, ok)
	}
	if _, ok := leadingPromptSlash("/review extra"); ok {
		t.Fatal("extra text must not match")
	}
	if _, ok := leadingPromptSlash("see /review"); ok {
		t.Fatal("not leading")
	}
}

func TestResolveLoopMissionPath(t *testing.T) {
	t.Setenv("HOME", t.TempDir())
	eng, _, _, _ := newTestEngine(t, &ScriptedRunner{})
	dst, err := store.BundlesDir()
	if err != nil {
		t.Fatal(err)
	}
	pack := filepath.Join(dst, "X")
	if err := os.MkdirAll(filepath.Join(pack, "prompts"), 0o755); err != nil {
		t.Fatal(err)
	}
	toml := `[bundle]
name = "X"
id = "x.loop"
version = "1.0.0"

[[role]]
id = "x.roles.ma"
name = "Maestro"
code = "MA"
mode = "loop"
prompt = "hello"

[[loop]]
id = "x.loops.one"
name = "One"
conductor = "x.roles.ma"
prompt = "prompts/mission.md"
entry = "done"

[[loop.phase]]
id = "done"
uses = "artifact"
`
	if err := os.WriteFile(filepath.Join(pack, "bundle.toml"), []byte(toml), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(pack, "prompts", "mission.md"), []byte("Ship it."), 0o644); err != nil {
		t.Fatal(err)
	}
	cat, err := eng.discoverLoopCatalog()
	if err != nil {
		t.Fatal(err)
	}
	lp, ok := bundles.FindLoop(cat.Loops, "x.loops.one")
	if !ok {
		t.Fatal("loop not found")
	}
	if strings.TrimSpace(lp.Prompts["mission"]) != "Ship it." {
		t.Fatalf("mission=%q, want loaded body", lp.Prompts["mission"])
	}
}
