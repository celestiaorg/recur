package mcp

import "encoding/json"

// protocolVersion is the MCP protocol version Recur's server speaks. We accept
// whatever recent version the client requests (claude negotiates a date-stamped
// version) and otherwise advertise this one — the wire shapes Recur uses
// (initialize / tools/list / tools/call) are stable across these revisions.
const protocolVersion = "2025-06-18"

// serverName / serverVersion identify the Recur MCP in the init handshake.
const (
	serverName    = "recur"
	serverVersion = "1.0.0"
)

// jsonrpcVersion is the only JSON-RPC version we accept.
const jsonrpcVersion = "2.0"

// JSON-RPC 2.0 error codes (subset we use).
const (
	codeParse          = -32700
	codeInvalidRequest = -32600
	codeMethodNotFound = -32601
	codeInvalidParams  = -32602
	codeInternal       = -32603
)

// rpcRequest is one incoming JSON-RPC request (or notification, when ID is nil).
// ID is kept as RawMessage so we can echo it back verbatim (it may be a string,
// number, or null per the spec).
type rpcRequest struct {
	JSONRPC string          `json:"jsonrpc"`
	ID      json.RawMessage `json:"id,omitempty"`
	Method  string          `json:"method"`
	Params  json.RawMessage `json:"params,omitempty"`
}

// isNotification reports whether the request is a notification (no id → no
// response is sent), per JSON-RPC 2.0.
func (r rpcRequest) isNotification() bool {
	return len(r.ID) == 0 || string(r.ID) == "null"
}

// rpcResponse is one JSON-RPC response. Exactly one of Result/Error is set.
type rpcResponse struct {
	JSONRPC string          `json:"jsonrpc"`
	ID      json.RawMessage `json:"id"`
	Result  any             `json:"result,omitempty"`
	Error   *rpcError       `json:"error,omitempty"`
}

// rpcError is a JSON-RPC error object.
type rpcError struct {
	Code    int    `json:"code"`
	Message string `json:"message"`
	Data    any    `json:"data,omitempty"`
}

func newResult(id json.RawMessage, result any) rpcResponse {
	return rpcResponse{JSONRPC: jsonrpcVersion, ID: id, Result: result}
}

func newError(id json.RawMessage, code int, msg string) rpcResponse {
	if len(id) == 0 {
		id = json.RawMessage("null")
	}
	return rpcResponse{JSONRPC: jsonrpcVersion, ID: id, Error: &rpcError{Code: code, Message: msg}}
}

// ---- MCP method payloads (only the fields Recur reads/sends) ----

// initializeResult is the server's response to `initialize`.
type initializeResult struct {
	ProtocolVersion string             `json:"protocolVersion"`
	Capabilities    serverCapabilities `json:"capabilities"`
	ServerInfo      implementation     `json:"serverInfo"`
	Instructions    string             `json:"instructions,omitempty"`
}

type serverCapabilities struct {
	Tools *toolsCapability `json:"tools,omitempty"`
}

type toolsCapability struct {
	ListChanged bool `json:"listChanged"`
}

type implementation struct {
	Name    string `json:"name"`
	Version string `json:"version"`
}

// initializeParams is the client's `initialize` request params; we read the
// requested protocol version to echo a compatible one.
type initializeParams struct {
	ProtocolVersion string `json:"protocolVersion"`
}

// tool is one entry in a tools/list result (MCP Tool shape).
type tool struct {
	Name        string          `json:"name"`
	Description string          `json:"description"`
	InputSchema json.RawMessage `json:"inputSchema"`
}

type listToolsResult struct {
	Tools []tool `json:"tools"`
}

// callToolParams is the `tools/call` request params.
type callToolParams struct {
	Name      string          `json:"name"`
	Arguments json.RawMessage `json:"arguments"`
}

// textContent is one text block of a CallToolResult.
type textContent struct {
	Type string `json:"type"`
	Text string `json:"text"`
}

// callToolResult is the `tools/call` response. A tool-level failure is reported
// via IsError + a text content block (NOT a JSON-RPC error), so the model can see
// it and self-correct (MCP convention).
type callToolResult struct {
	Content []textContent `json:"content"`
	IsError bool          `json:"isError,omitempty"`
}

func toolText(s string) callToolResult {
	return callToolResult{Content: []textContent{{Type: "text", Text: s}}}
}

func toolError(s string) callToolResult {
	return callToolResult{Content: []textContent{{Type: "text", Text: s}}, IsError: true}
}
