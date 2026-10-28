//! Go `cursor/install_test.go`, plus the env-gated pin, wheel and install
//! checks (`RECUR_CURSOR_SDK_PIN_CHECK`, `RECUR_CURSOR_ATTRIBUTION_PATCH_TEST`,
//! `RECUR_CURSOR_INSTALL_TEST`).

use std::io::Write as _;

use super::*;
use crate::locate::locate;

#[test]
fn pinned_sdk_version_in_pypi_url() {
    assert!(!PINNED_SDK_VERSION.is_empty() && PINNED_SDK_VERSION != "latest");
    assert!(pypi_pinned_project().contains(&format!("/cursor-sdk/{PINNED_SDK_VERSION}/json")));
    // The attribution anchors were validated against this release: bump the
    // pin, the anchors and this hardcode together.
    assert_eq!(
        PINNED_SDK_VERSION, "1.0.38",
        "update this test (and the attribution anchors) deliberately when bumping"
    );
}

/// The Go app and this port install the same wheel and patch it with the
/// same anchors, so a re-pin must bump both.
#[test]
fn pin_matches_go_app() {
    let go = include_str!("../../../../../internal/cursor/install.go");
    let want = format!("const pinnedSDKVersion = \"{PINNED_SDK_VERSION}\"");
    assert!(
        go.contains(&want),
        "internal/cursor/install.go pins a different cursor-sdk; re-pin both per .agents/skills/repin-cursor-sdk/SKILL.md"
    );
}

/// `wails3 task check:cursor-sdk-pin` for the Rust port: fails when PyPI's
/// latest cursor-sdk is ahead of the pin. Network; opt in with
/// `RECUR_CURSOR_SDK_PIN_CHECK=1 cargo test -p recur-provider-cursor check_cursor_sdk_pin_drift`.
/// On failure, re-pin per `.agents/skills/repin-cursor-sdk/SKILL.md`.
#[tokio::test]
async fn check_cursor_sdk_pin_drift() {
    if std::env::var_os("RECUR_CURSOR_SDK_PIN_CHECK").is_none() {
        eprintln!("skipped: set RECUR_CURSOR_SDK_PIN_CHECK=1 to compare the pin to PyPI latest");
        return;
    }
    let got = tokio::time::timeout(std::time::Duration::from_secs(30), check_pin_drift())
        .await
        .expect("PyPI timed out");
    match got {
        Ok(v) => eprintln!("cursor-sdk pin current: {v}"),
        Err(e) => panic!("{e}"),
    }
}

const SAMPLES: &[(&str, &str, &str)] = &[
    (
        "darwin",
        "arm64",
        "cursor_sdk-1.0.27-py3-none-macosx_11_0_arm64.whl",
    ),
    (
        "darwin",
        "amd64",
        "cursor_sdk-1.0.27-py3-none-macosx_11_0_x86_64.whl",
    ),
    (
        "linux",
        "arm64",
        "cursor_sdk-1.0.27-py3-none-manylinux2014_aarch64.manylinux_2_17_aarch64.whl",
    ),
    (
        "linux",
        "amd64",
        "cursor_sdk-1.0.27-py3-none-manylinux2014_x86_64.manylinux_2_17_x86_64.whl",
    ),
    (
        "windows",
        "amd64",
        "cursor_sdk-1.0.27-py3-none-win_amd64.whl",
    ),
];

fn matches(name: &str, os_tag: &str, arch: &[&str]) -> bool {
    let lower = name.to_lowercase();
    lower.contains(os_tag) && arch.iter().any(|a| lower.contains(a))
}

/// The tags match Cursor's real filenames (macOS `macosx_11_0_*`, Linux's
/// compound manylinux tag), for every platform, and each matches exactly one
/// wheel: a loose tag would install the wrong architecture's Node.
#[test]
fn platform_tags_match_real_filenames_exactly_once() {
    for (os, arch, want) in SAMPLES {
        let (os_tag, arch_tags) = platform_tags_for(os, arch).unwrap();
        assert!(matches(want, os_tag, arch_tags), "{os}/{arch} vs {want}");
        let n = SAMPLES
            .iter()
            .filter(|(_, _, n)| matches(n, os_tag, arch_tags))
            .count();
        assert_eq!(n, 1, "{os}/{arch} matched {n} wheels");
    }
    assert!(platform_tags_for("plan9", "amd64").is_err());
    assert!(platform_tags_for("linux", "riscv64").is_err());
    let (os_tag, arch_tags) = platform_tags().expect("this platform is supported");
    assert!(!os_tag.is_empty() && !arch_tags.is_empty());
}

#[test]
fn pick_wheel_requires_pin_and_platform() {
    let w = |name: &str| Wheel {
        filename: name.into(),
        ..Default::default()
    };
    let urls = [
        w(&format!("cursor_sdk-{PINNED_SDK_VERSION}.tar.gz")),
        w("cursor_sdk-1.0.27-py3-none-macosx_11_0_arm64.whl"),
        w(&format!(
            "cursor_sdk-{PINNED_SDK_VERSION}-py3-none-macosx_11_0_x86_64.whl"
        )),
        w(&format!(
            "Cursor_SDK-{PINNED_SDK_VERSION}-py3-none-macosx_11_0_arm64.WHL"
        )),
    ];
    let got = pick_wheel(&urls, "macosx", &["arm64", "aarch64"]).unwrap();
    assert_eq!(got.filename, urls[3].filename);
    assert!(pick_wheel(&urls[..3], "macosx", &["arm64"]).is_none());
}

fn write_test_wheel(path: &Path, members: &[(&str, &str, u32)]) {
    let f = std::fs::File::create(path).unwrap();
    let mut zw = zip::ZipWriter::new(f);
    for (name, body, mode) in members {
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .unix_permissions(*mode);
        zw.start_file(*name, opts).unwrap();
        zw.write_all(body.as_bytes()).unwrap();
    }
    zw.finish().unwrap();
}

fn noop(_: i64, _: i64) {}

/// Without the mode carried across, bin/node and the launcher come out
/// non-executable and the bridge cannot start.
#[test]
fn extract_preserves_executable_bit() {
    let tmp = tempfile::tempdir().unwrap();
    let archive = tmp.path().join("wheel.zip");
    write_test_wheel(
        &archive,
        &[
            (
                "cursor_sdk/_vendor/bridge/bin/cursor-sdk-bridge",
                "#!/bin/sh\necho hi\n",
                0o755,
            ),
            ("cursor_sdk/_vendor/bridge/bin/node", "binary", 0o755),
            (
                "cursor_sdk/_vendor/bridge/manifest.json",
                r#"{"sdkVersion":"1.0.27"}"#,
                0o644,
            ),
            ("cursor_sdk/_vendor/bridge/dist/index.js", "//js", 0o644),
            ("cursor_sdk/_client.py", "# python", 0o644),
        ],
    );
    let data = tempfile::tempdir().unwrap();
    let dst = data.path().join(BRIDGE_DIR_NAME);
    extract_bridge(&archive, &dst, &noop).unwrap();
    assert!(is_executable_file(&dst.join(launcher_rel_path())));
    assert!(is_executable_file(&dst.join("bin/node")));
    assert!(dst.join("manifest.json").is_file());
    assert!(dst.join("dist/index.js").is_file());
    assert!(
        !dst.join("_client.py").exists(),
        "extracted a file from outside the bridge prefix"
    );
    assert!(locate(data.path()).is_ok());
}

#[test]
fn extract_reports_progress() {
    let tmp = tempfile::tempdir().unwrap();
    let archive = tmp.path().join("wheel.zip");
    write_test_wheel(
        &archive,
        &[
            ("cursor_sdk/_vendor/bridge/a", "1234", 0o644),
            ("cursor_sdk/_vendor/bridge/dir/", "", 0o755),
            ("cursor_sdk/_vendor/bridge/b", "56", 0o644),
        ],
    );
    let ticks = std::sync::Mutex::new(Vec::new());
    extract_bridge(&archive, &tmp.path().join("out"), &|c, t| {
        ticks.lock().unwrap().push((c, t))
    })
    .unwrap();
    assert_eq!(*ticks.lock().unwrap(), [(4, 6), (6, 6)]);
}

#[test]
fn extract_rejects_wheel_without_bridge() {
    let tmp = tempfile::tempdir().unwrap();
    let archive = tmp.path().join("wheel.zip");
    write_test_wheel(&archive, &[("cursor_sdk/_client.py", "# python", 0o644)]);
    let err = extract_bridge(&archive, &tmp.path().join("out"), &noop).unwrap_err();
    assert_eq!(
        err,
        "cursor bridge install: wheel contains no cursor_sdk/_vendor/bridge/"
    );
}

/// Zip-slip: a crafted member must not escape the destination.
#[test]
fn extract_rejects_path_traversal() {
    let tmp = tempfile::tempdir().unwrap();
    let archive = tmp.path().join("wheel.zip");
    write_test_wheel(
        &archive,
        &[
            ("cursor_sdk/_vendor/bridge/../../escaped.sh", "pwned", 0o755),
            ("cursor_sdk/_vendor/bridge/bin/node", "ok", 0o755),
        ],
    );
    let err = extract_bridge(&archive, &tmp.path().join("out"), &noop).unwrap_err();
    assert!(err.contains("refusing unsafe path"), "{err}");
    assert!(!tmp.path().join("escaped.sh").exists());
}

#[test]
fn progress_percent() {
    let p = |current, total| InstallProgress {
        phase: InstallPhase::Downloading,
        message: String::new(),
        current,
        total,
    };
    assert_eq!(p(50, 200).percent(), 25);
    // An unknown total is indeterminate, not 0% or a divide panic.
    assert_eq!(p(50, 0).percent(), -1);
    assert_eq!(
        serde_json::to_string(&p(50, 200).event_payload()).unwrap(),
        r#"{"current":50,"message":"","percent":25,"phase":"downloading","total":200}"#
    );
    assert_eq!(
        serde_json::to_string(&install_error_payload("boom")).unwrap(),
        r#"{"message":"boom","percent":-1,"phase":"error"}"#
    );
}

#[tokio::test]
async fn install_rejects_empty_data_dir() {
    let err = install(Path::new(""), &CancellationToken::new(), &|_| {})
        .await
        .unwrap_err();
    assert_eq!(err, "cursor bridge install: no data directory");
}

/// A REAL download, unpack and start (~50MB from PyPI). Opt in with
/// `RECUR_CURSOR_INSTALL_TEST=1`.
#[tokio::test]
async fn install_real_smoke() {
    if std::env::var_os("RECUR_CURSOR_INSTALL_TEST").is_none() {
        eprintln!("skipped: set RECUR_CURSOR_INSTALL_TEST=1 to exercise a real download");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let phases = std::sync::Mutex::new(Vec::<&str>::new());
    let bin = install(dir.path(), &CancellationToken::new(), &|p| {
        let mut ph = phases.lock().unwrap();
        if ph.last() != Some(&p.phase.as_str()) {
            ph.push(p.phase.as_str());
        }
    })
    .await
    .expect("install");
    assert_eq!(
        *phases.lock().unwrap(),
        [
            "resolving",
            "downloading",
            "verifying",
            "extracting",
            "done"
        ]
    );
    // What was installed must run.
    let b = crate::bridge::Bridge::start(
        crate::bridge::BridgeConfig {
            bin: bin.clone(),
            ..Default::default()
        },
        &CancellationToken::new(),
    )
    .await
    .expect("start freshly installed bridge");
    let v = b
        .connect_client()
        .unwrap()
        .get_version(&CancellationToken::new())
        .await
        .expect("GetVersion");
    assert_eq!(v.protocol_version, "sdk.v1");
    b.close().await;
}

/// Downloads the pinned wheel and asserts the minify anchors still match,
/// then runs the patched output on the wheel's own Node. Opt in with
/// `RECUR_CURSOR_ATTRIBUTION_PATCH_TEST=1` (network, ~50MB).
#[tokio::test]
async fn attribution_patch_against_pinned_wheel() {
    use crate::attribution::*;
    if std::env::var_os("RECUR_CURSOR_ATTRIBUTION_PATCH_TEST").is_none() {
        eprintln!(
            "skipped: set RECUR_CURSOR_ATTRIBUTION_PATCH_TEST=1 to download the pinned wheel"
        );
        return;
    }
    let http = reqwest::Client::new();
    let (w, version) = resolve_wheel(&http).await.expect("resolve wheel");
    assert_eq!(version, PINNED_SDK_VERSION);
    let staging = tempfile::tempdir().unwrap();
    let archive = staging.path().join("wheel.zip");
    download(&http, &w.url, &archive, &|_| {})
        .await
        .expect("download");
    let root = staging.path().join("bridge");
    extract_bridge(&archive, &root, &noop).expect("extract");
    ensure_attribution_patch(&root).expect("patch the pinned wheel");
    ensure_attribution_patch(&root).expect("idempotent patch");

    let mut patched: Vec<PathBuf> = [
        ATTRIBUTION_PRELOAD_REL,
        ATTRIBUTION_ENTRYPOINT_REL,
        STEER_RUN_SERVER_REL,
    ]
    .iter()
    .map(PathBuf::from)
    .collect();
    let needle = format!("attributionConfigProvider=globalThis.{ATTRIBUTION_PROVIDER_MARKER}");
    for rel in [
        "node_modules/@cursor/sdk/dist/cjs",
        "node_modules/@cursor/sdk/dist/esm",
    ] {
        let mut found = false;
        let mut entries: Vec<_> = std::fs::read_dir(root.join(rel))
            .unwrap()
            .flatten()
            .collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let name = e.file_name().to_string_lossy().into_owned();
            if !name.ends_with(".js") {
                continue;
            }
            let s = std::fs::read_to_string(e.path()).unwrap_or_default();
            if s.contains(&needle) {
                found = true;
                assert!(
                    !s.contains(NATIVE_AGENT_SKILLS_ASSIGN) && s.contains(STUB_AGENT_SKILLS_ASSIGN),
                    "{rel}/{name}"
                );
                patched.push(Path::new(rel).join(&name));
                break;
            }
        }
        assert!(found, "no patched SDK bundle under {rel}");
    }
    let skip = sdk_summary_delta_anchors()[0].old;
    for rel in [
        "node_modules/@cursor/sdk/dist/cjs/index.js",
        "node_modules/@cursor/sdk/dist/esm/index.js",
    ] {
        let s = std::fs::read_to_string(root.join(rel)).unwrap();
        assert!(!s.contains(skip), "{rel} still skips summary onDelta");
        assert!(
            s.contains(&summary_delta_patch_needle()),
            "{rel} missing summary marker"
        );
        patched.push(PathBuf::from(rel));
    }
    assert!(
        std::fs::read_to_string(root.join(STEER_RUN_SERVER_REL))
            .unwrap()
            .contains("__recurInstallSteerRun")
    );

    // String checks cannot see a rewrite that no longer parses.
    let node = root.join("bin/node");
    for rel in &patched {
        let out = std::process::Command::new(&node)
            .arg("--check")
            .arg(root.join(rel))
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "node --check {}: {}",
            rel.display(),
            String::from_utf8_lossy(&out.stderr)
        );
    }
    let script = format!(
        r#"
await import("./recur-attribution-provider.mjs");
const {{ SdkAgentService }} = await import("@anysphere/proto/sdk/v1/sdk_agent_service_connect.js");
if (!globalThis.{ATTRIBUTION_PROVIDER_MARKER}?.get().attribution) throw new Error("attribution provider not installed");
const impl = {{}};
globalThis.__recurInstallSteerRun(SdkAgentService, impl, {{ getRun: () => undefined }});
if (SdkAgentService.methods.steerRun?.name !== "SteerRun") throw new Error("SteerRun not registered");
const res = await impl.steerRun({{ runId: "r", text: "t" }});
if (res.outcome !== "revert_to_followup") throw new Error("steerRun outcome " + res.outcome);
"#
    );
    let out = std::process::Command::new(&node)
        .args(["--input-type=module", "-e", &script])
        .current_dir(root.join("dist"))
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "preload smoke: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}
