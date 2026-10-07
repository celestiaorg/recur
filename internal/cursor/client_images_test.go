package cursor

import (
	"context"
	"encoding/binary"
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"testing"
	"time"
)

// TestSendIncludesImages verifies the Connect payload carries SDKUserMessage
// images alongside text (Cursor multimodal chat paste).
func TestSendIncludesImages(t *testing.T) {
	var gotPayload []byte
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		raw, _ := io.ReadAll(r.Body)
		if len(raw) >= 5 {
			size := binary.BigEndian.Uint32(raw[1:5])
			if int(5+size) <= len(raw) {
				gotPayload = raw[5 : 5+size]
			}
		}
		w.Header().Set("Content-Type", "application/connect+json")
		w.WriteHeader(http.StatusOK)
		// Empty body → client sees EOF and returns nil (clean end of stream).
	}))
	t.Cleanup(srv.Close)

	cl := newClient(srv.URL, "test-token")
	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
	defer cancel()
	err := cl.Send(ctx, "agent-1", "see here", []Image{{
		Data: "AAAA", MimeType: "image/png",
	}}, func(RunStreamEvent) error { return nil })
	if err != nil {
		t.Fatalf("Send: %v", err)
	}
	if len(gotPayload) == 0 {
		t.Fatal("expected framed JSON payload")
	}
	var req struct {
		Message struct {
			Text   string `json:"text"`
			Images []struct {
				Data struct {
					Data     string `json:"data"`
					MimeType string `json:"mimeType"`
				} `json:"data"`
			} `json:"images"`
		} `json:"message"`
	}
	if err := json.Unmarshal(gotPayload, &req); err != nil {
		t.Fatalf("unmarshal: %v\n%s", err, gotPayload)
	}
	if req.Message.Text != "see here" {
		t.Fatalf("text=%q", req.Message.Text)
	}
	if len(req.Message.Images) != 1 ||
		req.Message.Images[0].Data.Data != "AAAA" ||
		req.Message.Images[0].Data.MimeType != "image/png" {
		t.Fatalf("images=%+v (want nested protobuf SdkImageData)", req.Message.Images)
	}
}

func TestSendEnablesDeltas(t *testing.T) {
	var gotPayload []byte
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		raw, _ := io.ReadAll(r.Body)
		if len(raw) >= 5 {
			size := binary.BigEndian.Uint32(raw[1:5])
			if int(5+size) <= len(raw) {
				gotPayload = raw[5 : 5+size]
			}
		}
		w.Header().Set("Content-Type", "application/connect+json")
		w.WriteHeader(http.StatusOK)
	}))
	t.Cleanup(srv.Close)

	cl := newClient(srv.URL, "test-token")
	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
	defer cancel()
	if err := cl.Send(ctx, "agent-1", "hi", nil, func(RunStreamEvent) error { return nil }); err != nil {
		t.Fatalf("Send: %v", err)
	}
	var req struct {
		Options struct {
			EnableDeltas bool `json:"enableDeltas"`
		} `json:"options"`
	}
	if err := json.Unmarshal(gotPayload, &req); err != nil {
		t.Fatalf("Send unmarshal = %v, want framed JSON\n%s", err, gotPayload)
	}
	if !req.Options.EnableDeltas {
		t.Fatalf("Send options.enableDeltas = false, want true (payload %s)", gotPayload)
	}
}
