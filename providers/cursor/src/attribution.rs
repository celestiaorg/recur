//! Recur's patch of the installed SDK bundle.
//!
//! The local SDK leaves `attributionConfigProvider` unset, which defaults
//! commit and PR trailers ON, and its public options have no attribution
//! fields. Recur owns the bridge install, so on unpack (and on cold start of
//! an older install) it:
//!
//! 1. drops an ESM preload that reads RECUR_CURSOR_ATTRIBUTION_PATH,
//! 2. imports it from the bridge entrypoint,
//! 3. patches `dist/server.js` so Connect exposes SteerRun (SDK run.steer),
//! 4. patches the SDK bundle so createLocalExecutor assigns that provider
//!    before constructing the executor, stubs getAgentSkills to `[]` (Recur
//!    owns skills via MCP), and forwards the documented summary
//!    InteractionUpdates to onDelta.
//!
//! The minify anchors match [`crate::install::PINNED_SDK_VERSION`]: bump both
//! together (`.agents/skills/repin-cursor-sdk/SKILL.md`).

use std::path::{Path, PathBuf};

/// The basename of the attribution config under Recur's data dir.
pub const ATTRIBUTION_CONFIG_FILE: &str = "cursor-attribution.json";

/// Embedded in the SDK bundle and the preload so the patch is idempotent.
pub(crate) const ATTRIBUTION_PROVIDER_MARKER: &str = "__recurAttributionConfigProvider";

pub(crate) const ATTRIBUTION_PRELOAD_REL: &str = "dist/recur-attribution-provider.mjs";

/// The bridge's ESM entry the preload must be imported from.
pub(crate) const ATTRIBUTION_ENTRYPOINT_REL: &str = "dist/bin/cursor-sdk-bridge.js";

#[derive(Debug, thiserror::Error)]
pub enum PatchError {
    /// No SDK bundle matched the known anchors (wrong SDK version, or not a
    /// full Install tree).
    #[error("{0}")]
    Unsupported(String),
    #[error("{0}")]
    Io(String),
}

impl PatchError {
    pub fn unsupported(&self) -> bool {
        matches!(self, Self::Unsupported(_))
    }
}

const UNSUPPORTED: &str = "cursor attribution patch: no SDK bundle matched known anchors";

/// `<dataDir>/cursor-attribution.json`; `None` without a data dir.
pub fn attribution_config_path(data_dir: &Path) -> Option<PathBuf> {
    (!data_dir.as_os_str().is_empty()).then(|| data_dir.join(ATTRIBUTION_CONFIG_FILE))
}

/// Writes the file the patched provider reads on every turn, as
/// two-space indented JSON plus a newline.
pub fn write_attribution_config(path: &Path, commits: bool, prs: bool) -> Result<(), String> {
    if path.as_os_str().is_empty() {
        return Err("cursor attribution: empty config path".into());
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("cursor attribution: mkdir: {e}"))?;
    }
    let body = format!(
        "{{\n  \"attributeCommitsToAgent\": {commits},\n  \"attributePRsToAgent\": {prs}\n}}\n"
    );
    write_file_atomic(path, body.as_bytes(), 0o644).map_err(|e| e.to_string())
}

/// Injects Recur's provider into an installed bridge. Safe to repeat.
pub fn ensure_attribution_patch(bridge_root: &Path) -> Result<(), PatchError> {
    if bridge_root.as_os_str().is_empty() {
        return Err(PatchError::Io(
            "cursor attribution patch: empty bridge root".into(),
        ));
    }
    let meta = std::fs::metadata(bridge_root)
        .map_err(|e| PatchError::Io(format!("cursor attribution patch: {e}")))?;
    if !meta.is_dir() {
        return Err(PatchError::Io(format!(
            "cursor attribution patch: {} is not a directory",
            bridge_root.display()
        )));
    }
    write_attribution_preload(&bridge_root.join(ATTRIBUTION_PRELOAD_REL))?;
    ensure_attribution_entrypoint_import(&bridge_root.join(ATTRIBUTION_ENTRYPOINT_REL))?;
    ensure_steer_run_server_hook(&bridge_root.join(STEER_RUN_SERVER_REL))?;
    if patch_attribution_sdk_bundles(bridge_root)? == 0 {
        return Err(PatchError::Unsupported(format!(
            "{UNSUPPORTED} under {}",
            bridge_root.display()
        )));
    }
    Ok(())
}

pub(crate) const ATTRIBUTION_PRELOAD_SOURCE: &str = r#"// Recur-owned preload: supplies attributionConfigProvider for the patched SDK
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

globalThis.__recurAttributionConfigProvider = { get: load };

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
"#;

fn write_attribution_preload(path: &Path) -> Result<(), PatchError> {
    let want = ATTRIBUTION_PRELOAD_SOURCE.as_bytes();
    if std::fs::read(path).is_ok_and(|cur| cur == want) {
        return Ok(());
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| PatchError::Io(format!("cursor attribution patch: preload mkdir: {e}")))?;
    }
    write_file_atomic(path, want, 0o644)
        .map_err(|e| PatchError::Io(format!("cursor attribution patch: preload write: {e}")))
}

pub(crate) const ATTRIBUTION_ENTRYPOINT_IMPORT: &str =
    "import \"../recur-attribution-provider.mjs\";\n";

fn ensure_attribution_entrypoint_import(path: &Path) -> Result<(), PatchError> {
    let src = std::fs::read_to_string(path)
        .map_err(|e| PatchError::Io(format!("cursor attribution patch: read entrypoint: {e}")))?;
    if src.contains("recur-attribution-provider.mjs") {
        return Ok(());
    }
    // Keep the shebang first when present.
    let out = if src.starts_with("#!") {
        match src.find('\n') {
            Some(nl) => format!(
                "{}{ATTRIBUTION_ENTRYPOINT_IMPORT}{}",
                &src[..=nl],
                &src[nl + 1..]
            ),
            None => format!("{src}\n{ATTRIBUTION_ENTRYPOINT_IMPORT}"),
        }
    } else {
        format!("{ATTRIBUTION_ENTRYPOINT_IMPORT}{src}")
    };
    write_file_atomic(path, out.as_bytes(), 0o644)
        .map_err(|e| PatchError::Io(format!("cursor attribution patch: write entrypoint: {e}")))
}

pub(crate) const STEER_RUN_SERVER_REL: &str = "dist/server.js";

pub(crate) const STEER_RUN_MOUNT_OLD: &str = "function mountSdkBridgeRoutes(router, options) {
    router.service(SdkAgentService, createSdkAgentService({
        registry: options.registry,
        workspaceRef: options.workspaceRef,
    }));";

pub(crate) const STEER_RUN_MOUNT_NEW: &str = "function mountSdkBridgeRoutes(router, options) {
    const impl = createSdkAgentService({
        registry: options.registry,
        workspaceRef: options.workspaceRef,
    });
    globalThis.__recurInstallSteerRun?.(SdkAgentService, impl, options.registry);
    router.service(SdkAgentService, impl);";

fn ensure_steer_run_server_hook(path: &Path) -> Result<(), PatchError> {
    let src = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => {
            return Err(PatchError::Io(format!(
                "cursor steer patch: read server: {e}"
            )));
        }
    };
    if src.contains("__recurInstallSteerRun") {
        return Ok(());
    }
    if !src.contains(STEER_RUN_MOUNT_OLD) {
        return Err(PatchError::Unsupported(format!(
            "unmatched steer hook in {}: {UNSUPPORTED}",
            path.display()
        )));
    }
    let src = src.replacen(STEER_RUN_MOUNT_OLD, STEER_RUN_MOUNT_NEW, 1);
    write_file_atomic(path, src.as_bytes(), 0o644)
        .map_err(|e| PatchError::Io(format!("cursor steer patch: write server: {e}")))
}

const ATTRIB_FALLBACK: &str =
    "||{get:()=>({attribution:{attributeCommitsToAgent:!1,attributePRsToAgent:!1}})}";

/// One `(needle, replacement)` pair.
pub(crate) struct Anchor {
    pub old: &'static str,
    pub neu: String,
}

/// The minified createLocalExecutor sites in @cursor/sdk 1.x (cjs + esm
/// chunk layouts), newest first.
pub(crate) fn sdk_attribution_anchors() -> Vec<Anchor> {
    let m = ATTRIBUTION_PROVIDER_MARKER;
    // `{fields}...i},{var}={exec};` becomes `{fields}...i},{var}=(inject,{exec});`.
    let chain = |old: &'static str, opt: &str| {
        let (head, tail) = old.split_once("...i},").expect("chain anchor shape");
        let (var, exec) = tail.split_once('=').expect("chain anchor shape");
        let exec = exec.strip_suffix(';').expect("chain anchor shape");
        Anchor {
            old,
            neu: format!(
                "{head}...i}},{var}=({opt}.attributionConfigProvider=globalThis.{m}{ATTRIB_FALLBACK},{exec});"
            ),
        }
    };
    let stmt = |old: &'static str, opt: &str, ret: &str| Anchor {
        old,
        neu: format!(
            "getAgentSkills:r.workspaceServices.getAgentSkills}};{opt}.attributionConfigProvider=globalThis.{m}{ATTRIB_FALLBACK};{ret}"
        ),
    };
    vec![
        // cjs 305.js (@cursor/sdk 1.3.1): two powershell fields follow
        // getAgentSkills, and the executor is `d` over options `u`.
        chain(
            "getAgentSkills:r.workspaceServices.getAgentSkills,powershellUtf8Console:a.powershellUtf8Console,powershellUtf8ChildEnv:a.powershellUtf8ChildEnv,...i},d=void 0===c?new CA(u):new PO(u,c);",
            "u",
        ),
        // esm 305.js (1.3.1).
        chain(
            "getAgentSkills:r.workspaceServices.getAgentSkills,powershellUtf8Console:a.powershellUtf8Console,powershellUtf8ChildEnv:a.powershellUtf8ChildEnv,...i},d=void 0===c?new SA(u):new CO(u,c);",
            "u",
        ),
        // cjs 748.js (@cursor/sdk 1.0.38): same const-chain shape as 1.0.37.
        chain(
            "getAgentSkills:r.workspaceServices.getAgentSkills,...i},u=void 0===a?new JE(l):new NP(l,a);",
            "l",
        ),
        // esm 748.js (1.0.38).
        chain(
            "getAgentSkills:r.workspaceServices.getAgentSkills,...i},u=void 0===a?new GE(l):new DP(l,a);",
            "l",
        ),
        // cjs 464.js (1.0.37): the options object sits in a const chain, so
        // the inject is a comma expression on the executor assignment.
        chain(
            "getAgentSkills:r.workspaceServices.getAgentSkills,...i},u=void 0===a?new OS(l):new dx(l,a);",
            "l",
        ),
        // esm 464.js (1.0.37).
        chain(
            "getAgentSkills:r.workspaceServices.getAgentSkills,...i},u=void 0===a?new MS(l):new lx(l,a);",
            "l",
        ),
        // cjs (1.0.31).
        stmt(
            "getAgentSkills:r.workspaceServices.getAgentSkills};return void 0===m?new Pb(n):new yx(n,m)",
            "n",
            "return void 0===m?new Pb(n):new yx(n,m)",
        ),
        // cjs (1.0.30).
        stmt(
            "getAgentSkills:r.workspaceServices.getAgentSkills};return void 0===m?new Yw(n):new BE(n,m)",
            "n",
            "return void 0===m?new Yw(n):new BE(n,m)",
        ),
        // cjs (1.0.28).
        stmt(
            "getAgentSkills:r.workspaceServices.getAgentSkills};return void 0===m?new Bw(n):new ex(n,m)",
            "n",
            "return void 0===m?new Bw(n):new ex(n,m)",
        ),
        // cjs (1.0.27): an unpatched older install still patches.
        stmt(
            "getAgentSkills:r.workspaceServices.getAgentSkills};return void 0===m?new $y(n):new $E(n,m)",
            "n",
            "return void 0===m?new $y(n):new $E(n,m)",
        ),
        // cjs (1.0.26).
        stmt(
            "getAgentSkills:r.workspaceServices.getAgentSkills};return void 0===m?new Wv(n):new zk(n,m)",
            "n",
            "return void 0===m?new Wv(n):new zk(n,m)",
        ),
        // esm 357.js (1.0.31): options object `o`.
        stmt(
            "getAgentSkills:r.workspaceServices.getAgentSkills};return void 0===b?new n.DvK(o):new l(o,b)",
            "o",
            "return void 0===b?new n.DvK(o):new l(o,b)",
        ),
        // esm 357.js (stable across 1.0.26–1.0.30).
        stmt(
            "getAgentSkills:r.workspaceServices.getAgentSkills};return void 0===b?new n.Dv(o):new l(o,b)",
            "o",
            "return void 0===b?new n.Dv(o):new l(o,b)",
        ),
    ]
}

/// Patches every present cjs/esm bundle. The bridge loads ESM, so an
/// unmatched esm tree is unsupported (Cursor's default trailers would stay
/// on); a cjs-only miss is ignored.
fn patch_attribution_sdk_bundles(bridge_root: &Path) -> Result<usize, PatchError> {
    let cjs_rel = Path::new("node_modules/@cursor/sdk/dist/cjs");
    let esm_rel = Path::new("node_modules/@cursor/sdk/dist/esm");
    let (cjs_n, _) = patch_attribution_format(bridge_root, cjs_rel)?;
    let (esm_n, esm_present) = patch_attribution_format(bridge_root, esm_rel)?;
    let patched = cjs_n + esm_n;
    // The count includes the summary rewrite in index.js; attribution must
    // still land in the ESM tree.
    if esm_present && !sdk_tree_contains(bridge_root, esm_rel, &attribution_needle()) {
        return Err(PatchError::Unsupported(format!(
            "unmatched {} under {}: {UNSUPPORTED}",
            esm_rel.display(),
            bridge_root.display()
        )));
    }
    // An attribution-patched bridge (or a shifted skip) must not look
    // successful while onDelta still drops the summary updates.
    if esm_present && !sdk_tree_contains(bridge_root, esm_rel, &summary_delta_patch_needle()) {
        return Err(PatchError::Unsupported(format!(
            "unmatched summary onDelta rewrite in {} under {}: {UNSUPPORTED}",
            esm_rel.display(),
            bridge_root.display()
        )));
    }
    Ok(patched)
}

fn attribution_needle() -> String {
    format!("attributionConfigProvider=globalThis.{ATTRIBUTION_PROVIDER_MARKER}")
}

fn js_files(dir: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir)? {
        let e = e?;
        if e.file_type().is_ok_and(|t| !t.is_dir())
            && e.file_name().to_string_lossy().ends_with(".js")
        {
            out.push(e.path());
        }
    }
    out.sort();
    Ok(out)
}

fn sdk_tree_contains(bridge_root: &Path, rel: &Path, needle: &str) -> bool {
    js_files(&bridge_root.join(rel)).is_ok_and(|files| {
        files
            .iter()
            .any(|f| std::fs::read(f).is_ok_and(|b| String::from_utf8_lossy(&b).contains(needle)))
    })
}

/// Injected into the onDelta yield so install can verify the rewrite landed:
/// a missing skip is not enough when the minifier renames the variable.
pub(crate) const SUMMARY_DELTA_MARKER: &str = "recurSummaryDelta";

pub(crate) const SDK_SUMMARY_DELTA_YIELD: &str =
    "(yield null===(n=(t=this.options).onDelta)||void 0===n?void 0:n.call(t,{update:e}))";

pub(crate) fn summary_delta_patch_needle() -> String {
    format!("/*{SUMMARY_DELTA_MARKER}*/")
}

/// Restores the documented onDelta contract: cursor-sdk 1.0.28–1.3.1's
/// RunInteractionAccumulator.apply skips summary, summary-started and
/// summary-completed.
pub(crate) fn sdk_summary_delta_anchors() -> Vec<Anchor> {
    let needle = summary_delta_patch_needle();
    vec![
        // 1.0.37: async/await instead of the generator yield.
        Anchor {
            old: r#"function(t){return"summary"===t.type||"summary-started"===t.type||"summary-completed"===t.type}(t)||await(this.options.onDelta?.({update:t}))"#,
            neu: format!("{needle}await(this.options.onDelta?.({{update:t}}))"),
        },
        Anchor {
            old: concat!(
                r#"function(e){return"summary"===e.type||"summary-started"===e.type||"summary-completed"===e.type}(e)||"#,
                "(yield null===(n=(t=this.options).onDelta)||void 0===n?void 0:n.call(t,{update:e}))"
            ),
            neu: format!("{needle}{SDK_SUMMARY_DELTA_YIELD}"),
        },
    ]
}

fn patch_attribution_format(bridge_root: &Path, rel: &Path) -> Result<(usize, bool), PatchError> {
    let files = match js_files(&bridge_root.join(rel)) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok((0, false)),
        Err(e) => {
            return Err(PatchError::Io(format!(
                "cursor attribution patch: read {}: {e}",
                rel.display()
            )));
        }
    };
    let mut patched = 0;
    let present = !files.is_empty();
    for f in files {
        patched += patch_attribution_file(&f)?;
    }
    Ok((patched, present))
}

/// The minified field that wires Cursor's filesystem skill loader. Recur owns
/// skills via MCP, so after the attribution inject it becomes an async stub:
/// SDK 1.0.28 does `.finally()` on its result, and a sync `()=>[]` throws.
pub(crate) const NATIVE_AGENT_SKILLS_ASSIGN: &str =
    "getAgentSkills:r.workspaceServices.getAgentSkills";
pub(crate) const STUB_AGENT_SKILLS_ASSIGN: &str = "getAgentSkills:async()=>[]";

fn patch_attribution_file(path: &Path) -> Result<usize, PatchError> {
    let body = std::fs::read(path).map_err(|e| {
        PatchError::Io(format!(
            "cursor attribution patch: read {}: {e}",
            path.display()
        ))
    })?;
    let orig = String::from_utf8_lossy(&body).into_owned();
    let mut src = apply_sdk_anchors(&orig);
    let has_attrib = src.contains(&attribution_needle());
    if has_attrib && src.contains(NATIVE_AGENT_SKILLS_ASSIGN) {
        src = src.replace(NATIVE_AGENT_SKILLS_ASSIGN, STUB_AGENT_SKILLS_ASSIGN);
    }
    if src != orig {
        write_file_atomic(path, src.as_bytes(), 0o644).map_err(|e| {
            PatchError::Io(format!(
                "cursor attribution patch: write {}: {e}",
                path.display()
            ))
        })?;
    }
    Ok(usize::from(has_attrib || src != orig))
}

fn apply_sdk_anchors(src: &str) -> String {
    let mut src = src.to_owned();
    for a in sdk_attribution_anchors()
        .iter()
        .chain(&sdk_summary_delta_anchors())
    {
        src = src.replacen(a.old, &a.neu, 1);
    }
    // A prior unmarked rewrite stripped the skip and left a bare yield. Stamp
    // the marker only when the yield is no longer behind `||`: a still
    // conditional skip must fail.
    if !src.contains(SUMMARY_DELTA_MARKER) && !src.contains(&format!("||{SDK_SUMMARY_DELTA_YIELD}"))
    {
        src = src.replacen(
            SDK_SUMMARY_DELTA_YIELD,
            &format!("{}{SDK_SUMMARY_DELTA_YIELD}", summary_delta_patch_needle()),
            1,
        );
    }
    src
}

/// Writes through a same-directory temp file and rename, so a crash cannot
/// leave a truncated bundle or entrypoint.
pub(crate) fn write_file_atomic(path: &Path, body: &[u8], mode: u32) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;
    let dir = path.parent().unwrap_or(Path::new("."));
    let base = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut tmp = tempfile::Builder::new()
        .prefix(&format!(".{base}.tmp-"))
        .tempfile_in(dir)?;
    tmp.write_all(body)?;
    tmp.as_file()
        .set_permissions(std::fs::Permissions::from_mode(mode))?;
    tmp.persist(path).map_err(|e| e.error)?;
    Ok(())
}

#[cfg(test)]
mod tests;
