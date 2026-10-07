package codex

import (
	"bufio"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"sync"
	"sync/atomic"
)

// Wire messages omit the `"jsonrpc":"2.0"` field (app-server protocol).

type requestMsg struct {
	Method string          `json:"method"`
	ID     json.RawMessage `json:"id,omitempty"`
	Params json.RawMessage `json:"params,omitempty"`
}

type responseMsg struct {
	ID     json.RawMessage `json:"id"`
	Result json.RawMessage `json:"result,omitempty"`
	Error  *rpcError       `json:"error,omitempty"`
}

type rpcError struct {
	Code    int             `json:"code"`
	Message string          `json:"message"`
	Data    json.RawMessage `json:"data,omitempty"`
}

func (e *rpcError) Error() string {
	if e == nil {
		return "codex rpc: unknown error"
	}
	return fmt.Sprintf("codex rpc %d: %s", e.Code, e.Message)
}

// ServerRequestHandler answers server→client requests (approvals, elicitations).
// Return a JSON-encodable result object for the response.
type ServerRequestHandler func(method string, params json.RawMessage) (any, error)

// Conn is a bidirectional JSON-RPC connection over newline-delimited stdio.
type Conn struct {
	w io.Writer
	r *bufio.Reader

	mu       sync.Mutex
	nextID   atomic.Int64
	pending  map[int64]chan responseMsg
	onNotify func(method string, params json.RawMessage)
	onReq    ServerRequestHandler

	closed  atomic.Bool
	readErr atomic.Value // error
}

func newConn(r io.Reader, w io.Writer) *Conn {
	c := &Conn{
		w:       w,
		r:       bufio.NewReaderSize(r, 1024*1024),
		pending: make(map[int64]chan responseMsg),
	}
	go c.readLoop()
	return c
}

// OnNotification registers a handler for server notifications (no id).
func (c *Conn) OnNotification(fn func(method string, params json.RawMessage)) {
	c.mu.Lock()
	c.onNotify = fn
	c.mu.Unlock()
}

// OnServerRequest registers a handler for server→client requests.
func (c *Conn) OnServerRequest(fn ServerRequestHandler) {
	c.mu.Lock()
	c.onReq = fn
	c.mu.Unlock()
}

// Call sends a request and waits for the matching response.
func (c *Conn) Call(ctx context.Context, method string, params any, result any) error {
	if c.closed.Load() {
		return fmt.Errorf("codex rpc: connection closed")
	}
	id := c.nextID.Add(1)
	ch := make(chan responseMsg, 1)
	c.mu.Lock()
	c.pending[id] = ch
	c.mu.Unlock()
	defer func() {
		c.mu.Lock()
		delete(c.pending, id)
		c.mu.Unlock()
	}()

	var rawParams json.RawMessage
	if params != nil {
		b, err := json.Marshal(params)
		if err != nil {
			return fmt.Errorf("codex rpc marshal params: %w", err)
		}
		rawParams = b
	}
	idRaw, _ := json.Marshal(id)
	msg := requestMsg{Method: method, ID: idRaw, Params: rawParams}
	if err := c.write(msg); err != nil {
		return err
	}

	select {
	case <-ctx.Done():
		return ctx.Err()
	case resp := <-ch:
		if resp.Error != nil {
			return resp.Error
		}
		if result != nil && len(resp.Result) > 0 {
			if err := json.Unmarshal(resp.Result, result); err != nil {
				return fmt.Errorf("codex rpc decode %s result: %w", method, err)
			}
		}
		return nil
	}
}

// Notify sends a notification (no id).
func (c *Conn) Notify(method string, params any) error {
	var rawParams json.RawMessage
	if params != nil {
		b, err := json.Marshal(params)
		if err != nil {
			return err
		}
		rawParams = b
	}
	return c.write(requestMsg{Method: method, Params: rawParams})
}

func (c *Conn) write(v any) error {
	b, err := json.Marshal(v)
	if err != nil {
		return err
	}
	c.mu.Lock()
	defer c.mu.Unlock()
	if c.closed.Load() {
		return fmt.Errorf("codex rpc: connection closed")
	}
	_, err = c.w.Write(append(b, '\n'))
	return err
}

func (c *Conn) readLoop() {
	for {
		line, err := c.r.ReadBytes('\n')
		if err != nil {
			c.readErr.Store(err)
			c.failPending(err)
			return
		}
		line = trimNL(line)
		if len(line) == 0 {
			continue
		}
		c.dispatch(line)
	}
}

func (c *Conn) dispatch(line []byte) {
	// Peek for id / method / error / result without full typed decode.
	var peek struct {
		ID     json.RawMessage `json:"id"`
		Method string          `json:"method"`
		Error  *rpcError       `json:"error"`
		Result json.RawMessage `json:"result"`
		Params json.RawMessage `json:"params"`
	}
	if err := json.Unmarshal(line, &peek); err != nil {
		return
	}

	// Response to our request: has id, no method.
	if len(peek.ID) > 0 && peek.Method == "" {
		var id int64
		if err := json.Unmarshal(peek.ID, &id); err != nil {
			return
		}
		c.mu.Lock()
		ch := c.pending[id]
		c.mu.Unlock()
		if ch != nil {
			ch <- responseMsg{ID: peek.ID, Result: peek.Result, Error: peek.Error}
		}
		return
	}

	// Server→client request: method + id.
	if peek.Method != "" && len(peek.ID) > 0 {
		c.mu.Lock()
		handler := c.onReq
		c.mu.Unlock()
		go c.answerServerRequest(peek.ID, peek.Method, peek.Params, handler)
		return
	}

	// Notification: method, no id.
	if peek.Method != "" {
		c.mu.Lock()
		fn := c.onNotify
		c.mu.Unlock()
		if fn != nil {
			fn(peek.Method, peek.Params)
		}
	}
}

func (c *Conn) answerServerRequest(id json.RawMessage, method string, params json.RawMessage, handler ServerRequestHandler) {
	var result any
	var err error
	if handler != nil {
		result, err = handler(method, params)
	} else {
		err = fmt.Errorf("no handler for server request %s", method)
	}
	resp := map[string]any{"id": jsonRaw(id)}
	if err != nil {
		resp["error"] = map[string]any{"code": -32000, "message": err.Error()}
	} else {
		if result == nil {
			result = map[string]any{}
		}
		resp["result"] = result
	}
	_ = c.write(resp)
}

func jsonRaw(r json.RawMessage) any {
	var v any
	if err := json.Unmarshal(r, &v); err != nil {
		return string(r)
	}
	return v
}

func (c *Conn) failPending(err error) {
	c.mu.Lock()
	defer c.mu.Unlock()
	for id, ch := range c.pending {
		ch <- responseMsg{Error: &rpcError{Code: -32001, Message: err.Error()}}
		delete(c.pending, id)
	}
}

func trimNL(b []byte) []byte {
	for len(b) > 0 && (b[len(b)-1] == '\n' || b[len(b)-1] == '\r') {
		b = b[:len(b)-1]
	}
	return b
}
