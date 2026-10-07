package bundles_test

import (
	"context"
	"strings"
	"testing"
	"time"

	"recur/internal/bundles"
)

func TestRunEnvStdinStdoutAndInput(t *testing.T) {
	cmd := bundles.Command{
		ID:          "echo-env",
		Name:        "Echo",
		Interpreter: bundles.InterpreterShebang,
		Output:      bundles.OutputReplaceSelection,
		Input:       bundles.InputSelection,
		Body: `#!/bin/sh
printf 'in=%s\n' "$(cat)"
printf 'clip=%s\n' "$RECUR_CLIPBOARD"
printf 'width=%s\n' "$RECUR_INPUT_WIDTH"
printf 'scope=%s\n' "$RECUR_SCOPE"
`,
	}
	rc := bundles.RunContext{
		SelectedText: "hello",
		Clipboard:    "paste",
		Scope:        "source.go",
		PromptValues: map[string]string{"width": "72"},
		WorkDir:      t.TempDir(),
	}
	res, err := bundles.Run(context.Background(), cmd, rc)
	if err != nil {
		t.Fatal(err)
	}
	if res.ExitCode != 0 {
		t.Fatalf("exit %d stderr=%q", res.ExitCode, res.Stderr)
	}
	out := res.Stdout
	for _, want := range []string{"in=hello", "clip=paste", "width=72", "scope=source.go"} {
		if !strings.Contains(out, want) {
			t.Fatalf("stdout missing %q:\n%s", want, out)
		}
	}
}

func TestRunNonZeroExitCaptured(t *testing.T) {
	cmd := bundles.Command{
		ID:          "fail",
		Name:        "Fail",
		Interpreter: bundles.InterpreterShebang,
		Body:        "#!/bin/sh\necho oops >&2\nexit 3\n",
	}
	res, err := bundles.Run(context.Background(), cmd, bundles.RunContext{WorkDir: t.TempDir()})
	if err != nil {
		t.Fatalf("non-zero should not be transport error: %v", err)
	}
	if res.ExitCode != 3 {
		t.Fatalf("exit=%d", res.ExitCode)
	}
	if !strings.Contains(res.Stderr, "oops") {
		t.Fatalf("stderr=%q", res.Stderr)
	}
}

func TestRunTimeoutAndCancel(t *testing.T) {
	cmd := bundles.Command{
		ID:          "sleep",
		Name:        "Sleep",
		Interpreter: bundles.InterpreterShebang,
		Body:        "#!/bin/sh\nsleep 5\n",
	}
	ctx, cancel := context.WithTimeout(context.Background(), 200*time.Millisecond)
	defer cancel()
	_, err := bundles.Run(ctx, cmd, bundles.RunContext{WorkDir: t.TempDir()})
	if err == nil {
		t.Fatal("expected timeout error")
	}
	if !strings.Contains(err.Error(), "cancelled") && !strings.Contains(err.Error(), "timed out") {
		t.Fatalf("err=%v", err)
	}
}

func TestEnvironStripsStaleRecurVars(t *testing.T) {
	t.Setenv("RECUR_CLIPBOARD", "stale")
	t.Setenv("PATH", "/usr/bin:/bin")
	env := bundles.Environ(bundles.RunContext{Clipboard: "fresh", SoftTabs: true, TabSize: 4})
	var clip, soft string
	for _, e := range env {
		if strings.HasPrefix(e, "RECUR_CLIPBOARD=") {
			clip = e
		}
		if strings.HasPrefix(e, "RECUR_SOFT_TABS=") {
			soft = e
		}
	}
	if clip != "RECUR_CLIPBOARD=fresh" {
		t.Fatalf("clipboard=%q", clip)
	}
	if soft != "RECUR_SOFT_TABS=1" {
		t.Fatalf("soft=%q", soft)
	}
}

func TestEnvironBundleSettings(t *testing.T) {
	env := bundles.Environ(bundles.RunContext{
		BundleSettings: map[string]string{"api_token": "secret"},
	})
	var token, blob string
	for _, e := range env {
		if strings.HasPrefix(e, "RECUR_SETTING_API_TOKEN=") {
			token = strings.TrimPrefix(e, "RECUR_SETTING_API_TOKEN=")
		}
		if strings.HasPrefix(e, "RECUR_BUNDLE_SETTINGS=") {
			blob = strings.TrimPrefix(e, "RECUR_BUNDLE_SETTINGS=")
		}
	}
	if token != "secret" {
		t.Fatalf("RECUR_SETTING_API_TOKEN=%q", token)
	}
	if !strings.Contains(blob, `"api_token":"secret"`) {
		t.Fatalf("RECUR_BUNDLE_SETTINGS=%q", blob)
	}
}
