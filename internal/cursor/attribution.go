package cursor

import (
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"strings"
)

// EnvAttributionPath is the absolute path of the JSON file the patched SDK
// reads from attributionConfigProvider.get() on every agent turn.
const EnvAttributionPath = "RECUR_CURSOR_ATTRIBUTION_PATH"

// AttributionConfigFile is the basename under Recur's data dir.
const AttributionConfigFile = "cursor-attribution.json"

// attributionProviderMarker is embedded in the SDK bundle and the preload so
// EnsureAttributionPatch is idempotent across reinstalls and startups.
const attributionProviderMarker = "__recurAttributionConfigProvider"

// attributionPreloadRel is where we drop the Node preload inside a bridge root.
const attributionPreloadRel = "dist/recur-attribution-provider.mjs"

// attributionEntrypointRel is the bridge's ESM entry the preload must import from.
const attributionEntrypointRel = "dist/bin/cursor-sdk-bridge.js"

// AttributionConfig is the on-disk shape read by the patched
// attributionConfigProvider.get(). Field names match Cursor's CLI config.
type AttributionConfig struct {
	AttributeCommitsToAgent bool `json:"attributeCommitsToAgent"`
	AttributePRsToAgent     bool `json:"attributePRsToAgent"`
}

// AttributionConfigPath returns <dataDir>/cursor-attribution.json.
func AttributionConfigPath(dataDir string) string {
	if dataDir == "" {
		return ""
	}
	return filepath.Join(dataDir, AttributionConfigFile)
}

// WriteAttributionConfig writes the JSON file the bridge preload reads.
func WriteAttributionConfig(path string, commits, prs bool) error {
	if path == "" {
		return fmt.Errorf("cursor attribution: empty config path")
	}
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		return fmt.Errorf("cursor attribution: mkdir: %w", err)
	}
	body, err := json.MarshalIndent(AttributionConfig{
		AttributeCommitsToAgent: commits,
		AttributePRsToAgent:     prs,
	}, "", "  ")
	if err != nil {
		return err
	}
	body = append(body, '\n')
	return writeFileAtomic(path, body, 0o644)
}

// BridgeRoot resolves the bridge install directory from a launcher path or root.
// Only returns a root when the launcher sits at <root>/bin/cursor-sdk-bridge —
// the layout Install produces. A bare test/dev launcher outside bin/ returns "".
func BridgeRoot(binOrRoot string) string {
	if binOrRoot == "" {
		return ""
	}
	if resolved := resolveCandidate(binOrRoot); resolved != "" {
		dir := filepath.Dir(resolved)
		if filepath.Base(dir) != "bin" {
			return ""
		}
		return filepath.Clean(filepath.Join(dir, ".."))
	}
	return ""
}

// OwnedBridgeRoot returns BridgeRoot(binOrRoot) only when that root is Recur's
// own <dataDir>/cursor-bridge install. Locate only returns that tree.
func OwnedBridgeRoot(dataDir, binOrRoot string) string {
	root := BridgeRoot(binOrRoot)
	if root == "" || dataDir == "" {
		return ""
	}
	owned := filepath.Clean(filepath.Join(dataDir, BridgeDirName))
	if root != owned {
		return ""
	}
	return root
}

// EnsureAttributionPatch injects Recur's attributionConfigProvider into an
// installed bridge. Safe to call repeatedly; no-ops when already patched.
//
// Cursor's public AgentOptions has no attribution fields. The local SDK leaves
// attributionConfigProvider unset, which defaults both commit and PR trailers
// ON. We own the bridge install under Recur's data dir, so on unpack (and on
// first cold start of an older install) we:
//  1. drop a small ESM preload that reads RECUR_CURSOR_ATTRIBUTION_PATH
//  2. import it from the bridge entrypoint
//  3. patch dist/server.js so Connect exposes SteerRun (SDK run.steer)
//  4. patch the SDK bundle so createLocalExecutor assigns that provider onto
//     the executor options object before the LocalExecutor constructor, so
//     getAgentSkills returns [] (Recur owns skill loading via MCP), and so
//     documented InteractionUpdate summary types reach onDelta.
func EnsureAttributionPatch(bridgeRoot string) error {
	if bridgeRoot == "" {
		return fmt.Errorf("cursor attribution patch: empty bridge root")
	}
	fi, err := os.Stat(bridgeRoot)
	if err != nil {
		return fmt.Errorf("cursor attribution patch: %w", err)
	}
	if !fi.IsDir() {
		return fmt.Errorf("cursor attribution patch: %s is not a directory", bridgeRoot)
	}

	if err := writeAttributionPreload(filepath.Join(bridgeRoot, attributionPreloadRel)); err != nil {
		return err
	}
	if err := ensureAttributionEntrypointImport(filepath.Join(bridgeRoot, attributionEntrypointRel)); err != nil {
		return err
	}
	if err := ensureSteerRunServerHook(filepath.Join(bridgeRoot, steerRunServerRel)); err != nil {
		return err
	}
	patched, err := patchAttributionSDKBundles(bridgeRoot)
	if err != nil {
		return err
	}
	if patched == 0 {
		return fmt.Errorf("%w under %s", ErrAttributionPatchUnsupported, bridgeRoot)
	}
	return nil
}

// ErrAttributionPatchUnsupported means the bridge tree has no @cursor/sdk
// bundle matching the anchors we know how to patch (wrong SDK version, or not
// a full Install tree).
var ErrAttributionPatchUnsupported = errors.New("cursor attribution patch: no SDK bundle matched known anchors")

// AttributionPatchUnsupported reports whether err is a missing-anchor failure.
func AttributionPatchUnsupported(err error) bool {
	return errors.Is(err, ErrAttributionPatchUnsupported)
}

const attributionPreloadSource = `// Recur-owned preload: supplies attributionConfigProvider for the patched SDK
// and installs Connect SteerRun (SDK run.steer) on the bridge agent service.
import { readFileSync } from "node:fs";
import { MethodKind, proto3, ScalarType } from "@bufbuild/protobuf";

function load() {
  const path = process.env.RECUR_CURSOR_ATTRIBUTION_PATH;
  const off = {
    attribution: {
      attributeCommitsToAgent: false,
      attributePRsToAgent: false,
    },
  };
  if (!path) return off;
  try {
    const j = JSON.parse(readFileSync(path, "utf8"));
    return {
      attribution: {
        attributeCommitsToAgent: !!j.attributeCommitsToAgent,
        attributePRsToAgent: !!j.attributePRsToAgent,
      },
    };
  } catch {
    return off;
  }
}

globalThis.` + attributionProviderMarker + ` = { get: load };

const SteerRunRequest = proto3.makeMessageType("sdk.v1.SteerRunRequest", [
  { no: 1, name: "run_id", kind: "scalar", T: ScalarType.STRING },
  { no: 2, name: "text", kind: "scalar", T: ScalarType.STRING },
]);
const SteerRunResponse = proto3.makeMessageType("sdk.v1.SteerRunResponse", [
  { no: 1, name: "outcome", kind: "scalar", T: ScalarType.STRING },
]);

globalThis.__recurInstallSteerRun = (SdkAgentService, impl, registry) => {
  if (!SdkAgentService?.methods || !impl || !registry) return;
  SdkAgentService.methods.steerRun = {
    name: "SteerRun",
    I: SteerRunRequest,
    O: SteerRunResponse,
    kind: MethodKind.Unary,
  };
  impl.steerRun = async (request) => {
    const runId = request?.runId ?? "";
    const text = request?.text ?? "";
    const liveRun = runId ? registry.getRun(runId) : undefined;
    if (!liveRun || typeof liveRun.steer !== "function" || !text) {
      return new SteerRunResponse({ outcome: "revert_to_followup" });
    }
    try {
      const outcome = await liveRun.steer(text);
      return new SteerRunResponse({
        outcome: outcome === "complete_delivered" ? "complete_delivered" : "revert_to_followup",
      });
    } catch {
      return new SteerRunResponse({ outcome: "revert_to_followup" });
    }
  };
};
`

func writeAttributionPreload(path string) error {
	want := []byte(attributionPreloadSource)
	if cur, err := os.ReadFile(path); err == nil && string(cur) == string(want) {
		return nil
	}
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		return fmt.Errorf("cursor attribution patch: preload mkdir: %w", err)
	}
	if err := writeFileAtomic(path, want, 0o644); err != nil {
		return fmt.Errorf("cursor attribution patch: preload write: %w", err)
	}
	return nil
}

const attributionEntrypointImport = `import "../recur-attribution-provider.mjs";` + "\n"

func ensureAttributionEntrypointImport(path string) error {
	body, err := os.ReadFile(path)
	if err != nil {
		return fmt.Errorf("cursor attribution patch: read entrypoint: %w", err)
	}
	src := string(body)
	if strings.Contains(src, "recur-attribution-provider.mjs") {
		return nil
	}
	// Keep the shebang first when present.
	var out string
	if strings.HasPrefix(src, "#!") {
		if nl := strings.IndexByte(src, '\n'); nl >= 0 {
			out = src[:nl+1] + attributionEntrypointImport + src[nl+1:]
		} else {
			out = src + "\n" + attributionEntrypointImport
		}
	} else {
		out = attributionEntrypointImport + src
	}
	if err := writeFileAtomic(path, []byte(out), 0o644); err != nil {
		return fmt.Errorf("cursor attribution patch: write entrypoint: %w", err)
	}
	return nil
}

const steerRunServerRel = "dist/server.js"

const steerRunMountOld = `function mountSdkBridgeRoutes(router, options) {
    router.service(SdkAgentService, createSdkAgentService({
        registry: options.registry,
        workspaceRef: options.workspaceRef,
    }));`

const steerRunMountNew = `function mountSdkBridgeRoutes(router, options) {
    const impl = createSdkAgentService({
        registry: options.registry,
        workspaceRef: options.workspaceRef,
    });
    globalThis.__recurInstallSteerRun?.(SdkAgentService, impl, options.registry);
    router.service(SdkAgentService, impl);`

func ensureSteerRunServerHook(path string) error {
	body, err := os.ReadFile(path)
	if err != nil {
		if os.IsNotExist(err) {
			return nil
		}
		return fmt.Errorf("cursor steer patch: read server: %w", err)
	}
	src := string(body)
	if strings.Contains(src, "__recurInstallSteerRun") {
		return nil
	}
	if !strings.Contains(src, steerRunMountOld) {
		return fmt.Errorf("unmatched steer hook in %s: %w", path, ErrAttributionPatchUnsupported)
	}
	src = strings.Replace(src, steerRunMountOld, steerRunMountNew, 1)
	if err := writeFileAtomic(path, []byte(src), 0o644); err != nil {
		return fmt.Errorf("cursor steer patch: write server: %w", err)
	}
	return nil
}

// sdkAttributionAnchors are (needle, replacement) pairs for the minified
// createLocalExecutor sites in @cursor/sdk 1.0.x (cjs + esm chunk layouts).
var sdkAttributionAnchors = []struct {
	old string
	neu string
}{
	{
		// cjs 748.js (@cursor/sdk 1.0.38): same const-chain shape as 1.0.37
		old: "getAgentSkills:r.workspaceServices.getAgentSkills,...i},u=void 0===a?new JE(l):new NP(l,a);",
		neu: "getAgentSkills:r.workspaceServices.getAgentSkills,...i},u=(l.attributionConfigProvider=globalThis." + attributionProviderMarker + "||{get:()=>({attribution:{attributeCommitsToAgent:!1,attributePRsToAgent:!1}})},void 0===a?new JE(l):new NP(l,a));",
	},
	{
		// esm 748.js (@cursor/sdk 1.0.38): same shape as cjs
		old: "getAgentSkills:r.workspaceServices.getAgentSkills,...i},u=void 0===a?new GE(l):new DP(l,a);",
		neu: "getAgentSkills:r.workspaceServices.getAgentSkills,...i},u=(l.attributionConfigProvider=globalThis." + attributionProviderMarker + "||{get:()=>({attribution:{attributeCommitsToAgent:!1,attributePRsToAgent:!1}})},void 0===a?new GE(l):new DP(l,a));",
	},
	{
		// cjs 464.js (@cursor/sdk 1.0.37): options object `l` sits in a const
		// chain, so inject via a comma expression on the executor assignment.
		old: "getAgentSkills:r.workspaceServices.getAgentSkills,...i},u=void 0===a?new OS(l):new dx(l,a);",
		neu: "getAgentSkills:r.workspaceServices.getAgentSkills,...i},u=(l.attributionConfigProvider=globalThis." + attributionProviderMarker + "||{get:()=>({attribution:{attributeCommitsToAgent:!1,attributePRsToAgent:!1}})},void 0===a?new OS(l):new dx(l,a));",
	},
	{
		// esm 464.js (@cursor/sdk 1.0.37): same shape as cjs
		old: "getAgentSkills:r.workspaceServices.getAgentSkills,...i},u=void 0===a?new MS(l):new lx(l,a);",
		neu: "getAgentSkills:r.workspaceServices.getAgentSkills,...i},u=(l.attributionConfigProvider=globalThis." + attributionProviderMarker + "||{get:()=>({attribution:{attributeCommitsToAgent:!1,attributePRsToAgent:!1}})},void 0===a?new MS(l):new lx(l,a));",
	},
	{
		// cjs (@cursor/sdk 1.0.31 chunk) — options object is `n`
		old: "getAgentSkills:r.workspaceServices.getAgentSkills};return void 0===m?new Pb(n):new yx(n,m)",
		neu: "getAgentSkills:r.workspaceServices.getAgentSkills};n.attributionConfigProvider=globalThis." + attributionProviderMarker + "||{get:()=>({attribution:{attributeCommitsToAgent:!1,attributePRsToAgent:!1}})};return void 0===m?new Pb(n):new yx(n,m)",
	},
	{
		// cjs (@cursor/sdk 1.0.30 chunk) — options object is `n`
		old: "getAgentSkills:r.workspaceServices.getAgentSkills};return void 0===m?new Yw(n):new BE(n,m)",
		neu: "getAgentSkills:r.workspaceServices.getAgentSkills};n.attributionConfigProvider=globalThis." + attributionProviderMarker + "||{get:()=>({attribution:{attributeCommitsToAgent:!1,attributePRsToAgent:!1}})};return void 0===m?new Yw(n):new BE(n,m)",
	},
	{
		// cjs (@cursor/sdk 1.0.28 chunk) — options object is `n`
		old: "getAgentSkills:r.workspaceServices.getAgentSkills};return void 0===m?new Bw(n):new ex(n,m)",
		neu: "getAgentSkills:r.workspaceServices.getAgentSkills};n.attributionConfigProvider=globalThis." + attributionProviderMarker + "||{get:()=>({attribution:{attributeCommitsToAgent:!1,attributePRsToAgent:!1}})};return void 0===m?new Bw(n):new ex(n,m)",
	},
	{
		// cjs (@cursor/sdk 1.0.27 chunk) — kept so an unpatched older Recur
		// install still patches cjs.
		old: "getAgentSkills:r.workspaceServices.getAgentSkills};return void 0===m?new $y(n):new $E(n,m)",
		neu: "getAgentSkills:r.workspaceServices.getAgentSkills};n.attributionConfigProvider=globalThis." + attributionProviderMarker + "||{get:()=>({attribution:{attributeCommitsToAgent:!1,attributePRsToAgent:!1}})};return void 0===m?new $y(n):new $E(n,m)",
	},
	{
		// cjs (@cursor/sdk 1.0.26 chunk) — options object is `n`
		old: "getAgentSkills:r.workspaceServices.getAgentSkills};return void 0===m?new Wv(n):new zk(n,m)",
		neu: "getAgentSkills:r.workspaceServices.getAgentSkills};n.attributionConfigProvider=globalThis." + attributionProviderMarker + "||{get:()=>({attribution:{attributeCommitsToAgent:!1,attributePRsToAgent:!1}})};return void 0===m?new Wv(n):new zk(n,m)",
	},
	{
		// esm 357.js (@cursor/sdk 1.0.31) — options object is `o`
		old: "getAgentSkills:r.workspaceServices.getAgentSkills};return void 0===b?new n.DvK(o):new l(o,b)",
		neu: "getAgentSkills:r.workspaceServices.getAgentSkills};o.attributionConfigProvider=globalThis." + attributionProviderMarker + "||{get:()=>({attribution:{attributeCommitsToAgent:!1,attributePRsToAgent:!1}})};return void 0===b?new n.DvK(o):new l(o,b)",
	},
	{
		// esm 357.js — options object is `o` (stable across 1.0.26–1.0.30)
		old: "getAgentSkills:r.workspaceServices.getAgentSkills};return void 0===b?new n.Dv(o):new l(o,b)",
		neu: "getAgentSkills:r.workspaceServices.getAgentSkills};o.attributionConfigProvider=globalThis." + attributionProviderMarker + "||{get:()=>({attribution:{attributeCommitsToAgent:!1,attributePRsToAgent:!1}})};return void 0===b?new n.Dv(o):new l(o,b)",
	},
}

// patchAttributionSDKBundles patches every present cjs/esm bundle. The bridge
// loads ESM, so an unmatched esm tree is unsupported (Cursor's default
// attribution would stay ON). A cjs-only miss is ignored — that copy is never
// required at runtime.
func patchAttributionSDKBundles(bridgeRoot string) (int, error) {
	cjsRel := filepath.Join("node_modules", "@cursor", "sdk", "dist", "cjs")
	esmRel := filepath.Join("node_modules", "@cursor", "sdk", "dist", "esm")

	cjsN, _, err := patchAttributionFormat(bridgeRoot, cjsRel)
	if err != nil {
		return 0, err
	}
	esmN, esmPresent, err := patchAttributionFormat(bridgeRoot, esmRel)
	if err != nil {
		return cjsN, err
	}
	patched := cjsN + esmN
	// Count includes the summary-onDelta rewrite in index.js. Attribution still
	// has to land in the ESM tree — a summary-only hit must not hide a missed
	// createLocalExecutor site (Cursor's default trailers would stay ON).
	if esmPresent && !sdkTreeHasAttribution(bridgeRoot, esmRel) {
		return patched, fmt.Errorf("unmatched %s under %s: %w", esmRel, bridgeRoot, ErrAttributionPatchUnsupported)
	}
	// Same for the summary rewrite: an already-attribution-patched bridge (or
	// a minifier-shifted skip we did not match) must not look successful while
	// onDelta still drops summary / summary-started / summary-completed.
	if esmPresent && !sdkTreeHasSummaryDelta(bridgeRoot, esmRel) {
		return patched, fmt.Errorf("unmatched summary onDelta rewrite in %s under %s: %w", esmRel, bridgeRoot, ErrAttributionPatchUnsupported)
	}
	return patched, nil
}

func sdkTreeHasAttribution(bridgeRoot, rel string) bool {
	dir := filepath.Join(bridgeRoot, rel)
	entries, err := os.ReadDir(dir)
	if err != nil {
		return false
	}
	needle := "attributionConfigProvider=globalThis." + attributionProviderMarker
	for _, e := range entries {
		if e.IsDir() || !strings.HasSuffix(e.Name(), ".js") {
			continue
		}
		b, err := os.ReadFile(filepath.Join(dir, e.Name()))
		if err != nil {
			continue
		}
		if strings.Contains(string(b), needle) {
			return true
		}
	}
	return false
}

// summaryDeltaMarker is injected into the onDelta yield so install can verify
// the rewrite landed — absence of the old skip text is not enough when the
// minifier renames the event variable.
const summaryDeltaMarker = "recurSummaryDelta"

const sdkSummaryDeltaYield = `(yield null===(n=(t=this.options).onDelta)||void 0===n?void 0:n.call(t,{update:e}))`

func summaryDeltaPatchNeedle() string {
	return "/*" + summaryDeltaMarker + "*/"
}

// sdkTreeHasSummaryDelta is true when Recur's summary-onDelta marker is in
// the ESM tree. The rewrite inserts this comment; a missing marker means
// summaries are still suppressed (or the skip was never this SDK's shape).
func sdkTreeHasSummaryDelta(bridgeRoot, rel string) bool {
	dir := filepath.Join(bridgeRoot, rel)
	entries, err := os.ReadDir(dir)
	if err != nil {
		return false
	}
	needle := summaryDeltaPatchNeedle()
	for _, e := range entries {
		if e.IsDir() || !strings.HasSuffix(e.Name(), ".js") {
			continue
		}
		b, err := os.ReadFile(filepath.Join(dir, e.Name()))
		if err != nil {
			continue
		}
		if strings.Contains(string(b), needle) {
			return true
		}
	}
	return false
}

// sdkSummaryDeltaAnchors restore the documented onDelta contract.
// InteractionUpdate (the type passed to agent.send's onDelta) includes
// SummaryUpdate, SummaryStartedUpdate, and SummaryCompletedUpdate.
// cursor-sdk 1.0.28–1.0.38's RunInteractionAccumulator.apply still skips
// those three types, so compact cards and session_context never see them.
var sdkSummaryDeltaAnchors = []struct {
	old string
	neu string
}{
	{
		// 1.0.37: async/await instead of the generator yield
		old: `function(t){return"summary"===t.type||"summary-started"===t.type||"summary-completed"===t.type}(t)||await(this.options.onDelta?.({update:t}))`,
		neu: summaryDeltaPatchNeedle() + `await(this.options.onDelta?.({update:t}))`,
	},
	{
		old: `function(e){return"summary"===e.type||"summary-started"===e.type||"summary-completed"===e.type}(e)||` + sdkSummaryDeltaYield,
		neu: summaryDeltaPatchNeedle() + sdkSummaryDeltaYield,
	},
}

func patchAttributionFormat(bridgeRoot, rel string) (patched int, present bool, err error) {
	dir := filepath.Join(bridgeRoot, rel)
	entries, err := os.ReadDir(dir)
	if err != nil {
		if os.IsNotExist(err) {
			return 0, false, nil
		}
		return 0, false, fmt.Errorf("cursor attribution patch: read %s: %w", rel, err)
	}
	for _, e := range entries {
		if e.IsDir() || !strings.HasSuffix(e.Name(), ".js") {
			continue
		}
		present = true
		n, err := patchAttributionFile(filepath.Join(dir, e.Name()))
		if err != nil {
			return patched, present, err
		}
		patched += n
	}
	return patched, present, nil
}

// nativeAgentSkillsAssign is the minified createLocalExecutor field that wires
// Cursor's filesystem skill loader. Recur owns skills via MCP, so we replace
// it with an empty stub after the attribution inject (same site, D-skill-loader).
//
// The stub must be async: SDK 1.0.28 computeCachedRequestContext does
// t.finally() on getAgentSkills(ctx). A sync ()=>[] throws TypeError
// (error_during_execution).
const (
	nativeAgentSkillsAssign = "getAgentSkills:r.workspaceServices.getAgentSkills"
	stubAgentSkillsAssign   = "getAgentSkills:async()=>[]"
)

func patchAttributionFile(path string) (int, error) {
	body, err := os.ReadFile(path)
	if err != nil {
		return 0, fmt.Errorf("cursor attribution patch: read %s: %w", path, err)
	}
	src := string(body)
	orig := src
	src = applySDKAnchors(src)
	hasAttrib := strings.Contains(src, "attributionConfigProvider=globalThis."+attributionProviderMarker)
	if hasAttrib && strings.Contains(src, nativeAgentSkillsAssign) {
		src = strings.ReplaceAll(src, nativeAgentSkillsAssign, stubAgentSkillsAssign)
	}
	if src != orig {
		if err := writeFileAtomic(path, []byte(src), 0o644); err != nil {
			return 0, fmt.Errorf("cursor attribution patch: write %s: %w", path, err)
		}
	}
	if hasAttrib || src != orig {
		return 1, nil
	}
	return 0, nil
}

func applySDKAnchors(src string) string {
	for _, a := range sdkAttributionAnchors {
		src = strings.Replace(src, a.old, a.neu, 1)
	}
	for _, a := range sdkSummaryDeltaAnchors {
		src = strings.Replace(src, a.old, a.neu, 1)
	}
	// Prior unmarked rewrite stripped the skip and left a bare yield. Stamp
	// the marker only when that yield is no longer behind `||` — a still-
	// conditional skip (any operand order) keeps `||(yield…)` and must fail.
	if !strings.Contains(src, summaryDeltaMarker) && !strings.Contains(src, "||"+sdkSummaryDeltaYield) {
		src = strings.Replace(src, sdkSummaryDeltaYield, summaryDeltaPatchNeedle()+sdkSummaryDeltaYield, 1)
	}
	return src
}

// writeFileAtomic writes path via a same-directory temp file + rename so a
// crash mid-write cannot leave a truncated SDK bundle or entrypoint.
func writeFileAtomic(path string, body []byte, mode os.FileMode) error {
	dir := filepath.Dir(path)
	tmp, err := os.CreateTemp(dir, "."+filepath.Base(path)+".tmp-*")
	if err != nil {
		return err
	}
	tmpPath := tmp.Name()
	cleanup := true
	defer func() {
		if cleanup {
			_ = os.Remove(tmpPath)
		}
	}()
	if _, err := tmp.Write(body); err != nil {
		_ = tmp.Close()
		return err
	}
	if err := tmp.Chmod(mode); err != nil {
		_ = tmp.Close()
		return err
	}
	if err := tmp.Close(); err != nil {
		return err
	}
	if err := os.Rename(tmpPath, path); err != nil {
		return err
	}
	cleanup = false
	return nil
}
