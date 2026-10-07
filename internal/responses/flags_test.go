package responses

import (
	"strings"
	"testing"

	"recur/internal/domain"
)

func TestApplyFlagsWiresNestedPaths(t *testing.T) {
	dst := map[string]any{}
	dims := []domain.CompatibleFlagDim{
		{ID: "effort", Wire: "reasoning.effort"},
		{ID: "verbosity", Wire: "text.verbosity"},
		{ID: "temp", Wire: "temperature"},
		{ID: "max", Wire: "max_output_tokens"},
		{ID: "skip", Wire: ""},
	}
	ApplyFlags(dst, dims, map[string]string{
		"effort":    "high",
		"verbosity": "low",
		"temp":      "0.2",
		"max":       "1024",
		"skip":      "x",
	})
	r, _ := dst["reasoning"].(map[string]any)
	if r["effort"] != "high" || r["summary"] != "auto" {
		t.Errorf("ApplyFlags reasoning = %v, want effort=high summary=auto", r)
	}
	tx, _ := dst["text"].(map[string]any)
	if tx["verbosity"] != "low" {
		t.Errorf("ApplyFlags text = %v, want verbosity=low", tx)
	}
	if dst["temperature"] != 0.2 {
		t.Errorf("ApplyFlags temperature = %v, want 0.2", dst["temperature"])
	}
	if dst["max_output_tokens"] != 1024 {
		t.Errorf("ApplyFlags max_output_tokens = %v, want 1024", dst["max_output_tokens"])
	}
}

func TestParseSSEJSONLinesWithoutBlank(t *testing.T) {
	raw := "data: {\"type\":\"response.output_text.delta\",\"delta\":\"a\"}\n" +
		"data: {\"type\":\"response.output_text.delta\",\"delta\":\"b\"}\n"
	var got string
	if err := ParseSSE(strings.NewReader(raw), func(e Event) { got += e.Delta }); err != nil {
		t.Fatalf("ParseSSE: %v", err)
	}
	if got != "ab" {
		t.Errorf("ParseSSE(%q) deltas = %q, want ab", raw, got)
	}
}

func TestParseSSEObjectDelta(t *testing.T) {
	raw := "data: {\"type\":\"response.output_text.delta\",\"delta\":{\"text\":\"hi\"}}\n\n"
	var got string
	if err := ParseSSE(strings.NewReader(raw), func(e Event) { got += e.Delta }); err != nil {
		t.Fatalf("ParseSSE: %v", err)
	}
	if got != "hi" {
		t.Errorf("ParseSSE object delta = %q, want hi", got)
	}
}

func TestParseSSETextAndFunctionCall(t *testing.T) {
	raw := strings.Join([]string{
		`event: response.created`,
		`data: {"type":"response.created","response":{"id":"resp_1"}}`,
		``,
		`event: response.output_text.delta`,
		`data: {"type":"response.output_text.delta","delta":"hi"}`,
		``,
		`event: response.output_item.done`,
		`data: {"type":"response.output_item.done","item":{"type":"function_call","id":"fc_1","call_id":"call_1","name":"Read","arguments":"{\"file_path\":\"a.go\"}"}}`,
		``,
		`event: response.completed`,
		`data: {"type":"response.completed","response":{"id":"resp_1","usage":{"input_tokens":10,"output_tokens":4}}}`,
		``,
	}, "\n")
	var evs []Event
	if err := ParseSSE(strings.NewReader(raw), func(e Event) { evs = append(evs, e) }); err != nil {
		t.Fatalf("ParseSSE: %v", err)
	}
	if len(evs) != 4 {
		t.Fatalf("ParseSSE events = %d, want 4: %+v", len(evs), evs)
	}
	if evs[0].ResponseID != "resp_1" {
		t.Errorf("ParseSSE created id = %q, want resp_1", evs[0].ResponseID)
	}
	if evs[1].Delta != "hi" {
		t.Errorf("ParseSSE delta = %q, want hi", evs[1].Delta)
	}
	if evs[2].Call == nil || evs[2].Call.Name != "Read" || evs[2].Call.CallID != "call_1" {
		t.Errorf("ParseSSE call = %+v, want Read/call_1", evs[2].Call)
	}
	if evs[3].Usage == nil || evs[3].Usage.InputTokens != 10 {
		t.Errorf("ParseSSE usage = %+v, want input 10", evs[3].Usage)
	}
}

func TestDecodeResponseJSONFallback(t *testing.T) {
	body := []byte(`{"id":"resp_2","status":"completed","output":[{"type":"message","content":[{"type":"output_text","text":"done"}]}],"usage":{"input_tokens":3,"output_tokens":1}}`)
	evs, err := DecodeResponse(body)
	if err != nil {
		t.Fatalf("DecodeResponse: %v", err)
	}
	var text string
	var usage *Usage
	for _, e := range evs {
		if e.Delta != "" {
			text += e.Delta
		}
		if e.Usage != nil {
			usage = e.Usage
		}
	}
	if text != "done" {
		t.Errorf("DecodeResponse text = %q, want done", text)
	}
	if usage == nil || usage.InputTokens != 3 {
		t.Errorf("DecodeResponse usage = %+v, want input 3", usage)
	}
}

func TestDecodeResponseIgnoresUnknown(t *testing.T) {
	if ev, ok := decodeEvent([]byte(`{"foo":1}`)); ok {
		t.Errorf("decodeEvent(unknown) = %+v ok=true, want false", ev)
	}
}

func TestDecodeResponseRejectsMalformedEnvelope(t *testing.T) {
	for _, body := range []string{
		`{}`,
		`null`,
		`[]`,
		`{"foo":1}`,
		`{"id":"r"}`,
	} {
		evs, err := DecodeResponse([]byte(body))
		if err == nil {
			t.Errorf("DecodeResponse(%s) = %+v err=nil, want not a Responses object", body, evs)
		}
	}
}

func TestDecodeResponseRejectsNonterminalStatus(t *testing.T) {
	for _, status := range []string{"queued", "in_progress"} {
		body := []byte(`{"id":"r","status":"` + status + `","output":[]}`)
		evs, err := DecodeResponse(body)
		if err == nil || !strings.Contains(err.Error(), status) {
			t.Errorf("DecodeResponse(status=%s) = %+v err=%v, want status error", status, evs, err)
		}
	}
}

func TestDecodeResponseIncompleteIsNotCompleted(t *testing.T) {
	body := []byte(`{"id":"r","status":"incomplete","output":[{"type":"message","content":[{"type":"output_text","text":"hi"}]}],"usage":{"input_tokens":1,"output_tokens":1}}`)
	evs, err := DecodeResponse(body)
	if err != nil {
		t.Fatalf("DecodeResponse(incomplete): %v", err)
	}
	var sawCompleted, sawIncomplete bool
	for _, e := range evs {
		if e.Type == "response.completed" {
			sawCompleted = true
		}
		if e.Type == "response.incomplete" {
			sawIncomplete = true
		}
	}
	if sawCompleted || !sawIncomplete {
		t.Errorf("DecodeResponse(incomplete) events = %+v, want incomplete without completed", evs)
	}
}
