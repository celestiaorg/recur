package cursor

import (
	"bytes"
	"context"
	"encoding/binary"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net"
	"net/http"
	"strings"
	"syscall"
	"time"
)

// The bridge speaks the Connect protocol with JSON payloads:
//   - unary        → POST <base>/<service>/<method>, Content-Type application/json
//   - server-stream→ POST same path, Content-Type application/connect+json, and
//     both request and response bodies are sequences of 5-byte-prefixed frames.
//
// We hand-roll it rather than pulling in connectrpc.com/connect because we need
// exactly two call shapes over a loopback socket, and the real dependency would
// drag in protobuf codegen for a schema Cursor does not publish.
const (
	connectProtocolVersion = "1"

	// endStreamFlag marks the final frame of a server stream. Its payload is a
	// JSON object carrying trailers and, on failure, an `error`.
	endStreamFlag byte = 0x02
)

// unaryTimeout bounds a single non-streaming RPC. Streams are NOT bounded here —
// a run can legitimately take as long as the task does; cancellation rides on
// the caller's context instead. SteerRun is also unbounded: SDK run.steer stays
// pending after queued until delivery or rejection.
var unaryTimeout = 60 * time.Second

// Client is a Connect/JSON client for one bridge instance. Safe for concurrent
// use.
type Client struct {
	baseURL string
	token   string
	http    *http.Client
	// log, when set, receives diagnostic lines (Connect faults). The Bridge
	// wires this to noteStderr so opaque `[internal]` errors land in
	// cursor-bridge.log even though the Node process stays silent.
	log func(string)
}

func newClient(baseURL, token string) *Client {
	return &Client{
		baseURL: strings.TrimSuffix(baseURL, "/"),
		token:   token,
		// No global client timeout: streams outlive it. Unary calls get a
		// per-call context deadline instead.
		http: &http.Client{},
	}
}

func (c *Client) note(line string) {
	if c == nil || c.log == nil || line == "" {
		return
	}
	c.log(line)
}

// Error is a Connect-level failure returned by the bridge, preserving the
// machine-readable code so callers can distinguish e.g. "unauthenticated"
// (bad/missing API key — a config problem the user must fix) from
// "unavailable" (transient, retryable from the checkpoint).
//
// Opaque codes like `[internal] internal error` usually carry nothing useful in
// Message; Details/Raw are what make them diagnosable. The Node bridge does not
// print these to stderr — Recur must log the Connect body itself.
type Error struct {
	Code    string          `json:"code"`
	Message string          `json:"message"`
	Details json.RawMessage `json:"details,omitempty"`
	// Raw is a truncated copy of the Connect error JSON (or end-stream frame).
	// Not part of the public Error() string — it goes to the bridge log.
	Raw string `json:"-"`
}

func (e *Error) Error() string {
	if e == nil {
		return "cursor bridge: <nil>"
	}
	if e.Code == "" {
		return e.Message
	}
	return fmt.Sprintf("cursor bridge: [%s] %s", e.Code, e.Message)
}

// parseConnectError turns a Connect JSON error body into *Error. Returns nil
// when the body is not a usable Connect error.
func parseConnectError(raw []byte) *Error {
	var e Error
	if json.Unmarshal(raw, &e) != nil || (e.Code == "" && e.Message == "") {
		return nil
	}
	e.Raw = truncate(strings.TrimSpace(string(raw)), 800)
	return &e
}

// Unauthenticated reports whether this is an auth failure, which the engine
// surfaces as a settings problem rather than a retryable agent error.
func (e *Error) Unauthenticated() bool {
	return e.Code == "unauthenticated" || e.Code == "permission_denied"
}

// Unimplemented reports a missing RPC (SteerRun on an unpatched bridge).
func (e *Error) Unimplemented() bool {
	if e == nil {
		return false
	}
	return e.Code == "unimplemented"
}

// Transient reports whether this Connect code is a provider/bridge blip that
// should be retried from the checkpoint (D56) rather than failing the loop
// terminally. `internal` is what surfaces as the opaque
// "cursor bridge: [internal] internal error" in the UI — almost always a
// Cursor-side fault, not a defect in the work.
func (e *Error) Transient() bool {
	if e == nil {
		return false
	}
	switch e.Code {
	case "internal", "unavailable", "resource_exhausted", "deadline_exceeded", "aborted":
		return true
	}
	return false
}

// ActiveRunConflict reports the Send failure when the agent store still has a
// non-terminal activeRunId — typically a dropped Send stream on the current
// bridge process, or a SQLite leftover after Recur/the bridge restarted.
// Recovery drains a live same-process run via ObserveRun then Sends this
// turn's prompt, or CancelRun + Send for leftovers from a previous process.
func (e *Error) ActiveRunConflict() bool {
	if e == nil {
		return false
	}
	return strings.Contains(strings.ToLower(e.Message), "already has active run")
}

// ActiveRunConflict reports whether err (possibly wrapped) is an active-run
// conflict from the bridge.
func ActiveRunConflict(err error) bool {
	var cerr *Error
	if errors.As(err, &cerr) {
		return cerr.ActiveRunConflict()
	}
	if err == nil {
		return false
	}
	return strings.Contains(strings.ToLower(err.Error()), "already has active run")
}

// BridgeUnreachable reports whether err means the local bridge process is gone
// or its loopback port is dead — the failure mode when the Node child crashes
// and the engine still holds a stale client pointing at the old port. Matched
// on the transport errors Go surfaces for that case (ECONNREFUSED/RESET, and
// the "connection refused" prose from net/http), not on Connect application
// codes (those mean the bridge is up and answering).
func BridgeUnreachable(err error) bool {
	if err == nil {
		return false
	}
	if errors.Is(err, syscall.ECONNREFUSED) || errors.Is(err, syscall.ECONNRESET) {
		return true
	}
	var op *net.OpError
	if errors.As(err, &op) {
		if errors.Is(op.Err, syscall.ECONNREFUSED) || errors.Is(op.Err, syscall.ECONNRESET) {
			return true
		}
	}
	msg := strings.ToLower(err.Error())
	for _, m := range []string{
		"connection refused",
		"connection reset",
		"broken pipe",
		"connect: connection refused",
	} {
		if strings.Contains(msg, m) {
			return true
		}
	}
	return false
}

func (c *Client) url(service, method string) string {
	return c.baseURL + "/" + service + "/" + method
}

func (c *Client) newRequest(ctx context.Context, service, method string, body []byte, streaming bool) (*http.Request, error) {
	req, err := http.NewRequestWithContext(ctx, http.MethodPost, c.url(service, method), bytes.NewReader(body))
	if err != nil {
		return nil, err
	}
	ct := "application/json"
	if streaming {
		ct = "application/connect+json"
	}
	req.Header.Set("Content-Type", ct)
	req.Header.Set("Accept", ct)
	req.Header.Set("Connect-Protocol-Version", connectProtocolVersion)
	if c.token != "" {
		req.Header.Set("Authorization", "Bearer "+c.token)
	}
	return req, nil
}

// unary performs a single request/response RPC, decoding into out (which may be
// nil to discard the body). Bounded by unaryTimeout.
func (c *Client) unary(ctx context.Context, service, method string, in, out any) error {
	ctx, cancel := context.WithTimeout(ctx, unaryTimeout)
	defer cancel()
	return c.unaryCtx(ctx, service, method, in, out)
}

// unaryCtx is unary without an extra deadline — the caller's context is the
// only cancellation (SteerRun waits for terminal acknowledgement).
func (c *Client) unaryCtx(ctx context.Context, service, method string, in, out any) error {
	body, err := json.Marshal(in)
	if err != nil {
		return fmt.Errorf("cursor bridge: marshal %s/%s: %w", service, method, err)
	}

	req, err := c.newRequest(ctx, service, method, body, false)
	if err != nil {
		return err
	}
	resp, err := c.http.Do(req)
	if err != nil {
		return fmt.Errorf("cursor bridge: %s/%s: %w", service, method, err)
	}
	defer func() { _ = resp.Body.Close() }()

	raw, err := io.ReadAll(io.LimitReader(resp.Body, 64<<20))
	if err != nil {
		return fmt.Errorf("cursor bridge: read %s/%s: %w", service, method, err)
	}

	// Connect reports application errors as a non-2xx status with a JSON
	// {code, message[, details]} body. The bridge does not echo these on
	// stderr, so we log the body ourselves.
	if resp.StatusCode < 200 || resp.StatusCode >= 300 {
		if e := parseConnectError(raw); e != nil {
			c.note(fmt.Sprintf("recur: connect %s/%s → http %d %s", service, method, resp.StatusCode, e.Raw))
			return e
		}
		return fmt.Errorf("cursor bridge: %s/%s: http %d: %s", service, method, resp.StatusCode, truncate(string(raw), 400))
	}
	if out == nil || len(raw) == 0 {
		return nil
	}
	if err := json.Unmarshal(raw, out); err != nil {
		return fmt.Errorf("cursor bridge: decode %s/%s: %w", service, method, err)
	}
	return nil
}

// stream performs a server-streaming RPC, invoking onMsg for each data frame.
// It returns when the end-stream frame arrives, the server closes, onMsg
// returns an error, or ctx is cancelled.
//
// Framing (both directions): [1 byte flags][4 bytes big-endian length][payload].
// A frame whose flags have endStreamFlag set terminates the stream; its payload
// carries trailers plus an optional `error`.
func (c *Client) stream(ctx context.Context, service, method string, in any, onMsg func(json.RawMessage) error) error {
	payload, err := json.Marshal(in)
	if err != nil {
		return fmt.Errorf("cursor bridge: marshal %s/%s: %w", service, method, err)
	}
	body := encodeFrame(0, payload)

	req, err := c.newRequest(ctx, service, method, body, true)
	if err != nil {
		return err
	}
	resp, err := c.http.Do(req)
	if err != nil {
		return fmt.Errorf("cursor bridge: %s/%s: %w", service, method, err)
	}
	defer func() { _ = resp.Body.Close() }()

	if resp.StatusCode < 200 || resp.StatusCode >= 300 {
		raw, _ := io.ReadAll(io.LimitReader(resp.Body, 1<<20))
		if e := parseConnectError(raw); e != nil {
			c.note(fmt.Sprintf("recur: connect %s/%s → http %d %s", service, method, resp.StatusCode, e.Raw))
			return e
		}
		return fmt.Errorf("cursor bridge: %s/%s: http %d: %s", service, method, resp.StatusCode, truncate(string(raw), 400))
	}

	hdr := make([]byte, 5)
	for {
		if err := ctx.Err(); err != nil {
			return err
		}
		if _, err := io.ReadFull(resp.Body, hdr); err != nil {
			if err := ctx.Err(); err != nil {
				return err
			}
			if err == io.EOF || err == io.ErrUnexpectedEOF {
				// Server closed without an explicit end frame. Treat as a
				// clean end of stream: the caller decides whether the absence
				// of a result is an error.
				return nil
			}
			return fmt.Errorf("cursor bridge: %s/%s: read frame header: %w", service, method, err)
		}
		flags := hdr[0]
		size := binary.BigEndian.Uint32(hdr[1:5])
		if size > 64<<20 {
			return fmt.Errorf("cursor bridge: %s/%s: frame too large (%d bytes)", service, method, size)
		}
		frame := make([]byte, size)
		if _, err := io.ReadFull(resp.Body, frame); err != nil {
			if err := ctx.Err(); err != nil {
				return err
			}
			return fmt.Errorf("cursor bridge: %s/%s: read frame body: %w", service, method, err)
		}

		if flags&endStreamFlag != 0 {
			err := decodeEndStream(frame)
			if e, ok := err.(*Error); ok && e != nil {
				c.note(fmt.Sprintf("recur: connect %s/%s end-stream %s", service, method, e.Raw))
			}
			return err
		}
		if err := onMsg(frame); err != nil {
			return err
		}
	}
}

// decodeEndStream turns the terminal frame into an error (or nil on success).
// Connect puts the failure under `error`; that body is what surfaces as the
// opaque UI string "[internal] internal error" and must be preserved in Raw
// for the bridge log — the Node process never prints it.
func decodeEndStream(frame []byte) error {
	if len(bytes.TrimSpace(frame)) == 0 {
		return nil
	}
	var end struct {
		Error json.RawMessage `json:"error"`
	}
	if err := json.Unmarshal(frame, &end); err != nil {
		// A malformed trailer is not worth failing an otherwise-complete run.
		return nil
	}
	if len(bytes.TrimSpace(end.Error)) == 0 {
		return nil
	}
	if e := parseConnectError(end.Error); e != nil {
		return e
	}
	return nil
}

func encodeFrame(flags byte, payload []byte) []byte {
	buf := make([]byte, 5+len(payload))
	buf[0] = flags
	binary.BigEndian.PutUint32(buf[1:5], uint32(len(payload)))
	copy(buf[5:], payload)
	return buf
}

func truncate(s string, n int) string {
	if len(s) <= n {
		return s
	}
	return s[:n] + "…"
}
