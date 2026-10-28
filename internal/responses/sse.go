package responses

import (
	"bufio"
	"bytes"
	"encoding/json"
	"fmt"
	"io"
	"strings"
)

// ParseSSE reads an OpenAI Responses SSE stream and invokes emit for each event.
// Unknown event types are ignored. A final [DONE] frame ends the stream.
//
// Local servers (LM Studio, llama.cpp) often emit `data: {json}\n` without the
// extra blank line SSE uses as a dispatch boundary. Waiting for that blank
// concatenates later events; json.Unmarshal then keeps only the first object,
// so the chat freezes while the model is still generating.
func ParseSSE(r io.Reader, emit func(Event)) error {
	br := bufio.NewReader(r)
	var data bytes.Buffer
	flush := func() {
		payload := bytes.TrimSpace(data.Bytes())
		data.Reset()
		if len(payload) == 0 || bytes.Equal(payload, []byte("[DONE]")) {
			return
		}
		ev, ok := decodeEvent(payload)
		if ok && emit != nil {
			emit(ev)
		}
	}
	for {
		line, err := br.ReadString('\n')
		if line != "" {
			line = strings.TrimRight(line, "\r\n")
			switch {
			case line == "":
				flush()
			case strings.HasPrefix(line, "data:"):
				if data.Len() > 0 {
					data.WriteByte('\n')
				}
				data.WriteString(strings.TrimSpace(strings.TrimPrefix(line, "data:")))
				if json.Valid(bytes.TrimSpace(data.Bytes())) {
					flush()
				}
			}
		}
		if err == io.EOF {
			flush()
			return nil
		}
		if err != nil {
			return err
		}
	}
}

// DecodeResponse parses a non-stream JSON response body into the same events
// a stream would have produced (function calls, final text, usage).
//
// The body must be a Responses object (or `{response: …}` wrap). `{}` / `null`
// / unrelated JSON is an error — not a synthetic completed turn. Nonterminal
// statuses (queued, in_progress) error; incomplete is a real terminal event so
// Ping can accept a token-capped probe while the harness still rejects it.
func DecodeResponse(body []byte) ([]Event, error) {
	body = bytes.TrimSpace(body)
	if len(body) == 0 || body[0] != '{' {
		return nil, fmt.Errorf("not a Responses object")
	}
	var envelope struct {
		Object string `json:"object"`
		ID     string `json:"id"`
		Status string `json:"status"`
		Error  *struct {
			Message string `json:"message"`
		} `json:"error"`
		Usage    json.RawMessage `json:"usage"`
		Output   json.RawMessage `json:"output"`
		Response json.RawMessage `json:"response"`
	}
	if err := json.Unmarshal(body, &envelope); err != nil {
		return nil, err
	}
	// Some proxies wrap the response object.
	if !isJSONArray(envelope.Output) && isJSONObject(envelope.Response) {
		if err := json.Unmarshal(envelope.Response, &envelope); err != nil {
			return nil, err
		}
	}
	errMsg := ""
	if envelope.Error != nil {
		errMsg = envelope.Error.Message
	}
	if !isResponsesObject(envelope.Object, envelope.ID, envelope.Status, envelope.Output, errMsg) {
		return nil, fmt.Errorf("not a Responses object")
	}
	var out []Event
	id := envelope.ID
	if errMsg != "" {
		out = append(out, Event{Type: "error", ResponseID: id, Status: envelope.Status, ErrorMessage: errMsg})
		return out, nil
	}
	termType, err := jsonResponseTerminal(envelope.Status)
	if err != nil {
		return nil, err
	}
	var items []json.RawMessage
	if isJSONArray(envelope.Output) {
		_ = json.Unmarshal(envelope.Output, &items)
	}
	for _, raw := range items {
		evs := eventsFromOutputItem(raw)
		for _, ev := range evs {
			ev.ResponseID = id
			out = append(out, ev)
		}
	}
	term := Event{Type: termType, ResponseID: id, Status: envelope.Status, Usage: parseUsage(envelope.Usage)}
	if term.Status == "" && termType == "response.completed" {
		term.Status = "completed"
	}
	out = append(out, term)
	return out, nil
}

func isResponsesObject(object, id, status string, output json.RawMessage, errMsg string) bool {
	if errMsg != "" || object == "response" || isResponseStatus(status) {
		return true
	}
	return id != "" && isJSONArray(output)
}

func isResponseStatus(status string) bool {
	switch status {
	case "completed", "failed", "cancelled", "canceled", "incomplete", "in_progress", "queued":
		return true
	default:
		return false
	}
}

func jsonResponseTerminal(status string) (string, error) {
	switch status {
	case "", "completed":
		return "response.completed", nil
	case "incomplete":
		return "response.incomplete", nil
	case "failed", "cancelled", "canceled":
		return "response.failed", nil
	default:
		return "", fmt.Errorf("responses status %s", status)
	}
}

func isJSONObject(raw json.RawMessage) bool {
	s := bytes.TrimSpace(raw)
	return len(s) > 0 && s[0] == '{'
}

func isJSONArray(raw json.RawMessage) bool {
	s := bytes.TrimSpace(raw)
	return len(s) > 0 && s[0] == '['
}

func decodeEvent(payload []byte) (Event, bool) {
	var head struct {
		Type     string          `json:"type"`
		Delta    json.RawMessage `json:"delta"`
		Text     string          `json:"text"`
		ItemID   string          `json:"item_id"`
		Response json.RawMessage `json:"response"`
		Item     json.RawMessage `json:"item"`
		Error    json.RawMessage `json:"error"`
	}
	if json.Unmarshal(payload, &head) != nil || head.Type == "" {
		return Event{}, false
	}
	delta := parseDelta(head.Delta)
	if delta == "" {
		delta = head.Text
	}
	ev := Event{Type: head.Type, ItemID: head.ItemID, Delta: delta}
	if ev.ItemID == "" && len(head.Item) > 0 {
		var idobj struct {
			ID string `json:"id"`
		}
		if json.Unmarshal(head.Item, &idobj) == nil {
			ev.ItemID = idobj.ID
		}
	}
	if len(head.Response) > 0 {
		var resp struct {
			ID     string          `json:"id"`
			Status string          `json:"status"`
			Usage  json.RawMessage `json:"usage"`
			Error  *struct {
				Message string `json:"message"`
			} `json:"error"`
			Output []json.RawMessage `json:"output"`
		}
		if json.Unmarshal(head.Response, &resp) == nil {
			ev.ResponseID = resp.ID
			ev.Status = resp.Status
			ev.Usage = parseUsage(resp.Usage)
			if resp.Error != nil {
				ev.ErrorMessage = resp.Error.Message
			}
			if head.Type == "response.completed" || head.Type == "response.failed" {
				ev.Output = resp.Output
				for _, raw := range resp.Output {
					for _, sub := range eventsFromOutputItem(raw) {
						if sub.Call != nil && ev.Call == nil {
							ev.Call = sub.Call
						}
						if sub.OutputText != "" && ev.OutputText == "" {
							ev.OutputText = sub.OutputText
						}
					}
				}
			}
		}
	}
	if len(head.Item) > 0 {
		if head.Type == "response.output_item.done" {
			ev.Output = []json.RawMessage{head.Item}
		}
		if call := parseFunctionCall(head.Item); call != nil {
			ev.Call = call
		}
		var msg struct {
			Type    string `json:"type"`
			Content []struct {
				Type string `json:"type"`
				Text string `json:"text"`
			} `json:"content"`
		}
		if json.Unmarshal(head.Item, &msg) == nil && msg.Type == "message" {
			var b strings.Builder
			for _, c := range msg.Content {
				if c.Type == "output_text" || c.Type == "text" {
					b.WriteString(c.Text)
				}
			}
			ev.OutputText = b.String()
		}
	}
	if len(head.Error) > 0 {
		var e struct {
			Message string `json:"message"`
		}
		if json.Unmarshal(head.Error, &e) == nil {
			ev.ErrorMessage = e.Message
		}
	}
	return ev, true
}

func eventsFromOutputItem(raw json.RawMessage) []Event {
	var item struct {
		Type      string `json:"type"`
		ID        string `json:"id"`
		CallID    string `json:"call_id"`
		Name      string `json:"name"`
		Arguments string `json:"arguments"`
		Content   []struct {
			Type string `json:"type"`
			Text string `json:"text"`
		} `json:"content"`
	}
	if json.Unmarshal(raw, &item) != nil {
		return nil
	}
	switch item.Type {
	case "function_call":
		return []Event{{
			Type: "response.output_item.done",
			Call: &FunctionCall{ID: item.ID, CallID: item.CallID, Name: item.Name, Arguments: item.Arguments},
		}}
	case "message":
		var b strings.Builder
		for _, c := range item.Content {
			if c.Type == "output_text" || c.Type == "text" {
				b.WriteString(c.Text)
			}
		}
		if b.Len() == 0 {
			return nil
		}
		return []Event{{Type: "response.output_text.delta", Delta: b.String(), OutputText: b.String()}}
	default:
		return nil
	}
}

func parseDelta(raw json.RawMessage) string {
	if len(raw) == 0 || bytes.Equal(raw, []byte("null")) {
		return ""
	}
	var s string
	if json.Unmarshal(raw, &s) == nil {
		return s
	}
	var obj struct {
		Text string `json:"text"`
	}
	if json.Unmarshal(raw, &obj) == nil {
		return obj.Text
	}
	return ""
}

func parseFunctionCall(raw json.RawMessage) *FunctionCall {
	var item struct {
		Type      string `json:"type"`
		ID        string `json:"id"`
		CallID    string `json:"call_id"`
		Name      string `json:"name"`
		Arguments string `json:"arguments"`
	}
	if json.Unmarshal(raw, &item) != nil || item.Type != "function_call" {
		return nil
	}
	return &FunctionCall{ID: item.ID, CallID: item.CallID, Name: item.Name, Arguments: item.Arguments}
}

func parseUsage(raw json.RawMessage) *Usage {
	if len(raw) == 0 {
		return nil
	}
	var u struct {
		InputTokens          int64 `json:"input_tokens"`
		OutputTokens         int64 `json:"output_tokens"`
		TotalTokens          int64 `json:"total_tokens"`
		CacheReadInputTokens int64 `json:"cache_read_input_tokens"`
		InputTokensDetails   *struct {
			CachedTokens int64 `json:"cached_tokens"`
		} `json:"input_tokens_details"`
	}
	if json.Unmarshal(raw, &u) != nil {
		return nil
	}
	cache := u.CacheReadInputTokens
	if cache == 0 && u.InputTokensDetails != nil {
		cache = u.InputTokensDetails.CachedTokens
	}
	if u.InputTokens == 0 && u.OutputTokens == 0 && cache == 0 {
		return nil
	}
	return &Usage{
		InputTokens:     u.InputTokens,
		OutputTokens:    u.OutputTokens,
		CacheReadTokens: cache,
		TotalTokens:     u.TotalTokens,
	}
}
