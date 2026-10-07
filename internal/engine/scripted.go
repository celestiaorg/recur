package engine

import (
	"context"
	"os"
	"path/filepath"
)

// ScriptedRunner is a TEST DOUBLE AgentRunner — NOT the production default. It
// replays a canned sequence of stream-json lines (parsed by the same parser the
// production ClaudeRunner uses) and optionally performs a scripted file edit in
// the worktree, so the engine, state machine, and persistence can be exercised
// deterministically WITHOUT invoking real `claude` (no API budget, no auth).
//
// It is intentionally defined in non-test code so tests across packages (engine,
// app) can inject it, but it is never wired as the production runner — main.go and
// the engine default both use ClaudeRunner.
type ScriptedRunner struct {
	// Lines are raw NDJSON stream-json lines to replay, in order. They are parsed
	// through parseStreamLine exactly like real claude output, so a ScriptedRunner
	// test also exercises the parser end to end.
	Lines [][]byte

	// Edits, if set, are applied to the worktree before the lines are replayed,
	// simulating the file changes a real Coder would make (so the engine's commit
	// step has something to commit). Keyed by repo-relative path → file contents.
	Edits map[string]string

	// OnRun, if set, is called once at the start of each Run with the spec — handy
	// for tests that want to observe which roles/tools the engine requested.
	OnRun func(AgentSpec)
}

// Run applies any scripted edits, then replays the canned lines through the real
// parser, emitting each parsed event and returning the final Result. It honors
// ctx cancellation between lines so Stop-during-agent tests behave realistically.
// When spec.Inject is set, any queued steer is drained after each tool_result
// (mirroring ClaudeRunner's D49 mid-turn inject timing) so engine tests can assert
// in-flight steering without spawning real claude.
func (r *ScriptedRunner) Run(ctx context.Context, spec AgentSpec, emit func(ParsedEvent)) (Result, error) {
	if r.OnRun != nil {
		r.OnRun(spec)
	}

	if err := r.applyEdits(spec.Worktree); err != nil {
		return Result{}, err
	}

	var final Result
	haveResult := false
	var ctxUsage contextUsageTracker
	for _, line := range r.Lines {
		if err := ctx.Err(); err != nil {
			return final, err
		}
		evs, perr := parseStreamLine(line)
		if perr != nil {
			continue
		}
		for _, ev := range evs {
			// Prefer last assistant.message.usage for meter fields before emit
			// so the mirror/broadcast never see cumulative result.usage.
			ctxUsage.observe(&ev)
			if ev.Kind == KindResult && ev.Result != nil {
				final = *ev.Result
				haveResult = true
			}
			// Event mirroring happens in the mirroringRunner's wrapped emit (mirror.go);
			// the raw line is mirrored above. Here we only forward to emit.
			if emit != nil {
				emit(ev)
			}
			// Mirror ClaudeRunner: after a tool_result, drain one pending steer inject.
			if ev.Kind == KindToolResult && spec.Inject != nil {
				select {
				case <-spec.Inject:
				default:
				}
			}
		}
	}
	if !haveResult {
		// A scripted turn with no result line still returns a benign success so the
		// engine can advance; tests that care should include a result line.
		final = Result{Subtype: "success"}
	}
	return final, nil
}

func (r *ScriptedRunner) applyEdits(worktree string) error {
	if len(r.Edits) == 0 || worktree == "" {
		return nil
	}
	for rel, content := range r.Edits {
		path := filepath.Join(worktree, rel)
		if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
			return err
		}
		if err := os.WriteFile(path, []byte(content), 0o644); err != nil { //nolint:gosec // test fixture
			return err
		}
	}
	return nil
}
