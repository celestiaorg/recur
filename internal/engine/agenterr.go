package engine

import (
	"errors"
	"fmt"
	"strings"

	"recur/internal/cursor"
	"recur/internal/term"
)

// Transient agent failures (D56).
//
// An agent turn can fail for two very different reasons, and v1 treated them
// identically — both ended the loop as `failed`, terminal, with no way back:
//
//   - A GENUINE failure: the engine could not commit, a rail was violated, the agent
//     produced nothing usable. Re-running it would just fail again.
//   - A TRANSIENT failure: the org hit its monthly spend limit, a rate limit or 5xx
//     from the provider, the connection dropped. Nothing is wrong with the work — the
//     loop was simply unable to continue right now.
//
// Only the second kind is retryable, and it is the kind a user hits in practice
// ("You've hit your org's monthly spend limit"). Those loops are marked retryable so
// the UI can offer Retry, which re-enters at the checkpoint (D55) once the user has
// raised the limit or the provider has recovered.
//
// Classification is deliberately keyed on SUBSTRINGS of the provider's own message
// rather than on error codes: the CLI surfaces these as prose on stderr or as a
// stream-json error subtype, and the exact wording varies by build and plan type.
// Unknown failures are treated as NOT retryable — the honest default, since offering
// Retry on a real defect would just burn tokens re-failing.

// transientMarkers are matched case-insensitively against an agent failure's full
// text (Go error + result subtype + stderr tail). Each is a phrase the provider or the
// CLI uses for a condition that is expected to clear on its own or after the user
// changes a limit — never for a defect in the code being written.
var transientMarkers = []string{
	// Spend / usage / quota ceilings.
	"spend limit",
	"monthly spend",
	"usage limit",
	"usage_limit",
	"rate limit",
	"rate_limit",
	"quota",
	"credit balance",
	"insufficient credit",
	"billing",
	"/usage-credits",
	"upgrade your plan",
	"too many requests",
	"resource_exhausted", // Cursor run status frame, e.g. "[resource_exhausted] Error"

	// Provider-side transients.
	"overloaded",
	"overloaded_error",
	"api_error",
	"internal server error",
	"service unavailable",
	"temporarily unavailable",
	"bad gateway",
	"gateway timeout",
	"server error",
	"529",
	"503",
	"502",
	"504",
	"429",

	// Transport / process transients.
	"connection reset",
	"connection refused",
	"broken pipe",
	"eof",
	"i/o timeout",
	"timeout",
	"timed out",
	"tls handshake",
	"no such host",
	"network is unreachable",
	// Cursor Send treated a dropped stream as success, then Run invents this.
	"ended without a result",
	// Codex ChatGPT session expired; Retry works after signing in again.
	"failed to refresh token",
	"login expired",
}

// classifyAgentError decides whether an agent-turn failure is transient (retryable)
// from the checkpoint) and returns the best human-readable reason it can assemble.
//
// The reason prefers the provider's own words — the message a user can act on — over
// the internal subtype, so the UI shows "You've hit your org's monthly spend limit"
// rather than "coder turn failed: error_during_execution".
func classifyAgentError(err error, res Result) (reason string, transient bool) {
	reason = agentFailureReason(err, res)
	// Connect codes from the cursor bridge beat substring matching: the UI's
	// "cursor bridge: [internal] internal error" is a Cursor-side blip, not a
	// defect in the work, and must offer Retry (D56).
	var cerr *cursor.Error
	if errors.As(err, &cerr) && cerr.Transient() {
		return reason, true
	}
	hay := strings.ToLower(strings.Join([]string{
		errText(err), res.Subtype, res.Text, res.Stderr,
	}, "\n"))
	if knownAgentError(hay) != "" {
		return reason, true
	}
	for _, m := range transientMarkers {
		if strings.Contains(hay, m) {
			return reason, true
		}
	}
	return reason, false
}

// Keep in sync with frontend CODEX_LOGIN_EXPIRED.
const codexLoginExpiredMsg = "Codex login expired: sign in with ChatGPT in Settings → Providers → Codex, then retry."

// agentFailureReason builds the message shown to the user for a failed turn,
// preferring the provider's own text (stderr tail, then the result text) and falling
// back to the Go error / subtype when the CLI said nothing useful.
func agentFailureReason(err error, res Result) string {
	hay := strings.Join([]string{res.Stderr, errText(err), res.Text}, "\n")
	if m := knownAgentError(hay); m != "" {
		return m
	}
	if s := firstLine(res.Stderr); s != "" {
		return tidyAgentError(s)
	}
	// Prefer the runner/transport error over Result.Text. When the Cursor bridge
	// dies mid-turn, Text is often the last assistant prose ("I'll check…")
	// while err carries the real failure ("ended without a result" / spawn
	// ENOENT). Using Text first made the chat show "Agent error: I'll check…".
	if err != nil {
		if s := strings.TrimSpace(term.StripVT(err.Error())); s != "" {
			return tidyAgentError(s)
		}
	}
	if res.IsError {
		if s := firstLine(res.Text); s != "" {
			return tidyAgentError(s)
		}
	}
	if res.Subtype != "" {
		return res.Subtype
	}
	return "the agent turn failed"
}

// knownAgentError maps CLI dumps we recognize onto a complete sentence.
// simplify: one Codex auth phrase; add rows if another dump needs a sentence.
func knownAgentError(s string) string {
	low := strings.ToLower(term.StripVT(s))
	if strings.Contains(low, "failed to refresh token") ||
		(strings.Contains(low, "codex_login") && strings.Contains(low, "401")) {
		return codexLoginExpiredMsg
	}
	return ""
}

func tidyAgentError(s string) string {
	s = strings.TrimSpace(term.StripVT(s))
	if t := stripCLILogPrefix(s); t != s {
		s = t
		if i := strings.Index(s, "{"); i > 0 {
			s = strings.TrimSpace(s[:i])
		}
	}
	return s
}

func stripCLILogPrefix(s string) string {
	i := strings.Index(s, " - ")
	if i <= 0 || !cliLogHead(s[:i]) {
		return s
	}
	return strings.TrimSpace(s[i+3:])
}

func cliLogHead(head string) bool {
	u := " " + strings.ToUpper(head) + " "
	if !strings.Contains(u, " ERROR ") && !strings.Contains(u, " WARN ") &&
		!strings.Contains(u, " WARNING ") && !strings.Contains(u, " INFO ") &&
		!strings.Contains(u, " DEBUG ") && !strings.Contains(u, " TRACE ") {
		return false
	}
	return strings.Contains(head, "::") || (strings.Contains(head, "T") && strings.Contains(head, "Z"))
}

// errText is err.Error() or "" for a nil error.
func errText(err error) string {
	if err == nil {
		return ""
	}
	return err.Error()
}

// firstLine returns the first non-empty line of s, trimmed and length-capped, so a
// multi-page stderr dump does not become the loop's status text.
func firstLine(s string) string {
	s = term.StripVT(s)
	for _, ln := range strings.Split(s, "\n") {
		ln = strings.TrimSpace(ln)
		if ln == "" {
			continue
		}
		const max = 300
		if len(ln) > max {
			return ln[:max] + "…"
		}
		return ln
	}
	return ""
}

// turnError wraps an agent turn failure so the phase that returns it up to Run keeps
// the classification and the provider's message. converge/verify return plain errors
// today; wrapping preserves them without changing any signature.
type turnError struct {
	role      string // human role name, e.g. "Coder"
	reason    string
	transient bool
	err       error
}

func (e *turnError) Error() string {
	if e.reason == "" {
		return fmt.Sprintf("%s turn failed", e.role)
	}
	return fmt.Sprintf("%s turn failed: %s", e.role, e.reason)
}

func (e *turnError) Unwrap() error { return e.err }

// newTurnError classifies a failed agent turn and wraps it for the phase to return.
func newTurnError(role string, err error, res Result) *turnError {
	reason, transient := classifyAgentError(err, res)
	return &turnError{role: role, reason: reason, transient: transient, err: err}
}

// transientFailure reports whether an error returned by a phase is a transient agent
// failure, and the reason to record. It unwraps turnError.
func transientFailure(err error) (reason string, transient bool) {
	var te *turnError
	if errors.As(err, &te) {
		return te.reason, te.transient
	}
	// A failed MCP bind is usually a busy/exhausted local port, which clears —
	// and ensureMCP retries it, so Retry is a real fix rather than a re-run of
	// the same failure.
	if errors.Is(err, errMCPUnavailable) {
		return err.Error(), true
	}
	// A phase that failed without a classified turn error still gets scanned: git and
	// network errors bubble up as plain errors and some of those are transient too.
	if err != nil {
		hay := strings.ToLower(err.Error())
		for _, m := range transientMarkers {
			if strings.Contains(hay, m) {
				return err.Error(), true
			}
		}
	}
	return "", false
}
