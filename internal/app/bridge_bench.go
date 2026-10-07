package app

import (
	"encoding/json"
	"log"
	"os"
	"path/filepath"
	"sync"
)

// Bridge ping helpers for measuring webview↔Go round-trip cost.
// Enabled when RECUR_BENCH_BRIDGE=1; the frontend runs the loop and calls
// BridgeBenchDone with stats. Not part of the product surface.

var (
	bridgeBenchOnce sync.Once
	bridgeBenchPath string
)

func bridgeBenchEnabled() bool {
	return os.Getenv("RECUR_BENCH_BRIDGE") == "1"
}

func bridgeBenchOutPath() string {
	bridgeBenchOnce.Do(func() {
		bridgeBenchPath = filepath.Join(os.TempDir(), "recur-bridge-bench.json")
	})
	return bridgeBenchPath
}

// BridgeBenchEnabled reports whether the frontend should run the RTT bench at boot.
func (s *CodeService) BridgeBenchEnabled() bool {
	return bridgeBenchEnabled()
}

// BridgePing is a no-op echo used to time a minimal binding round-trip.
func (s *CodeService) BridgePing(n int64) int64 {
	return n
}

// BridgeEcho returns len(payload). Times serialization + IPC for larger bodies.
// Uses string (not []byte) so the JS side can pass a plain string without
// TypedArray → Go []uint8 decode failures.
func (s *CodeService) BridgeEcho(payload string) int {
	return len(payload)
}

// BridgeBenchDone persists frontend-measured stats for the driving script to read.
func (s *CodeService) BridgeBenchDone(statsJSON string) {
	path := bridgeBenchOutPath()
	if err := os.WriteFile(path, []byte(statsJSON), 0o644); err != nil {
		log.Printf("bridge-bench: write %s: %v", path, err)
		return
	}
	var pretty any
	if json.Unmarshal([]byte(statsJSON), &pretty) == nil {
		b, _ := json.MarshalIndent(pretty, "", "  ")
		log.Printf("bridge-bench results (%s):\n%s", path, b)
	} else {
		log.Printf("bridge-bench results written to %s", path)
	}
}
