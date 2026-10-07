package cursor

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

func TestRedactBridgeLineStripsAuthToken(t *testing.T) {
	line := readyPrefix + `{"schemaVersion":1,"authToken":"secret-token","url":"http://127.0.0.1:1"}`
	got := redactBridgeLine(line)
	if strings.Contains(got, "secret-token") {
		t.Fatalf("authToken leaked into log line: %s", got)
	}
	if !strings.Contains(got, "[redacted]") {
		t.Fatalf("expected redacted token marker, got %s", got)
	}
	if !strings.Contains(got, "http://127.0.0.1:1") {
		t.Fatalf("non-secret fields should survive redaction: %s", got)
	}
}

func TestRedactBridgeLineLeavesOrdinaryLines(t *testing.T) {
	in := "some bridge warning: thing failed"
	if got := redactBridgeLine(in); got != in {
		t.Fatalf("got %q, want unchanged", got)
	}
}

func TestBridgeStderrLogAndTail(t *testing.T) {
	dir := t.TempDir()
	bin := writeFakeBridge(t, dir)

	b := &Bridge{Bin: bin, LogDir: dir}
	ctx := t.Context()
	if err := b.Start(ctx); err != nil {
		t.Fatalf("Start: %v", err)
	}
	defer func() { _ = b.Close() }()

	// Give the scanner a moment to record the ready line.
	waitFor(t, 2*time.Second, func() bool {
		return strings.Contains(b.StderrTail(), readyPrefix)
	})

	tail := b.StderrTail()
	if strings.Contains(tail, "test-token") {
		t.Fatalf("auth token leaked into StderrTail: %s", tail)
	}
	if !strings.Contains(tail, `"authToken":"[redacted]"`) {
		t.Fatalf("ready line should keep a redacted authToken field, got %s", tail)
	}
	if !strings.Contains(tail, bridgeLogFile) && !strings.Contains(tail, dir) {
		t.Fatalf("StderrTail should mention the log path, got %q", tail)
	}

	raw, err := os.ReadFile(filepath.Join(dir, bridgeLogFile))
	if err != nil {
		t.Fatalf("read log: %v", err)
	}
	body := string(raw)
	if !strings.Contains(body, "starting cursor-sdk-bridge") {
		t.Fatalf("log missing start marker: %s", body)
	}
	if strings.Contains(body, "test-token") {
		t.Fatalf("auth token leaked onto disk: %s", body)
	}
	if !strings.Contains(body, readyPrefix) {
		t.Fatalf("log missing ready line: %s", body)
	}
}

func writeFakeBridge(t *testing.T, dir string) string {
	t.Helper()
	if err := os.MkdirAll(dir, 0o755); err != nil {
		t.Fatalf("mkdir: %v", err)
	}
	bin := filepath.Join(dir, "cursor-sdk-bridge")
	const script = `#!/usr/bin/env python3
import json, os, sys, time
from http.server import BaseHTTPRequestHandler
from socketserver import TCPServer

class H(BaseHTTPRequestHandler):
    def do_POST(self):
        n = int(self.headers.get("Content-Length", 0))
        if n:
            self.rfile.read(n)
        if self.path.endswith("/GetVersion"):
            body = b'{"bridgeVersion":"fake","protocolVersion":"sdk.v1","capabilities":["agent.create","agent.send"]}'
        else:
            body = b"{}"
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)
    def log_message(self, *args):
        pass

print("bridge boot noise", file=sys.stderr, flush=True)
httpd = TCPServer(("127.0.0.1", 0), H)
port = httpd.server_address[1]
disc = {
    "schemaVersion": 1,
    "serverVersion": "fake",
    "pid": os.getpid(),
    "transport": "tcp",
    "protocol": "connect",
    "host": "127.0.0.1",
    "port": port,
    "url": f"http://127.0.0.1:{port}",
    "authToken": "test-token",
}
print("cursor-sdk-bridge ready " + json.dumps(disc), file=sys.stderr, flush=True)
print("post-ready diagnostic line", file=sys.stderr, flush=True)
httpd.serve_forever()
`
	if err := os.WriteFile(bin, []byte(script), 0o755); err != nil {
		t.Fatalf("write fake bridge: %v", err)
	}
	return bin
}

func waitFor(t *testing.T, d time.Duration, ok func() bool) {
	t.Helper()
	deadline := time.Now().Add(d)
	for time.Now().Before(deadline) {
		if ok() {
			return
		}
		time.Sleep(20 * time.Millisecond)
	}
	t.Fatal("timed out")
}
