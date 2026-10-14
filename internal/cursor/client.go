package cursor

import (
	"context"
	"encoding/json"
	"strconv"
	"strings"
	"time"
)

// sdk.v1 service names, as served by the bridge. Verified against a live bridge
// (GetVersion advertises protocolVersion "sdk.v1" plus a capabilities list).
const (
	svcAgent   = "sdk.v1.SdkAgentService"
	svcCursor  = "sdk.v1.SdkCursorService"
	svcControl = "sdk.v1.SdkBridgeControlService"
)

// --- control -----------------------------------------------------------------

// VersionInfo is the GetVersion response. Capabilities is the bridge's
// self-declared feature list (e.g. "agent.create", "agent.send", "run.observe");
// Recur checks it at startup so a bridge too old for what we need fails with a
// clear message instead of an opaque RPC error later.
type VersionInfo struct {
	BridgeVersion   string   `json:"bridgeVersion"`
	ProtocolVersion string   `json:"protocolVersion"`
	Capabilities    []string `json:"capabilities"`
}

// Has reports whether the bridge advertises a capability.
func (v VersionInfo) Has(capability string) bool {
	for _, c := range v.Capabilities {
		if c == capability {
			return true
		}
	}
	return false
}

func (c *Client) GetVersion(ctx context.Context) (VersionInfo, error) {
	var out VersionInfo
	err := c.unary(ctx, svcControl, "GetVersion", map[string]any{}, &out)
	return out, err
}

func (c *Client) Ping(ctx context.Context) error {
	return c.unary(ctx, svcControl, "Ping", map[string]any{}, nil)
}

func (c *Client) Shutdown(ctx context.Context) error {
	return c.unary(ctx, svcControl, "Shutdown", map[string]any{"graceSeconds": 0}, nil)
}

// --- catalog -----------------------------------------------------------------

// Model is one entry from ListModels — the same shape as Cursor.models.list()
// (SDK ModelListItem). Parameters/variants are the structured flag dimensions;
// Recur maps them into the Settings flags picker rather than parsing long ids.
type Model struct {
	ID          string                     `json:"id"`
	DisplayName string                     `json:"displayName"`
	Description string                     `json:"description"`
	Aliases     []string                   `json:"aliases"`
	Parameters  []ModelParameterDefinition `json:"parameters,omitempty"`
	Variants    []ModelVariant             `json:"variants,omitempty"`
}

// ModelParameterDefinition is one configurable dimension on a model (e.g. fast,
// effort, context). Values are the allowed choices for that dimension.
type ModelParameterDefinition struct {
	ID          string                 `json:"id"`
	DisplayName string                 `json:"displayName,omitempty"`
	Values      []ModelParameterChoice `json:"values"`
}

// ModelParameterChoice is one allowed value for a ModelParameterDefinition.
type ModelParameterChoice struct {
	Value       string `json:"value"`
	DisplayName string `json:"displayName,omitempty"`
}

// ModelVariant is a named preset of parameter values (e.g. "High Fast").
type ModelVariant struct {
	Params      []ModelParameterValue `json:"params"`
	DisplayName string                `json:"displayName"`
	Description string                `json:"description,omitempty"`
	IsDefault   bool                  `json:"isDefault,omitempty"`
}

// ModelParameterValue is one {id, value} pair on a ModelSelection or variant.
type ModelParameterValue struct {
	ID    string `json:"id"`
	Value string `json:"value"`
}

// ListModels returns the models the API key may use. Recur uses it to populate
// the model pickers when the Cursor provider is selected, and as the cheapest
// possible credential check in Settings.
func (c *Client) ListModels(ctx context.Context, apiKey string) ([]Model, error) {
	req := map[string]any{"options": map[string]any{"apiKey": apiKey}}
	var out struct {
		Items []Model `json:"items"`
	}
	err := c.unary(ctx, svcCursor, "ListModels", req, &out)
	return out.Items, err
}

// User is the Me / SDKUser response — used to show "signed in as …" in Settings.
// Fields match sdk.v1.SdkUser (optional name/email may be empty for some keys).
type User struct {
	APIKeyName    string `json:"apiKeyName"`
	UserEmail     string `json:"userEmail"`
	UserFirstName string `json:"userFirstName"`
	UserLastName  string `json:"userLastName"`
	CreatedAt     string `json:"createdAt"`
}

// DisplayName joins first + last name when present.
func (u User) DisplayName() string {
	first := strings.TrimSpace(u.UserFirstName)
	last := strings.TrimSpace(u.UserLastName)
	switch {
	case first != "" && last != "":
		return first + " " + last
	case first != "":
		return first
	default:
		return last
	}
}

// Summary is a short identity for toasts / one-line status (email, else key name).
func (u User) Summary() string {
	if e := strings.TrimSpace(u.UserEmail); e != "" {
		return e
	}
	return strings.TrimSpace(u.APIKeyName)
}

func (c *Client) Me(ctx context.Context, apiKey string) (User, error) {
	req := map[string]any{"options": map[string]any{"apiKey": apiKey}}
	var out struct {
		User User `json:"user"`
	}
	err := c.unary(ctx, svcCursor, "Me", req, &out)
	return out.User, err
}

// --- agents ------------------------------------------------------------------

// MCPServer is one inline `mcpServers` entry. The bridge's oneof is encoded as a
// wrapper object, so an HTTP server marshals as:
//
//	{"http":{"type":"HTTP_MCP_TRANSPORT_TYPE_HTTP","url":"http://127.0.0.1:…"}}
//
// This is the whole reason Recur uses the bridge: the CLI can only read MCP
// config from `<projectRoot>/.cursor/mcp.json` or `~/.cursor/mcp.json` (both
// paths are hardcoded to homedir()/projectRoot in the CLI bundle — no env var
// or flag relocates them), which would mean writing into the loop's worktree.
type MCPServer struct {
	HTTP  *HTTPMCPServer  `json:"http,omitempty"`
	Stdio *StdioMCPServer `json:"stdio,omitempty"`
}

// HTTPMCPServer is the http/sse transport variant.
type HTTPMCPServer struct {
	Type    string            `json:"type"` // HTTP_MCP_TRANSPORT_TYPE_HTTP
	URL     string            `json:"url"`
	Headers map[string]string `json:"headers,omitempty"`
}

// StdioMCPServer is the subprocess transport variant, used for the Playwright
// browser MCP the QA role drives.
type StdioMCPServer struct {
	Command string            `json:"command"`
	Args    []string          `json:"args,omitempty"`
	Env     map[string]string `json:"env,omitempty"`
	Cwd     string            `json:"cwd,omitempty"`
}

// NewHTTPMCPServer builds an inline streamable-HTTP MCP entry. Recur's MCP
// server (internal/mcp) puts a per-agent token in the URL path, so no headers
// are needed for attribution.
func NewHTTPMCPServer(url string) MCPServer {
	return MCPServer{HTTP: &HTTPMCPServer{Type: "HTTP_MCP_TRANSPORT_TYPE_HTTP", URL: url}}
}

// NewStdioMCPServer builds an inline stdio MCP entry.
func NewStdioMCPServer(command string, args ...string) MCPServer {
	return MCPServer{Stdio: &StdioMCPServer{Command: command, Args: args}}
}

// ModelSelection is `{id, params}`. Params carry per-model flags (fast, effort,
// …) discovered via ListModels; omitting them makes the run use each parameter's
// first allowed value.
type ModelSelection struct {
	ID     string                `json:"id"`
	Params []ModelParameterValue `json:"params,omitempty"`
}

// LocalOptions are the local-runtime options on AgentOptions.
type LocalOptions struct {
	// Cwd is the loop's worktree — what scopes a run's file access and what the
	// bridge uses as the project root. It is `repeated string` on the wire (the
	// SDK supports multi-root workspaces), so a single directory still marshals
	// as a one-element array; sending a bare string is rejected with
	// `[invalid_argument] cannot decode field sdk.v1.LocalAgentOptions.cwd`.
	Cwd []string `json:"cwd,omitempty"`

	// SettingSources selects which ambient Cursor config layers load, as
	// SETTING_SOURCE_* enum names (see the SettingSource* constants).
	//
	// CAUTION: an empty slice does NOT mean "load nothing". The bridge maps
	// `settingSources.length === 0` to `undefined`, i.e. SDK defaults — proto3
	// cannot distinguish an empty repeated field from an absent one. To narrow
	// what loads you must name the layers you DO want; see SettingSourcesMinimal.
	SettingSources []string `json:"settingSources,omitempty"`

	// Sandbox must be sent explicitly as enabled:false. Omitting the field
	// lets ~/.cursor/sandbox.json (per-user policy) turn sandboxing ON, which
	// has crashed the bridge mid Shell tool (spawn /bin/zsh ENOENT inside the
	// sandbox) and left the agent with a stuck active run. Recur already
	// isolates each loop in a git worktree; the sandbox also blocks the
	// build/test commands agents need.
	Sandbox *SandboxOptions `json:"sandboxOptions,omitempty"`
}

// Ambient Cursor settings layers, as the bridge's proto enum names.
const (
	SettingSourceProject = "SETTING_SOURCE_PROJECT" // <worktree>/.cursor/**, AGENTS.md
	SettingSourceUser    = "SETTING_SOURCE_USER"    // ~/.cursor/**
	SettingSourceTeam    = "SETTING_SOURCE_TEAM"
	SettingSourceMDM     = "SETTING_SOURCE_MDM"
	SettingSourcePlugins = "SETTING_SOURCE_PLUGINS"
	SettingSourceAll     = "SETTING_SOURCE_ALL"
)

// SettingSourcesMinimal is the narrowest set of ambient layers we can ask for.
// Because an empty slice reads as "defaults", naming only the MDM layer (which
// is empty on any machine without managed device config) is how Recur suppresses
// the user's and project's own Cursor config when isolation is wanted.
//
// Recur does NOT use this by default: a repo's .cursor/rules and AGENTS.md are
// legitimate project context, the same way ClaudeRunner lets CLAUDE.md load.
// It exists for the Settings toggle and for tests that need a clean room.
var SettingSourcesMinimal = []string{SettingSourceMDM}

// SandboxOptions mirrors sdk.v1.SandboxOptions.
//
// Enabled is intentionally NOT omitempty: the wire value must be the JSON
// boolean false. If omitted, the bridge treats sandboxOptions as unset and may
// apply a per-user ~/.cursor/sandbox.json policy.
type SandboxOptions struct {
	Enabled bool `json:"enabled"`
}

// SandboxDisabled is the LocalOptions.Sandbox value Recur always sends.
var SandboxDisabled = &SandboxOptions{Enabled: false}

// AgentOptions is the sdk.v1.AgentOptions payload for CreateAgent.
type AgentOptions struct {
	Model      *ModelSelection      `json:"model,omitempty"`
	APIKey     string               `json:"apiKey,omitempty"`
	Name       string               `json:"name,omitempty"`
	Local      *LocalOptions        `json:"local,omitempty"`
	MCPServers map[string]MCPServer `json:"mcpServers,omitempty"`

	// Mode is "agent" (default) or "plan" (read-only). Recur maps read-only
	// roles onto plan mode, which is the closest thing Cursor offers to Claude
	// Code's per-role --allowedTools scoping.
	Mode string `json:"mode,omitempty"`
}

// CreateAgent creates a local agent and returns its id.
func (c *Client) CreateAgent(ctx context.Context, opts AgentOptions) (string, error) {
	req := map[string]any{"options": opts}
	var out struct {
		AgentID string `json:"agentId"`
	}
	if err := c.unary(ctx, svcAgent, "CreateAgent", req, &out); err != nil {
		return "", err
	}
	return out.AgentID, nil
}

// ResumeAgent reattaches to an agent created earlier, preserving its
// conversation. This is Recur's multi-turn continuity for the Maestro chat: the
// engine persists the agentId and passes it back on the next turn, the same way
// it persists a Claude Code session id.
func (c *Client) ResumeAgent(ctx context.Context, agentID string, opts AgentOptions) (string, error) {
	req := map[string]any{"agentId": agentID, "options": opts}
	var out struct {
		AgentID string `json:"agentId"`
	}
	if err := c.unary(ctx, svcAgent, "ResumeAgent", req, &out); err != nil {
		return "", err
	}
	if out.AgentID == "" {
		return agentID, nil
	}
	return out.AgentID, nil
}

// CloseAgent releases bridge-side resources for an agent. Best-effort: a failure
// here leaks a little bridge memory but must never fail a run.
func (c *Client) CloseAgent(ctx context.Context, agentID string) error {
	return c.unary(ctx, svcAgent, "CloseAgent", map[string]any{"agentId": agentID}, nil)
}

// CancelRun stops an in-flight run.
func (c *Client) CancelRun(ctx context.Context, runID string) error {
	return c.unary(ctx, svcAgent, "CancelRun", map[string]any{"runId": runID}, nil)
}

const (
	SteerCompleteDelivered = "complete_delivered"
	SteerRevertToFollowup  = "revert_to_followup"
)

// SteerRun injects text into a live run (SDK run.steer). complete_delivered
// means the turn has the message. Anything else — including a missing RPC —
// means the caller should Send after the run instead.
//
// The RPC stays pending after the provider queues it, so this call follows
// ctx through terminal acknowledgement rather than unaryTimeout.
func (c *Client) SteerRun(ctx context.Context, runID, text string) (string, error) {
	var out struct {
		Outcome string `json:"outcome"`
	}
	err := c.unaryCtx(ctx, svcAgent, "SteerRun", map[string]any{"runId": runID, "text": text}, &out)
	if err != nil {
		return "", err
	}
	return out.Outcome, nil
}

// RunSnapshot is one row from ListRuns (sdk.v1.RunSnapshot).
type RunSnapshot struct {
	RunID     string    `json:"runId"`
	AgentID   string    `json:"agentId"`
	Status    string    `json:"status"`
	CreatedAt protoTime `json:"createdAt"`
}

// protoTime decodes google.protobuf.Timestamp JSON: proto3 RFC3339 or
// {seconds,nanos}. Invalid values stay zero rather than failing ListRuns.
type protoTime time.Time

func (p protoTime) Time() time.Time { return time.Time(p) }

func (p protoTime) MarshalJSON() ([]byte, error) {
	t := time.Time(p)
	if t.IsZero() {
		return []byte("null"), nil
	}
	return t.UTC().MarshalJSON()
}

func (p *protoTime) UnmarshalJSON(b []byte) error {
	if len(b) == 0 || string(b) == "null" || string(b) == `""` {
		return nil
	}
	if b[0] == '"' {
		var s string
		if json.Unmarshal(b, &s) != nil || s == "" {
			return nil
		}
		t, err := time.Parse(time.RFC3339Nano, s)
		if err != nil {
			t, err = time.Parse(time.RFC3339, s)
		}
		if err != nil {
			return nil
		}
		*p = protoTime(t)
		return nil
	}
	var ts struct {
		Seconds json.RawMessage `json:"seconds"`
		Nanos   int32           `json:"nanos"`
	}
	if json.Unmarshal(b, &ts) != nil {
		return nil
	}
	sec, ok := parseJSONInt64(ts.Seconds)
	if !ok {
		return nil
	}
	*p = protoTime(time.Unix(sec, int64(ts.Nanos)).UTC())
	return nil
}

func parseJSONInt64(raw json.RawMessage) (int64, bool) {
	if len(raw) == 0 || string(raw) == "null" {
		return 0, false
	}
	if raw[0] == '"' {
		var s string
		if json.Unmarshal(raw, &s) != nil || s == "" {
			return 0, false
		}
		n, err := strconv.ParseInt(s, 10, 64)
		return n, err == nil
	}
	var n int64
	if json.Unmarshal(raw, &n) != nil {
		return 0, false
	}
	return n, true
}

// ListRuns returns runs for an agent (newest first from the bridge store).
func (c *Client) ListRuns(ctx context.Context, agentID string) ([]RunSnapshot, error) {
	var out struct {
		Items []RunSnapshot `json:"items"`
	}
	if err := c.unary(ctx, svcAgent, "ListRuns", map[string]any{"agentId": agentID}, &out); err != nil {
		return nil, err
	}
	return out.Items, nil
}

// FirstNonTerminalRun returns the newest creating/running run for agentID.
// ListRuns is newest-first; a zero RunID means none.
func (c *Client) FirstNonTerminalRun(ctx context.Context, agentID string) (RunSnapshot, error) {
	runs, err := c.ListRuns(ctx, agentID)
	if err != nil {
		return RunSnapshot{}, err
	}
	for _, run := range runs {
		if run.RunID != "" && !RunStatusTerminal(run.Status) {
			return run, nil
		}
	}
	return RunSnapshot{}, nil
}

// RunCreatedOnThisBridge reports whether run has a timestamp placing it after
// this bridge process started. Missing or unparseable CreatedAt fails closed
// (false): ObserveRun on a leftover SQLite row would replay then hang, so
// unknown timestamps take the cancel+Send path.
func RunCreatedOnThisBridge(run RunSnapshot, bridgeStarted time.Time) bool {
	if run.RunID == "" {
		return false
	}
	created, started := run.CreatedAt.Time(), bridgeStarted
	if created.IsZero() || started.IsZero() {
		return false
	}
	return !created.Before(started.Add(-time.Second))
}

// CancelNonTerminalRuns cancels any creating/running runs for agentID.
// Fallback when ObserveRun cannot re-attach after "already has active run"
// (typically a SQLite leftover whose executor is gone).
func (c *Client) CancelNonTerminalRuns(ctx context.Context, agentID string) (int, error) {
	runs, err := c.ListRuns(ctx, agentID)
	if err != nil {
		return 0, err
	}
	n := 0
	var first error
	for _, run := range runs {
		if run.RunID == "" || RunStatusTerminal(run.Status) {
			continue
		}
		if err := c.CancelRun(ctx, run.RunID); err != nil {
			if first == nil {
				first = err
			}
			continue
		}
		n++
	}
	return n, first
}

// Image is one image attached to a user message (base64 data + MIME type).
// Matches Cursor's SDKImage `{ data, mimeType }` form at the TypeScript layer.
type Image struct {
	Data     string `json:"data"`
	MimeType string `json:"mimeType"`
}

// Send starts a run and streams its events. onEvent is called for each frame in
// order; returning an error from it aborts the stream. images (optional) are
// attached alongside text as SDKUserMessage.images.
//
// Wire shape note: the bridge speaks protobuf JSON for sdk.v1.UserMessage.
// SdkImage is a oneof, so data images must be nested as
// `{ "data": { "data": "<b64>", "mimeType": "image/png" } }` — NOT the flat
// TypeScript SDK shape `{ "data": "<b64>", "mimeType": "…" }`. Sending the flat
// form drops the image (or fails the RPC) and the chat turn looks like a no-op.
//
// Recur maps D49 Inject onto SteerRun (SDK run.steer) when the live turn
// appends the text (complete_delivered). Otherwise CancelRun plus a follow-up
// Send on the same agent; see CursorRunner.Run.
func (c *Client) Send(ctx context.Context, agentID, text string, images []Image, onEvent func(RunStreamEvent) error) error {
	msg := map[string]any{"text": text}
	if len(images) > 0 {
		imgs := make([]map[string]any, len(images))
		for i, img := range images {
			imgs[i] = map[string]any{
				"data": map[string]any{
					"data":     img.Data,
					"mimeType": img.MimeType,
				},
			}
		}
		msg["images"] = imgs
	}
	req := map[string]any{
		"agentId": agentID,
		"message": msg,
		// enableDeltas attaches onDelta. InteractionUpdate includes summary,
		// summary-started, summary-completed, and turn-ended; Recur's minify
		// patch is required because stock 1.0.37 still skips the summary types.
		"options": map[string]any{"enableDeltas": true},
	}
	return c.stream(ctx, svcAgent, "Send", req, c.decodeRunStream(onEvent))
}

// ObserveRun subscribes to an existing run and streams the same frames as Send
// (sdk.v1.SdkAgentService/ObserveRun). afterOffset is exclusive; empty starts
// from the beginning of durable events. Recur's active-run recovery waits until
// the leftover run is terminal or goes idle (no Offset frames), without folding
// those frames into the current turn; the prompt is still Sent afterwards.
func (c *Client) ObserveRun(ctx context.Context, runID, afterOffset string, onEvent func(RunStreamEvent) error) error {
	req := map[string]any{"runId": runID}
	if afterOffset != "" {
		req["afterOffset"] = afterOffset
	}
	return c.stream(ctx, svcAgent, "ObserveRun", req, c.decodeRunStream(onEvent))
}

func (c *Client) decodeRunStream(onEvent func(RunStreamEvent) error) func(json.RawMessage) error {
	return func(raw json.RawMessage) error {
		var ev RunStreamEvent
		if err := json.Unmarshal(raw, &ev); err != nil {
			// One unparseable frame must not kill a run; skip it.
			return nil
		}
		// The Node bridge never logs a failed run, and its proto RunResult drops
		// the SDK's RunError — this status frame is the only trace of WHY.
		if m := ev.SDKMessage; m != nil && m.Type == MsgStatus && m.Status == StatusError && m.Message != "" {
			c.note("recur: run " + m.RunID + " status ERROR: " + m.Message)
		}
		return onEvent(ev)
	}
}
