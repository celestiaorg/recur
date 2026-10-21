//! Go `cursor/attribution_test.go`.

use std::path::Path;

use super::*;

const CJS: &str = "node_modules/@cursor/sdk/dist/cjs/616.js";
const ESM: &str = "node_modules/@cursor/sdk/dist/esm/357.js";

fn summary_skip() -> &'static str {
    sdk_summary_delta_anchors()[0].old
}

fn write(p: &Path, body: &str) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

fn read(p: &Path) -> String {
    std::fs::read_to_string(p).unwrap()
}

fn write_minimal_bridge_for_patch(root: &Path) {
    write(&root.join(ATTRIBUTION_ENTRYPOINT_REL), "#!/usr/bin/env node\nconsole.log('bridge')\n");
    write(
        &root.join(CJS),
        "prefix;getAgentSkills:r.workspaceServices.getAgentSkills,...i},u=void 0===a?new OS(l):new dx(l,a);suffix",
    );
    write(
        &root.join(ESM),
        "prefix;getAgentSkills:r.workspaceServices.getAgentSkills,...i},u=void 0===a?new MS(l):new lx(l,a);suffix",
    );
    for dir in ["node_modules/@cursor/sdk/dist/cjs", "node_modules/@cursor/sdk/dist/esm"] {
        write(&root.join(dir).join("index.js"), &format!("prefix;{};suffix", summary_skip()));
    }
    write(&root.join(STEER_RUN_SERVER_REL), &format!("{STEER_RUN_MOUNT_OLD}\n"));
}

fn assert_agent_skills_stubbed(name: &str, src: &str) {
    assert!(!src.contains(NATIVE_AGENT_SKILLS_ASSIGN), "{name} still assigns workspaceServices.getAgentSkills");
    assert!(src.contains(STUB_AGENT_SKILLS_ASSIGN), "{name} missing getAgentSkills stub");
}

fn attributed(src: &str) -> bool {
    src.contains(&format!("attributionConfigProvider=globalThis.{ATTRIBUTION_PROVIDER_MARKER}"))
}

#[test]
fn write_config_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let path = attribution_config_path(dir.path()).unwrap();
    write_attribution_config(&path, true, false).unwrap();
    let v: serde_json::Value = serde_json::from_str(&read(&path)).unwrap();
    assert_eq!(v["attributeCommitsToAgent"], true);
    assert_eq!(v["attributePRsToAgent"], false);
    assert_eq!(attribution_config_path(Path::new("")), None);
    assert!(write_attribution_config(Path::new(""), true, true).is_err());
}

#[test]
fn preload_skips_identical() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(ATTRIBUTION_PRELOAD_REL);
    write_attribution_preload(&path).unwrap();
    let before = std::fs::metadata(&path).unwrap().modified().unwrap();
    let body1 = read(&path);
    write_attribution_preload(&path).unwrap();
    assert_eq!(body1, read(&path));
    assert_eq!(before, std::fs::metadata(&path).unwrap().modified().unwrap(), "identical preload rewritten");
}

#[test]
fn injects_provider() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path();
    write_minimal_bridge_for_patch(root);
    ensure_attribution_patch(root).unwrap();
    ensure_attribution_patch(root).unwrap();

    let preload = read(&root.join(ATTRIBUTION_PRELOAD_REL));
    assert!(preload.contains(ATTRIBUTION_PROVIDER_MARKER));
    assert!(preload.contains("__recurInstallSteerRun"));
    let entry = read(&root.join(ATTRIBUTION_ENTRYPOINT_REL));
    assert!(entry.contains("recur-attribution-provider.mjs"), "{entry}");
    assert!(entry.starts_with("#!/usr/bin/env node\n"), "shebang not preserved:\n{entry}");
    assert!(read(&root.join(STEER_RUN_SERVER_REL)).contains("__recurInstallSteerRun"));
    for rel in [CJS, ESM] {
        let s = read(&root.join(rel));
        assert!(attributed(&s), "{rel} not patched");
        assert_agent_skills_stubbed(rel, &s);
    }
    for rel in ["node_modules/@cursor/sdk/dist/cjs/index.js", "node_modules/@cursor/sdk/dist/esm/index.js"] {
        let s = read(&root.join(rel));
        assert!(!s.contains(summary_skip()), "{rel} still skips summary onDelta");
        assert!(s.contains(&summary_delta_patch_needle()), "{rel} missing summary marker");
    }
}

#[test]
fn steer_hook_rejects_unmatched() {
    let root = tempfile::tempdir().unwrap();
    write_minimal_bridge_for_patch(root.path());
    write(&root.path().join(STEER_RUN_SERVER_REL), "function mountSdkBridgeRoutes() { return 0 }\n");
    let err = ensure_attribution_patch(root.path()).unwrap_err();
    assert!(err.unsupported(), "{err}");
}

#[test]
fn steer_hook_missing_ok() {
    let root = tempfile::tempdir().unwrap();
    write_minimal_bridge_for_patch(root.path());
    std::fs::remove_file(root.path().join(STEER_RUN_SERVER_REL)).unwrap();
    ensure_attribution_patch(root.path()).unwrap();
}

#[test]
fn ignores_unmatched_cjs() {
    // The bridge loads ESM: a cjs-only miss must not fail the patch.
    let root = tempfile::tempdir().unwrap();
    write_minimal_bridge_for_patch(root.path());
    let cjs = root.path().join(CJS);
    write(&cjs, "prefix;getAgentSkills:r.workspaceServices.getAgentSkills};return void 0===m?new ZZ(n):new YY(n,m);suffix");
    ensure_attribution_patch(root.path()).unwrap();
    let esm = read(&root.path().join(ESM));
    assert!(attributed(&esm));
    assert_agent_skills_stubbed("esm", &esm);
    assert!(!read(&cjs).contains(STUB_AGENT_SKILLS_ASSIGN), "unmatched cjs must stay unmodified");
}

#[test]
fn rejects_unmatched_esm() {
    let root = tempfile::tempdir().unwrap();
    write_minimal_bridge_for_patch(root.path());
    write(
        &root.path().join(ESM),
        "prefix;getAgentSkills:r.workspaceServices.getAgentSkills};return void 0===b?new ZZ(o):new YY(o,b);suffix",
    );
    assert!(ensure_attribution_patch(root.path()).unwrap_err().unsupported());
}

fn already_attributed_esm() -> String {
    format!(
        "prefix;getAgentSkills:r.workspaceServices.getAgentSkills}};o.attributionConfigProvider=globalThis.{ATTRIBUTION_PROVIDER_MARKER}||{{get:()=>({{attribution:{{attributeCommitsToAgent:!1,attributePRsToAgent:!1}}}})}};return void 0===b?new n.Dv(o):new l(o,b);suffix"
    )
}

#[test]
fn rejects_unmatched_summary_delta() {
    let root = tempfile::tempdir().unwrap();
    write_minimal_bridge_for_patch(root.path());
    write(&root.path().join(ESM), &already_attributed_esm());
    let idx = root.path().join("node_modules/@cursor/sdk/dist/esm/index.js");
    write(
        &idx,
        &format!(
            r#"prefix;function(x){{return x.type==="summary"||x.type==="summary-started"||x.type==="summary-completed"}}(e)||{SDK_SUMMARY_DELTA_YIELD};suffix"#
        ),
    );
    assert!(ensure_attribution_patch(root.path()).unwrap_err().unsupported());
    assert!(!read(&idx).contains(&summary_delta_patch_needle()), "marker stamped onto a conditional yield");
}

#[test]
fn upgrades_unmarked_summary_delta() {
    let root = tempfile::tempdir().unwrap();
    write_minimal_bridge_for_patch(root.path());
    write(&root.path().join(ESM), &already_attributed_esm());
    let idx = root.path().join("node_modules/@cursor/sdk/dist/esm/index.js");
    write(&idx, &format!("prefix;{SDK_SUMMARY_DELTA_YIELD};suffix"));
    ensure_attribution_patch(root.path()).unwrap();
    assert!(read(&idx).contains(&summary_delta_patch_needle()));
}

#[test]
fn still_matches_sdk_1031() {
    let root = tempfile::tempdir().unwrap();
    write_minimal_bridge_for_patch(root.path());
    let dist = root.path().join("node_modules/@cursor/sdk/dist");
    let old = sdk_summary_delta_anchors()[1].old;
    let files = [
        (dist.join("cjs/616.js"), "prefix;getAgentSkills:r.workspaceServices.getAgentSkills};return void 0===m?new Pb(n):new yx(n,m);suffix".to_owned()),
        (dist.join("esm/357.js"), "prefix;getAgentSkills:r.workspaceServices.getAgentSkills};return void 0===b?new n.DvK(o):new l(o,b);suffix".to_owned()),
        (dist.join("cjs/index.js"), format!("prefix;{old};suffix")),
        (dist.join("esm/index.js"), format!("prefix;{old};suffix")),
    ];
    for (p, b) in &files {
        write(p, b);
    }
    ensure_attribution_patch(root.path()).unwrap();
    for (p, _) in &files {
        let s = read(p);
        if p.ends_with("index.js") {
            assert!(s.contains(&summary_delta_patch_needle()), "{}", p.display());
            continue;
        }
        assert!(attributed(&s), "{}", p.display());
        assert_agent_skills_stubbed(&p.display().to_string(), &s);
    }
}

fn old_cjs_case(body: &str) {
    let root = tempfile::tempdir().unwrap();
    write_minimal_bridge_for_patch(root.path());
    let cjs = root.path().join(CJS);
    write(&cjs, body);
    ensure_attribution_patch(root.path()).unwrap();
    let s = read(&cjs);
    assert!(attributed(&s), "old cjs left unpatched: {body}");
    assert_agent_skills_stubbed("old cjs", &s);
}

#[test]
fn still_matches_sdk_1028_cjs() {
    old_cjs_case("prefix;getAgentSkills:r.workspaceServices.getAgentSkills};return void 0===m?new Bw(n):new ex(n,m);suffix");
}

#[test]
fn still_matches_sdk_1027_cjs() {
    old_cjs_case("prefix;getAgentSkills:r.workspaceServices.getAgentSkills};return void 0===m?new $y(n):new $E(n,m);suffix");
}

#[test]
fn still_matches_sdk_1026_cjs() {
    old_cjs_case("prefix;getAgentSkills:r.workspaceServices.getAgentSkills};return void 0===m?new Wv(n):new zk(n,m);suffix");
}

#[test]
fn unsupported_without_anchors() {
    let root = tempfile::tempdir().unwrap();
    write(&root.path().join(ATTRIBUTION_ENTRYPOINT_REL), "console.log(1)\n");
    assert!(ensure_attribution_patch(root.path()).unwrap_err().unsupported());
}

#[test]
fn rejects_bad_roots() {
    assert!(!ensure_attribution_patch(Path::new("")).unwrap_err().unsupported());
    let f = tempfile::NamedTempFile::new().unwrap();
    let err = ensure_attribution_patch(f.path()).unwrap_err();
    assert!(err.to_string().ends_with("is not a directory"), "{err}");
}

/// The SDK does `.finally()` on getAgentSkills' result: the stub must be
/// thenable. Needs `node` on PATH, as in Go.
#[test]
fn agent_skills_stub_is_thenable() {
    let (_, expr) = STUB_AGENT_SKILLS_ASSIGN.split_once(':').unwrap();
    let out = std::process::Command::new("node")
        .arg("-e")
        .arg(format!("const getAgentSkills={expr}; getAgentSkills().finally(()=>{{}})"))
        .output()
        .expect("node on PATH");
    assert!(out.status.success(), "stub {expr} is not thenable: {}", String::from_utf8_lossy(&out.stderr));
}

#[test]
fn stubs_skills_on_already_attributed_install() {
    let root = tempfile::tempdir().unwrap();
    write_minimal_bridge_for_patch(root.path());
    ensure_attribution_patch(root.path()).unwrap();
    let cjs = root.path().join(CJS);
    // An older Recur install: attribution injected, native skills still wired.
    write(
        &cjs,
        &format!(
            "prefix;{NATIVE_AGENT_SKILLS_ASSIGN}}};n.attributionConfigProvider=globalThis.{ATTRIBUTION_PROVIDER_MARKER}||{{get:()=>({{attribution:{{attributeCommitsToAgent:!1,attributePRsToAgent:!1}}}})}};return void 0===m?new Bw(n):new ex(n,m);suffix"
        ),
    );
    ensure_attribution_patch(root.path()).unwrap();
    assert_agent_skills_stubbed("legacy-cjs", &read(&cjs));
}
