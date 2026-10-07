package cursor

import (
	"context"
	"os"
	"testing"
	"time"
)

// TestBridgeSmoke drives a REAL cursor-sdk-bridge. It is skipped unless
// RECUR_CURSOR_BRIDGE points at one, so the default `go test ./...` stays
// hermetic and fast (spawning the bridge costs a ~117MB Node cold start).
//
// To run it:
//
//	pip download cursor-sdk --no-deps -d /tmp/cs && unzip -q /tmp/cs/*.whl -d /tmp/cs/w
//	RECUR_CURSOR_BRIDGE=/tmp/cs/w/cursor_sdk/_vendor/bridge go test ./internal/cursor -run Smoke -v
//
// It asserts only what needs no Cursor credentials: process spawn, discovery
// parse, and the control RPCs. Credentialed calls are covered by
// TestBridgeCatalogSmoke below.
func TestBridgeSmoke(t *testing.T) {
	bin := requireBridge(t)

	ctx, cancel := context.WithTimeout(context.Background(), 90*time.Second)
	defer cancel()

	b := &Bridge{Bin: bin, Workspace: t.TempDir()}
	if err := b.Start(ctx); err != nil {
		t.Fatalf("start bridge: %v", err)
	}
	defer func() { _ = b.Close() }()

	d := b.Discovery()
	if d.Port == 0 || d.URL == "" {
		t.Fatalf("discovery missing url/port: %+v", d)
	}
	if d.Transport != "tcp" || d.Protocol != "connect" {
		t.Fatalf("unexpected transport: %+v", d)
	}

	v, err := b.Client().GetVersion(ctx)
	if err != nil {
		t.Fatalf("GetVersion: %v", err)
	}
	if v.ProtocolVersion != "sdk.v1" {
		t.Fatalf("protocol = %q, want sdk.v1 — the bridge changed its wire contract", v.ProtocolVersion)
	}
	// These are the capabilities CursorRunner actually depends on. If a future
	// bridge drops one, this test names it rather than failing at run time.
	for _, want := range []string{"agent.create", "agent.send", "run.cancel", "run.observe"} {
		if !v.Has(want) {
			t.Errorf("bridge missing capability %q (has %v)", want, v.Capabilities)
		}
	}

	if err := b.Client().Ping(ctx); err != nil {
		t.Fatalf("Ping: %v", err)
	}
}

// TestBridgeCatalogSmoke checks the credentialed path end to end. Needs
// CURSOR_API_KEY in addition to the bridge.
func TestBridgeCatalogSmoke(t *testing.T) {
	bin := requireBridge(t)
	key := os.Getenv("CURSOR_API_KEY")
	if key == "" {
		t.Skip("CURSOR_API_KEY not set")
	}

	ctx, cancel := context.WithTimeout(context.Background(), 90*time.Second)
	defer cancel()

	b := &Bridge{Bin: bin, APIKey: key, Workspace: t.TempDir()}
	if err := b.Start(ctx); err != nil {
		t.Fatalf("start bridge: %v", err)
	}
	defer func() { _ = b.Close() }()

	models, err := b.Client().ListModels(ctx, key)
	if err != nil {
		t.Fatalf("ListModels: %v", err)
	}
	if len(models) == 0 {
		t.Fatal("ListModels returned no models")
	}
	t.Logf("%d models, first = %s (%s) params=%d variants=%d",
		len(models), models[0].ID, models[0].DisplayName, len(models[0].Parameters), len(models[0].Variants))
	for _, m := range models {
		if len(m.Parameters) > 0 {
			t.Logf("  %s parameters: %+v", m.ID, m.Parameters)
			break
		}
	}
}

func requireBridge(t *testing.T) string {
	t.Helper()
	p := os.Getenv(EnvBridgePath)
	if p == "" {
		t.Skipf("%s not set; skipping real-bridge smoke test", EnvBridgePath)
	}
	bin := ResolveLauncher(p)
	if bin == "" {
		t.Fatalf("not a cursor-sdk-bridge: %s", p)
	}
	return bin
}
