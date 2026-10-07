package managed

import (
	"runtime"
	"testing"
)

func TestRustAnalyzerAssetForHost(t *testing.T) {
	asset, err := rustAnalyzerAsset()
	if err != nil {
		t.Skip(err)
	}
	if _, ok := rustAnalyzerSHA256[asset]; !ok {
		t.Fatalf("no sha for %s", asset)
	}
	switch runtime.GOOS + "/" + runtime.GOARCH {
	case "darwin/arm64":
		if asset != "rust-analyzer-aarch64-apple-darwin.gz" {
			t.Fatalf("got %s", asset)
		}
	case "darwin/amd64":
		if asset != "rust-analyzer-x86_64-apple-darwin.gz" {
			t.Fatalf("got %s", asset)
		}
	}
}

func TestMarksmanAssetForHost(t *testing.T) {
	asset, err := marksmanAsset()
	if err != nil {
		t.Skip(err)
	}
	if _, ok := marksmanSHA256[asset]; !ok {
		t.Fatalf("no sha for %s", asset)
	}
}

func TestRuffAssetForHost(t *testing.T) {
	asset, err := ruffAsset()
	if err != nil {
		t.Skip(err)
	}
	if _, ok := ruffSHA256[asset]; !ok {
		t.Fatalf("no sha for %s", asset)
	}
}

func TestTaploAssetForHost(t *testing.T) {
	asset, err := taploAsset()
	if err != nil {
		t.Skip(err)
	}
	if _, ok := taploSHA256[asset]; !ok {
		t.Fatalf("no sha for %s", asset)
	}
}
