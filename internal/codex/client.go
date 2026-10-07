package codex

import (
	"context"
	"encoding/json"
	"fmt"
)

// Client is a thin typed façade over Conn for the methods Recur uses.
type Client struct {
	conn *Conn
}

// NewClient wraps a live Conn.
func NewClient(conn *Conn) *Client { return &Client{conn: conn} }

// Conn exposes the underlying connection (for notification registration).
func (c *Client) Conn() *Conn { return c.conn }

func (c *Client) AccountRead(ctx context.Context) (AccountReadResult, error) {
	var out AccountReadResult
	err := c.conn.Call(ctx, "account/read", map[string]any{"refreshToken": false}, &out)
	return out, err
}

func (c *Client) ModelList(ctx context.Context, limit int) ([]Model, error) {
	if limit <= 0 {
		limit = 50
	}
	var out struct {
		Data []Model `json:"data"`
	}
	err := c.conn.Call(ctx, "model/list", map[string]any{
		"limit":         limit,
		"includeHidden": false,
	}, &out)
	return out.Data, err
}

func (c *Client) ThreadStart(ctx context.Context, p ThreadStartParams) (Thread, error) {
	if p.ServiceName == "" {
		p.ServiceName = "recur"
	}
	var out struct {
		Thread Thread `json:"thread"`
	}
	err := c.conn.Call(ctx, "thread/start", p, &out)
	return out.Thread, err
}

func (c *Client) ThreadResume(ctx context.Context, threadID string, p ThreadStartParams) (Thread, error) {
	params := map[string]any{"threadId": threadID}
	if p.Model != "" {
		params["model"] = p.Model
	}
	if p.CWD != "" {
		params["cwd"] = p.CWD
	}
	if p.ApprovalPolicy != "" {
		params["approvalPolicy"] = p.ApprovalPolicy
	}
	if p.Sandbox != "" {
		params["sandbox"] = p.Sandbox
	}
	if p.DeveloperInstructions != "" {
		params["developerInstructions"] = p.DeveloperInstructions
	}
	var out struct {
		Thread Thread `json:"thread"`
	}
	err := c.conn.Call(ctx, "thread/resume", params, &out)
	return out.Thread, err
}

func (c *Client) TurnStart(ctx context.Context, p TurnStartParams) (Turn, error) {
	var out struct {
		Turn Turn `json:"turn"`
	}
	err := c.conn.Call(ctx, "turn/start", p, &out)
	return out.Turn, err
}

func (c *Client) TurnSteer(ctx context.Context, p TurnSteerParams) (string, error) {
	var out struct {
		TurnID string `json:"turnId"`
	}
	err := c.conn.Call(ctx, "turn/steer", p, &out)
	if err != nil {
		return "", err
	}
	if out.TurnID == "" {
		// Some builds return the turn id at the top level differently.
		return "", nil
	}
	return out.TurnID, nil
}

func (c *Client) TurnInterrupt(ctx context.Context, threadID, turnID string) error {
	return c.conn.Call(ctx, "turn/interrupt", TurnInterruptParams{
		ThreadID: threadID,
		TurnID:   turnID,
	}, &map[string]any{})
}

// DecodeNotification is a helper for tests / runners.
func DecodeNotification(method string, params json.RawMessage) (Notification, error) {
	if method == "" {
		return Notification{}, fmt.Errorf("empty notification method")
	}
	return Notification{Method: method, Params: params}, nil
}
