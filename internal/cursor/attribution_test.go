package cursor

import (
	"encoding/json"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
)

func TestWriteAttributionConfigRoundTrip(t *testing.T) {
	dir := t.TempDir()
	path := AttributionConfigPath(dir)
	if err := WriteAttributionConfig(path, true, false); err != nil {
		t.Fatal(err)
	}
	body, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	var got AttributionConfig
	if err := json.Unmarshal(body, &got); err != nil {
		t.Fatal(err)
	}
	if !got.AttributeCommitsToAgent || got.AttributePRsToAgent {
		t.Fatalf("got %+v", got)
	}
}

func TestBridgeRootRequiresBinLayout(t *testing.T) {
	dir := t.TempDir()
	// Bare launcher (test fake): not a full bridge tree.
	bare := filepath.Join(dir, "cursor-sdk-bridge")
	if err := os.WriteFile(bare, []byte("#!/bin/sh\n"), 0o755); err != nil {
		t.Fatal(err)
	}
	if got := BridgeRoot(bare); got != "" {
		t.Fatalf("bare launcher BridgeRoot = %q, want empty", got)
	}

	root := filepath.Join(dir, "cursor-bridge")
	launcher := filepath.Join(root, "bin", "cursor-sdk-bridge")
	if err := os.MkdirAll(filepath.Dir(launcher), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(launcher, []byte("#!/bin/sh\n"), 0o755); err != nil {
		t.Fatal(err)
	}
	if got := BridgeRoot(launcher); got != root {
		t.Fatalf("BridgeRoot = %q, want %q", got, root)
	}
	if got := BridgeRoot(root); got != root {
		t.Fatalf("BridgeRoot(root) = %q, want %q", got, root)
	}
}

func TestOwnedBridgeRootOnlyRecurDataDir(t *testing.T) {
	dataDir := t.TempDir()
	owned := filepath.Join(dataDir, BridgeDirName)
	launcher := filepath.Join(owned, "bin", "cursor-sdk-bridge")
	if err := os.MkdirAll(filepath.Dir(launcher), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(launcher, []byte("#!/bin/sh\n"), 0o755); err != nil {
		t.Fatal(err)
	}
	if got := OwnedBridgeRoot(dataDir, launcher); got != owned {
		t.Fatalf("owned = %q, want %q", got, owned)
	}

	// Same bin/ layout, but outside Recur's data dir (pip / RECUR_CURSOR_BRIDGE).
	external := filepath.Join(t.TempDir(), "cursor-bridge")
	extLauncher := filepath.Join(external, "bin", "cursor-sdk-bridge")
	if err := os.MkdirAll(filepath.Dir(extLauncher), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(extLauncher, []byte("#!/bin/sh\n"), 0o755); err != nil {
		t.Fatal(err)
	}
	if got := OwnedBridgeRoot(dataDir, extLauncher); got != "" {
		t.Fatalf("external bridge OwnedBridgeRoot = %q, want empty", got)
	}
}

func TestWriteAttributionPreloadSkipsIdentical(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, attributionPreloadRel)
	if err := writeAttributionPreload(path); err != nil {
		t.Fatal(err)
	}
	fi1, err := os.Stat(path)
	if err != nil {
		t.Fatal(err)
	}
	// Second call must be a no-op (same content) — mtime should not need to
	// change for correctness, but we assert the file is unchanged byte-for-byte.
	body1, _ := os.ReadFile(path)
	if err := writeAttributionPreload(path); err != nil {
		t.Fatal(err)
	}
	body2, _ := os.ReadFile(path)
	if string(body1) != string(body2) {
		t.Fatal("preload rewritten with different content")
	}
	fi2, _ := os.Stat(path)
	if !fi1.ModTime().Equal(fi2.ModTime()) {
		// Best-effort: skip rewrite should avoid touching the file. Some FS
		// round mtime; content equality above is the hard check.
		t.Logf("mtime changed despite identical content (fs quirk): %v → %v", fi1.ModTime(), fi2.ModTime())
	}
}

func TestEnsureAttributionPatchInjectsProvider(t *testing.T) {
	root := t.TempDir()
	writeMinimalBridgeForPatch(t, root)

	if err := EnsureAttributionPatch(root); err != nil {
		t.Fatalf("first patch: %v", err)
	}
	// Idempotent.
	if err := EnsureAttributionPatch(root); err != nil {
		t.Fatalf("second patch: %v", err)
	}

	preload := filepath.Join(root, attributionPreloadRel)
	body, err := os.ReadFile(preload)
	if err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(string(body), attributionProviderMarker) {
		t.Fatal("preload missing provider marker")
	}
	if !strings.Contains(string(body), "__recurInstallSteerRun") {
		t.Fatal("preload missing SteerRun installer")
	}

	entry, err := os.ReadFile(filepath.Join(root, attributionEntrypointRel))
	if err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(string(entry), "recur-attribution-provider.mjs") {
		t.Fatalf("entrypoint missing preload import:\n%s", entry)
	}
	// Shebang preserved.
	if !strings.HasPrefix(string(entry), "#!/usr/bin/env node\n") {
		t.Fatalf("shebang not preserved:\n%s", entry)
	}

	server, err := os.ReadFile(filepath.Join(root, steerRunServerRel))
	if err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(string(server), "__recurInstallSteerRun") {
		t.Fatalf("dist/server.js missing SteerRun hook:\n%s", server)
	}

	for _, rel := range []string{
		filepath.Join("node_modules", "@cursor", "sdk", "dist", "cjs", "616.js"),
		filepath.Join("node_modules", "@cursor", "sdk", "dist", "esm", "357.js"),
	} {
		b, err := os.ReadFile(filepath.Join(root, rel))
		if err != nil {
			t.Fatal(err)
		}
		s := string(b)
		if !strings.Contains(s, "attributionConfigProvider=globalThis."+attributionProviderMarker) {
			t.Errorf("%s not patched", rel)
		}
		assertAgentSkillsStubbed(t, rel, s)
	}
	for _, rel := range []string{
		filepath.Join("node_modules", "@cursor", "sdk", "dist", "cjs", "index.js"),
		filepath.Join("node_modules", "@cursor", "sdk", "dist", "esm", "index.js"),
	} {
		b, err := os.ReadFile(filepath.Join(root, rel))
		if err != nil {
			t.Fatal(err)
		}
		s := string(b)
		if strings.Contains(s, sdkSummaryDeltaAnchors[0].old) {
			t.Errorf("%s still skips summary onDelta", rel)
		}
		if !strings.Contains(s, summaryDeltaPatchNeedle()) {
			t.Errorf("%s missing summary onDelta marker", rel)
		}
	}
}

func TestEnsureSteerRunServerHookRejectsUnmatched(t *testing.T) {
	root := t.TempDir()
	writeMinimalBridgeForPatch(t, root)
	path := filepath.Join(root, steerRunServerRel)
	if err := os.WriteFile(path, []byte("function mountSdkBridgeRoutes() { return 0 }\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	err := EnsureAttributionPatch(root)
	if !AttributionPatchUnsupported(err) {
		t.Fatalf("EnsureAttributionPatch() error = %v, want unsupported (unmatched steer hook)", err)
	}
}

func TestEnsureSteerRunServerHookMissingOK(t *testing.T) {
	root := t.TempDir()
	writeMinimalBridgeForPatch(t, root)
	if err := os.Remove(filepath.Join(root, steerRunServerRel)); err != nil {
		t.Fatal(err)
	}
	if err := EnsureAttributionPatch(root); err != nil {
		t.Fatalf("EnsureAttributionPatch() error = %v, want nil (missing dist/server.js)", err)
	}
}

// TestEnsureAttributionPatchAgainstPinnedWheel downloads the pinned cursor-sdk
// wheel and asserts the minify anchors still match. Opt-in (network + ~50MB):
//
//	RECUR_CURSOR_ATTRIBUTION_PATCH_TEST=1 go test ./internal/cursor -run AgainstPinnedWheel -v
func TestEnsureAttributionPatchAgainstPinnedWheel(t *testing.T) {
	if os.Getenv("RECUR_CURSOR_ATTRIBUTION_PATCH_TEST") == "" {
		t.Skip("set RECUR_CURSOR_ATTRIBUTION_PATCH_TEST=1 to download the pinned wheel")
	}
	ctx := t.Context()
	w, version, err := resolveWheel(ctx)
	if err != nil {
		t.Fatalf("resolveWheel: %v", err)
	}
	if version != pinnedSDKVersion {
		t.Fatalf("resolved %q, want pinned %s", version, pinnedSDKVersion)
	}
	staging := t.TempDir()
	archive := filepath.Join(staging, "wheel.zip")
	if _, err := download(ctx, w.URL, archive, nil); err != nil {
		t.Fatalf("download: %v", err)
	}
	root := filepath.Join(staging, "bridge")
	if err := extractBridge(archive, root, nil); err != nil {
		t.Fatalf("extract: %v", err)
	}
	if err := EnsureAttributionPatch(root); err != nil {
		t.Fatalf("EnsureAttributionPatch on %s: %v", pinnedSDKVersion, err)
	}
	if err := EnsureAttributionPatch(root); err != nil {
		t.Fatalf("idempotent EnsureAttributionPatch: %v", err)
	}
	for _, rel := range []string{
		filepath.Join("node_modules", "@cursor", "sdk", "dist", "cjs"),
		filepath.Join("node_modules", "@cursor", "sdk", "dist", "esm"),
	} {
		dir := filepath.Join(root, rel)
		entries, err := os.ReadDir(dir)
		if err != nil {
			t.Fatal(err)
		}
		found := false
		for _, e := range entries {
			if e.IsDir() || !strings.HasSuffix(e.Name(), ".js") {
				continue
			}
			b, err := os.ReadFile(filepath.Join(dir, e.Name()))
			if err != nil {
				t.Fatal(err)
			}
			if strings.Contains(string(b), "attributionConfigProvider=globalThis."+attributionProviderMarker) {
				found = true
				assertAgentSkillsStubbed(t, filepath.Join(rel, e.Name()), string(b))
				t.Logf("patched %s/%s", rel, e.Name())
				break
			}
		}
		if !found {
			t.Errorf("no patched SDK bundle under %s", rel)
		}
	}
	skip := sdkSummaryDeltaAnchors[0].old
	marker := summaryDeltaPatchNeedle()
	for _, rel := range []string{
		filepath.Join("node_modules", "@cursor", "sdk", "dist", "cjs", "index.js"),
		filepath.Join("node_modules", "@cursor", "sdk", "dist", "esm", "index.js"),
	} {
		b, err := os.ReadFile(filepath.Join(root, rel))
		if err != nil {
			t.Fatal(err)
		}
		s := string(b)
		if strings.Contains(s, skip) {
			t.Errorf("%s still skips summary onDelta", rel)
		}
		if !strings.Contains(s, marker) {
			t.Errorf("%s missing summary onDelta marker", rel)
		}
	}
	preload, err := os.ReadFile(filepath.Join(root, attributionPreloadRel))
	if err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(string(preload), "__recurInstallSteerRun") {
		t.Error("preload missing SteerRun installer")
	}
	server, err := os.ReadFile(filepath.Join(root, steerRunServerRel))
	if err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(string(server), "__recurInstallSteerRun") {
		t.Error("dist/server.js missing SteerRun hook")
	}
}

func TestEnsureAttributionPatchIgnoresUnmatchedCJS(t *testing.T) {
	root := t.TempDir()
	writeMinimalBridgeForPatch(t, root)
	// cjs present but using an unknown minify site — esm still matches.
	// The bridge loads ESM, so a cjs-only miss must not fail the patch.
	cjs := filepath.Join(root, "node_modules", "@cursor", "sdk", "dist", "cjs", "616.js")
	if err := os.WriteFile(cjs, []byte("prefix;getAgentSkills:r.workspaceServices.getAgentSkills};return void 0===m?new ZZ(n):new YY(n,m);suffix"), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := EnsureAttributionPatch(root); err != nil {
		t.Fatalf("EnsureAttributionPatch() error = %v, want nil (cjs miss, esm matched)", err)
	}
	esm := filepath.Join(root, "node_modules", "@cursor", "sdk", "dist", "esm", "357.js")
	body, err := os.ReadFile(esm)
	if err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(string(body), "attributionConfigProvider=globalThis."+attributionProviderMarker) {
		t.Fatalf("EnsureAttributionPatch() left esm unpatched")
	}
	assertAgentSkillsStubbed(t, "esm", string(body))
	cjsBody, err := os.ReadFile(cjs)
	if err != nil {
		t.Fatal(err)
	}
	if strings.Contains(string(cjsBody), stubAgentSkillsAssign) {
		t.Fatal("unmatched cjs should not be skill-stubbed (attribution miss must stay unmodified)")
	}
}

func TestEnsureAttributionPatchRejectsUnmatchedESM(t *testing.T) {
	root := t.TempDir()
	writeMinimalBridgeForPatch(t, root)
	esm := filepath.Join(root, "node_modules", "@cursor", "sdk", "dist", "esm", "357.js")
	if err := os.WriteFile(esm, []byte("prefix;getAgentSkills:r.workspaceServices.getAgentSkills};return void 0===b?new ZZ(o):new YY(o,b);suffix"), 0o644); err != nil {
		t.Fatal(err)
	}
	err := EnsureAttributionPatch(root)
	if !AttributionPatchUnsupported(err) {
		t.Fatalf("EnsureAttributionPatch() error = %v, want unsupported (unmatched esm)", err)
	}
}

func TestEnsureAttributionPatchRejectsUnmatchedSummaryDelta(t *testing.T) {
	root := t.TempDir()
	writeMinimalBridgeForPatch(t, root)
	// Attribution already injected in the ESM executor bundle, but the
	// summary-onDelta skip is still present in a form the exact anchor
	// does not match (minifier drift / already-patched older install).
	esm := filepath.Join(root, "node_modules", "@cursor", "sdk", "dist", "esm", "357.js")
	attributed := "prefix;getAgentSkills:r.workspaceServices.getAgentSkills};o.attributionConfigProvider=globalThis." + attributionProviderMarker + "||{get:()=>({attribution:{attributeCommitsToAgent:!1,attributePRsToAgent:!1}})};return void 0===b?new n.Dv(o):new l(o,b);suffix"
	if err := os.WriteFile(esm, []byte(attributed), 0o644); err != nil {
		t.Fatal(err)
	}
	idx := filepath.Join(root, "node_modules", "@cursor", "sdk", "dist", "esm", "index.js")
	shifted := `prefix;function(x){return x.type==="summary"||x.type==="summary-started"||x.type==="summary-completed"}(e)||` + sdkSummaryDeltaYield + `;suffix`
	if err := os.WriteFile(idx, []byte(shifted), 0o644); err != nil {
		t.Fatal(err)
	}
	err := EnsureAttributionPatch(root)
	if !AttributionPatchUnsupported(err) {
		t.Fatalf("EnsureAttributionPatch() error = %v, want unsupported (unmatched summary onDelta rewrite)", err)
	}
	body, err := os.ReadFile(idx)
	if err != nil {
		t.Fatal(err)
	}
	if strings.Contains(string(body), summaryDeltaPatchNeedle()) {
		t.Fatal("EnsureAttributionPatch() stamped summary marker onto a still-conditional onDelta yield")
	}
}

func TestEnsureAttributionPatchUpgradesUnmarkedSummaryDelta(t *testing.T) {
	root := t.TempDir()
	writeMinimalBridgeForPatch(t, root)
	esm := filepath.Join(root, "node_modules", "@cursor", "sdk", "dist", "esm", "357.js")
	attributed := "prefix;getAgentSkills:r.workspaceServices.getAgentSkills};o.attributionConfigProvider=globalThis." + attributionProviderMarker + "||{get:()=>({attribution:{attributeCommitsToAgent:!1,attributePRsToAgent:!1}})};return void 0===b?new n.Dv(o):new l(o,b);suffix"
	if err := os.WriteFile(esm, []byte(attributed), 0o644); err != nil {
		t.Fatal(err)
	}
	idx := filepath.Join(root, "node_modules", "@cursor", "sdk", "dist", "esm", "index.js")
	unmarked := "prefix;" + sdkSummaryDeltaYield + ";suffix"
	if err := os.WriteFile(idx, []byte(unmarked), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := EnsureAttributionPatch(root); err != nil {
		t.Fatalf("EnsureAttributionPatch() error = %v, want nil (upgrade unmarked summary yield)", err)
	}
	body, err := os.ReadFile(idx)
	if err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(string(body), summaryDeltaPatchNeedle()) {
		t.Fatalf("EnsureAttributionPatch() left unmarked summary yield without marker")
	}
}

func TestEnsureAttributionPatchStillMatchesSDK1028CJS(t *testing.T) {
	root := t.TempDir()
	writeMinimalBridgeForPatch(t, root)
	cjs := filepath.Join(root, "node_modules", "@cursor", "sdk", "dist", "cjs", "616.js")
	old := "prefix;getAgentSkills:r.workspaceServices.getAgentSkills};return void 0===m?new Bw(n):new ex(n,m);suffix"
	if err := os.WriteFile(cjs, []byte(old), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := EnsureAttributionPatch(root); err != nil {
		t.Fatalf("EnsureAttributionPatch() error = %v, want nil for 1.0.28 cjs + current esm", err)
	}
	body, err := os.ReadFile(cjs)
	if err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(string(body), "attributionConfigProvider=globalThis."+attributionProviderMarker) {
		t.Fatalf("EnsureAttributionPatch() left 1.0.28 cjs unpatched")
	}
}

func TestEnsureAttributionPatchStillMatchesSDK1027CJS(t *testing.T) {
	root := t.TempDir()
	writeMinimalBridgeForPatch(t, root)
	cjs := filepath.Join(root, "node_modules", "@cursor", "sdk", "dist", "cjs", "616.js")
	old := "prefix;getAgentSkills:r.workspaceServices.getAgentSkills};return void 0===m?new $y(n):new $E(n,m);suffix"
	if err := os.WriteFile(cjs, []byte(old), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := EnsureAttributionPatch(root); err != nil {
		t.Fatalf("EnsureAttributionPatch() error = %v, want nil for 1.0.27 cjs + current esm", err)
	}
	body, err := os.ReadFile(cjs)
	if err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(string(body), "attributionConfigProvider=globalThis."+attributionProviderMarker) {
		t.Fatalf("EnsureAttributionPatch() left 1.0.27 cjs unpatched")
	}
	assertAgentSkillsStubbed(t, "cjs-1.0.27", string(body))
}

func TestEnsureAttributionPatchStillMatchesSDK1026CJS(t *testing.T) {
	root := t.TempDir()
	writeMinimalBridgeForPatch(t, root)
	cjs := filepath.Join(root, "node_modules", "@cursor", "sdk", "dist", "cjs", "616.js")
	old := "prefix;getAgentSkills:r.workspaceServices.getAgentSkills};return void 0===m?new Wv(n):new zk(n,m);suffix"
	if err := os.WriteFile(cjs, []byte(old), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := EnsureAttributionPatch(root); err != nil {
		t.Fatalf("EnsureAttributionPatch() error = %v, want nil for 1.0.26 cjs + current esm", err)
	}
	body, err := os.ReadFile(cjs)
	if err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(string(body), "attributionConfigProvider=globalThis."+attributionProviderMarker) {
		t.Fatalf("EnsureAttributionPatch() left 1.0.26 cjs unpatched")
	}
	assertAgentSkillsStubbed(t, "cjs-1.0.26", string(body))
}

func TestEnsureAttributionPatchUnsupported(t *testing.T) {
	root := t.TempDir()
	// Entrypoint present but no SDK anchors.
	entry := filepath.Join(root, attributionEntrypointRel)
	if err := os.MkdirAll(filepath.Dir(entry), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(entry, []byte("console.log(1)\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	err := EnsureAttributionPatch(root)
	if !AttributionPatchUnsupported(err) {
		t.Fatalf("err = %v, want unsupported", err)
	}
}

func writeMinimalBridgeForPatch(t *testing.T, root string) {
	t.Helper()
	entry := filepath.Join(root, attributionEntrypointRel)
	if err := os.MkdirAll(filepath.Dir(entry), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(entry, []byte("#!/usr/bin/env node\nconsole.log('bridge')\n"), 0o644); err != nil {
		t.Fatal(err)
	}

	cjs := filepath.Join(root, "node_modules", "@cursor", "sdk", "dist", "cjs", "616.js")
	esm := filepath.Join(root, "node_modules", "@cursor", "sdk", "dist", "esm", "357.js")
	for _, path := range []string{cjs, esm} {
		if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
			t.Fatal(err)
		}
	}
	cjsBody := "prefix;getAgentSkills:r.workspaceServices.getAgentSkills};return void 0===m?new Pb(n):new yx(n,m);suffix"
	esmBody := "prefix;getAgentSkills:r.workspaceServices.getAgentSkills};return void 0===b?new n.DvK(o):new l(o,b);suffix"
	if err := os.WriteFile(cjs, []byte(cjsBody), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(esm, []byte(esmBody), 0o644); err != nil {
		t.Fatal(err)
	}
	skip := sdkSummaryDeltaAnchors[0].old
	for _, dir := range []string{filepath.Dir(cjs), filepath.Dir(esm)} {
		idx := filepath.Join(dir, "index.js")
		if err := os.WriteFile(idx, []byte("prefix;"+skip+";suffix"), 0o644); err != nil {
			t.Fatal(err)
		}
	}
	server := filepath.Join(root, steerRunServerRel)
	if err := os.MkdirAll(filepath.Dir(server), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(server, []byte(steerRunMountOld+"\n"), 0o644); err != nil {
		t.Fatal(err)
	}
}

func assertAgentSkillsStubbed(t *testing.T, name, src string) {
	t.Helper()
	if strings.Contains(src, nativeAgentSkillsAssign) {
		t.Errorf("%s still assigns workspaceServices.getAgentSkills", name)
	}
	if !strings.Contains(src, stubAgentSkillsAssign) {
		t.Errorf("%s missing getAgentSkills stub", name)
	}
}

// TestAgentSkillsStubIsThenable pins the contract @cursor/sdk 1.0.28 actually
// uses: computeCachedRequestContext does t.finally() on getAgentSkills(ctx).
// A sync ()=>[] throws TypeError → Recur error_during_execution.
func TestAgentSkillsStubIsThenable(t *testing.T) {
	_, expr, ok := strings.Cut(stubAgentSkillsAssign, ":")
	if !ok {
		t.Fatalf("stubAgentSkillsAssign %q has no ':'", stubAgentSkillsAssign)
	}
	cmd := exec.Command("node", "-e", "const getAgentSkills="+expr+"; getAgentSkills().finally(()=>{})")
	if out, err := cmd.CombinedOutput(); err != nil {
		t.Fatalf("SDK calls result.finally(); stub %q is not thenable: %v\n%s", expr, err, out)
	}
}

func TestEnsureAttributionPatchStubsSkillsOnAlreadyAttributedInstall(t *testing.T) {
	root := t.TempDir()
	writeMinimalBridgeForPatch(t, root)
	if err := EnsureAttributionPatch(root); err != nil {
		t.Fatal(err)
	}
	cjs := filepath.Join(root, "node_modules", "@cursor", "sdk", "dist", "cjs", "616.js")
	// Simulate an older Recur install: attribution injected, native skills still wired.
	legacy := "prefix;" + nativeAgentSkillsAssign + "};n.attributionConfigProvider=globalThis." +
		attributionProviderMarker + "||{get:()=>({attribution:{attributeCommitsToAgent:!1,attributePRsToAgent:!1}})};return void 0===m?new Bw(n):new ex(n,m);suffix"
	if err := os.WriteFile(cjs, []byte(legacy), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := EnsureAttributionPatch(root); err != nil {
		t.Fatalf("upgrade patch: %v", err)
	}
	body, err := os.ReadFile(cjs)
	if err != nil {
		t.Fatal(err)
	}
	assertAgentSkillsStubbed(t, "legacy-cjs", string(body))
}
