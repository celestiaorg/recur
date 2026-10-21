//! On-demand bridge install (Go `cursor/install.go`). The bridge ships only
//! inside Cursor's `cursor-sdk` Python wheel (~50MB, ~155MB unpacked, mostly
//! a Node runtime), so Recur downloads the pinned wheel and unpacks
//! `cursor_sdk/_vendor/bridge/**` instead of vendoring it.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;

use crate::attribution::ensure_attribution_patch;
use crate::gojson::null_default;
use crate::locate::{BRIDGE_DIR_NAME, clean, is_executable_file, launcher_rel_path};

/// The cursor-sdk release Recur installs. The attribution patch matches
/// minified anchors from this build; fetching "latest" would break them on
/// the next Cursor publish. Bump only together with refreshed anchors
/// (`.agents/skills/repin-cursor-sdk/SKILL.md`).
pub const PINNED_SDK_VERSION: &str = "1.0.38";

/// The pinned release's PyPI metadata.
pub(crate) fn pypi_pinned_project() -> String {
    format!("https://pypi.org/pypi/cursor-sdk/{PINNED_SDK_VERSION}/json")
}

/// Project-level PyPI metadata (includes the latest version).
pub(crate) const PYPI_PROJECT_JSON: &str = "https://pypi.org/pypi/cursor-sdk/json";

/// The path inside the wheel that holds the bridge.
pub(crate) const BRIDGE_ARCHIVE_PREFIX: &str = "cursor_sdk/_vendor/bridge/";

/// Bounds the whole fetch and unpack.
const INSTALL_TIMEOUT: Duration = Duration::from_secs(15 * 60);

/// Progress callbacks fire about once per MiB, not per read.
const PROGRESS_STRIDE: u64 = 1 << 20;

/// Bounds one extracted member against a hostile archive; the largest real
/// one is the ~120MB Node binary.
const MEMBER_LIMIT: u64 = 512 << 20;

/// One install step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum InstallPhase {
    Resolving,
    Downloading,
    Verifying,
    Extracting,
    Done,
}

impl InstallPhase {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Resolving => "resolving",
            Self::Downloading => "downloading",
            Self::Verifying => "verifying",
            Self::Extracting => "extracting",
            Self::Done => "done",
        }
    }
}

/// One progress tick; `total` is 0 when unknown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallProgress {
    pub phase: InstallPhase,
    pub message: String,
    pub current: i64,
    pub total: i64,
}

impl InstallProgress {
    /// 0..100, or -1 when the total is unknown.
    pub fn percent(&self) -> i64 {
        if self.total <= 0 {
            return -1;
        }
        self.current * 100 / self.total
    }

    /// The `cursor.install` bus payload:
    /// `{phase, message, current, total, percent}` (keys sorted like Go's
    /// map encoding).
    pub fn event_payload(&self) -> serde_json::Value {
        let m: std::collections::BTreeMap<&str, serde_json::Value> = [
            ("current", self.current.into()),
            ("message", self.message.clone().into()),
            ("percent", self.percent().into()),
            ("phase", self.phase.as_str().into()),
            ("total", self.total.into()),
        ]
        .into_iter()
        .collect();
        serde_json::to_value(m).unwrap_or_default()
    }
}

/// The `cursor.install` payload for a failed install:
/// `{phase: "error", message, percent: -1}`.
pub fn install_error_payload(message: &str) -> serde_json::Value {
    let m: std::collections::BTreeMap<&str, serde_json::Value> = [
        ("message", message.into()),
        ("percent", (-1).into()),
        ("phase", "error".into()),
    ]
    .into_iter()
    .collect();
    serde_json::to_value(m).unwrap_or_default()
}

/// One downloadable file from the PyPI metadata.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Wheel {
    #[serde(deserialize_with = "null_default")]
    pub filename: String,
    #[serde(deserialize_with = "null_default")]
    pub url: String,
    #[serde(deserialize_with = "null_default")]
    pub size: i64,
    #[serde(deserialize_with = "null_default")]
    pub digests: Digests,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Digests {
    #[serde(deserialize_with = "null_default")]
    pub sha256: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Meta {
    #[serde(deserialize_with = "null_default")]
    info: Info,
    #[serde(deserialize_with = "null_default")]
    urls: Vec<Wheel>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Info {
    #[serde(deserialize_with = "null_default")]
    version: String,
}

/// Downloads and unpacks the bridge into `<data_dir>/cursor-bridge`,
/// returning the launcher. Staged in a sibling temp dir and moved into place
/// only when complete, so an interrupted download never leaves a
/// half-extracted bridge for [`crate::locate`].
pub async fn install(
    data_dir: &Path,
    cancel: &CancellationToken,
    progress: &(dyn Fn(InstallProgress) + Send + Sync),
) -> Result<PathBuf, String> {
    if data_dir.as_os_str().is_empty() {
        return Err("cursor bridge install: no data directory".into());
    }
    let run = install_inner(data_dir, progress);
    tokio::select! {
        r = tokio::time::timeout(INSTALL_TIMEOUT, run) => {
            r.unwrap_or_else(|_| Err("cursor bridge install: context deadline exceeded".into()))
        }
        _ = cancel.cancelled() => Err("cursor bridge install: context canceled".into()),
    }
}

async fn install_inner(
    data_dir: &Path,
    progress: &(dyn Fn(InstallProgress) + Send + Sync),
) -> Result<PathBuf, String> {
    let report = |phase, message: String, current, total| {
        progress(InstallProgress {
            phase,
            message,
            current,
            total,
        })
    };
    report(
        InstallPhase::Resolving,
        "Finding the Cursor SDK for this platform…".into(),
        0,
        0,
    );
    let http = reqwest::Client::new();
    let (wheel, version) = resolve_wheel(&http).await?;

    let dest = data_dir.join(BRIDGE_DIR_NAME);
    let staging = tempfile::Builder::new()
        .prefix(".cursor-bridge-staging-")
        .tempdir_in(data_dir)
        .map_err(|e| format!("cursor bridge install: staging dir: {e}"))?;

    let archive = staging.path().join("wheel.zip");
    let msg = format!("Downloading Cursor SDK {version}…");
    report(InstallPhase::Downloading, msg.clone(), 0, wheel.size);
    let sum = download(&http, &wheel.url, &archive, &|n| {
        report(InstallPhase::Downloading, msg.clone(), n, wheel.size)
    })
    .await?;

    // PyPI publishes a sha256 per file: a truncated or tampered download
    // fails here, not as a baffling Node crash later.
    report(InstallPhase::Verifying, "Verifying download…".into(), 0, 0);
    let want = wheel.digests.sha256.to_lowercase();
    if !want.is_empty() && sum != want {
        return Err(format!(
            "cursor bridge install: checksum mismatch (got {sum}, want {want})"
        ));
    }

    report(InstallPhase::Extracting, "Extracting…".into(), 0, 0);
    let unpacked = staging.path().join("bridge");
    {
        let (archive, unpacked) = (archive.clone(), unpacked.clone());
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<(i64, i64)>();
        let job = tokio::task::spawn_blocking(move || {
            extract_bridge(&archive, &unpacked, &|c, t| {
                let _ = tx.send((c, t));
            })
        });
        while let Some((c, t)) = rx.recv().await {
            report(InstallPhase::Extracting, "Extracting…".into(), c, t);
        }
        job.await
            .map_err(|e| format!("cursor bridge install: extract: {e}"))??;
    }

    if !is_executable_file(&unpacked.join(launcher_rel_path())) {
        return Err(format!(
            "cursor bridge install: no launcher at {} after extraction",
            launcher_rel_path().display()
        ));
    }

    // Wire attribution before the tree goes live: the wheel is the pin, so
    // the anchors must match.
    report(
        InstallPhase::Extracting,
        "Patching attribution controls…".into(),
        0,
        0,
    );
    ensure_attribution_patch(&unpacked).map_err(|e| {
        format!("cursor bridge install: attribution patch (sdk {PINNED_SDK_VERSION}): {e}")
    })?;

    // Swap into place. A running bridge holds its own open files.
    match std::fs::remove_dir_all(&dest) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
            return Err(format!("cursor bridge install: clear old install: {e}"));
        }
        _ => {}
    }
    std::fs::rename(&unpacked, &dest)
        .map_err(|e| format!("cursor bridge install: move into place: {e}"))?;

    report(InstallPhase::Done, "Cursor SDK installed".into(), 0, 0);
    Ok(dest.join(launcher_rel_path()))
}

/// PyPI's latest cursor-sdk version, for the pin-drift check (not Install).
pub async fn latest_sdk_version() -> Result<String, String> {
    let resp = reqwest::Client::new()
        .get(PYPI_PROJECT_JSON)
        .send()
        .await
        .map_err(|e| format!("cursor sdk pin check: reach PyPI: {e}"))?;
    if resp.status() != reqwest::StatusCode::OK {
        return Err(format!(
            "cursor sdk pin check: PyPI returned {}",
            status_text(resp.status())
        ));
    }
    let body = read_capped(resp, 8 << 20)
        .await
        .map_err(|e| format!("cursor sdk pin check: decode PyPI metadata: {e}"))?;
    let meta: Meta = serde_json::from_slice::<Option<Meta>>(&body)
        .map_err(|e| format!("cursor sdk pin check: decode PyPI metadata: {e}"))?
        .unwrap_or_default();
    if meta.info.version.is_empty() {
        return Err("cursor sdk pin check: empty version in PyPI metadata".into());
    }
    Ok(meta.info.version)
}

/// The pin check `check:cursor-sdk-pin` runs: `Ok(latest)` when PyPI's
/// latest equals [`PINNED_SDK_VERSION`], else the drift message.
pub async fn check_pin_drift() -> Result<String, String> {
    let latest = latest_sdk_version().await?;
    if latest == PINNED_SDK_VERSION {
        return Ok(latest);
    }
    Err(format!(
        "cursor-sdk pin drift: pinned={PINNED_SDK_VERSION} pypi_latest={latest} — re-pin (see .agents/skills/repin-cursor-sdk/SKILL.md)"
    ))
}

/// Picks this platform's wheel from the pinned release by MATCHING
/// filenames: macOS wheels are `macosx_11_0_*`, Linux ones carry a compound
/// `manylinux2014_x86_64.manylinux_2_17_x86_64`, and a constructed PEP 425
/// tag would stop matching on the next rebuild.
pub(crate) async fn resolve_wheel(http: &reqwest::Client) -> Result<(Wheel, String), String> {
    let resp = http
        .get(pypi_pinned_project())
        .send()
        .await
        .map_err(|e| format!("cursor bridge install: reach PyPI: {e}"))?;
    if resp.status() != reqwest::StatusCode::OK {
        return Err(format!(
            "cursor bridge install: PyPI returned {} for cursor-sdk=={PINNED_SDK_VERSION}",
            status_text(resp.status())
        ));
    }
    let body = read_capped(resp, 8 << 20)
        .await
        .map_err(|e| format!("cursor bridge install: decode PyPI metadata: {e}"))?;
    let meta: Meta = serde_json::from_slice::<Option<Meta>>(&body)
        .map_err(|e| format!("cursor bridge install: decode PyPI metadata: {e}"))?
        .unwrap_or_default();
    if meta.info.version != PINNED_SDK_VERSION {
        return Err(format!(
            "cursor bridge install: PyPI returned version {:?}, want pinned {PINNED_SDK_VERSION}",
            meta.info.version
        ));
    }
    let (os_tag, arch_tags) = platform_tags()?;
    pick_wheel(&meta.urls, os_tag, arch_tags)
        .map(|w| (w, PINNED_SDK_VERSION.to_owned()))
        .ok_or_else(|| {
            format!(
                "cursor bridge install: no cursor-sdk=={PINNED_SDK_VERSION} wheel for {}/{}",
                go_os(),
                go_arch()
            )
        })
}

/// The first wheel for this OS and arch whose filename names the pin.
pub(crate) fn pick_wheel(urls: &[Wheel], os_tag: &str, arch_tags: &[&str]) -> Option<Wheel> {
    let pin_token = format!("-{}-", PINNED_SDK_VERSION.to_lowercase());
    urls.iter()
        .find(|w| {
            let name = w.filename.to_lowercase();
            name.ends_with(".whl")
                && name.contains(os_tag)
                && name.contains(&pin_token)
                && arch_tags.iter().any(|a| name.contains(a))
        })
        .cloned()
}

fn go_os() -> &'static str {
    match std::env::consts::OS {
        "macos" => "darwin",
        other => other,
    }
}

fn go_arch() -> &'static str {
    match std::env::consts::ARCH {
        "aarch64" => "arm64",
        "x86_64" => "amd64",
        other => other,
    }
}

/// The OS substring and acceptable arch substrings of this machine's wheel.
pub(crate) fn platform_tags() -> Result<(&'static str, &'static [&'static str]), String> {
    platform_tags_for(go_os(), go_arch())
}

pub(crate) fn platform_tags_for(
    os: &str,
    arch: &str,
) -> Result<(&'static str, &'static [&'static str]), String> {
    let os_tag = match os {
        "darwin" => "macosx",
        "linux" => "manylinux",
        "windows" => "win",
        other => return Err(format!("cursor bridge install: unsupported OS {other:?}")),
    };
    match arch {
        "arm64" => Ok((os_tag, &["arm64", "aarch64"])),
        "amd64" => Ok((os_tag, &["x86_64", "amd64"])),
        other => Err(format!(
            "cursor bridge install: unsupported architecture {other:?}"
        )),
    }
}

fn status_text(s: reqwest::StatusCode) -> String {
    format!(
        "{} {}",
        s.as_u16(),
        s.canonical_reason().unwrap_or_default()
    )
}

async fn read_capped(mut resp: reqwest::Response, limit: usize) -> Result<Vec<u8>, reqwest::Error> {
    let mut out = Vec::new();
    while let Some(chunk) = resp.chunk().await? {
        let room = limit.saturating_sub(out.len());
        out.extend_from_slice(&chunk[..chunk.len().min(room)]);
        if out.len() >= limit {
            break;
        }
    }
    Ok(out)
}

/// Streams `url` to `dst`, reporting bytes written about once per MiB, and
/// returns the hex sha256 of what it wrote.
pub(crate) async fn download(
    http: &reqwest::Client,
    url: &str,
    dst: &Path,
    on_progress: &(dyn Fn(i64) + Send + Sync),
) -> Result<String, String> {
    let mut resp = http
        .get(url)
        .send()
        .await
        .map_err(|e| format!("cursor bridge install: download: {e}"))?;
    if resp.status() != reqwest::StatusCode::OK {
        return Err(format!(
            "cursor bridge install: download returned {}",
            status_text(resp.status())
        ));
    }
    let mut f = std::fs::File::create(dst).map_err(|e| e.to_string())?;
    let mut h = Sha256::new();
    let (mut n, mut last) = (0u64, 0u64);
    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|e| format!("cursor bridge install: download: {e}"))?
    {
        f.write_all(&chunk)
            .map_err(|e| format!("cursor bridge install: download: {e}"))?;
        h.update(&chunk);
        n += chunk.len() as u64;
        if n - last >= PROGRESS_STRIDE {
            last = n;
            on_progress(n as i64);
        }
    }
    f.sync_all().map_err(|e| e.to_string())?;
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// Unpacks `cursor_sdk/_vendor/bridge/**` into `dst`, keeping the
/// executable bit: without it `bin/node` and the launcher cannot start.
pub(crate) fn extract_bridge(
    archive: &Path,
    dst: &Path,
    on_progress: &dyn Fn(i64, i64),
) -> Result<(), String> {
    use std::os::unix::fs::OpenOptionsExt;
    let file = std::fs::File::open(archive)
        .map_err(|e| format!("cursor bridge install: open wheel: {e}"))?;
    let mut zip = zip::ZipArchive::new(file)
        .map_err(|e| format!("cursor bridge install: open wheel: {e}"))?;

    let mut members = Vec::new();
    let mut total: i64 = 0;
    for i in 0..zip.len() {
        let f = zip.by_index_raw(i).map_err(|e| e.to_string())?;
        let name = String::from_utf8_lossy(f.name_raw()).into_owned();
        if !name.starts_with(BRIDGE_ARCHIVE_PREFIX) || name.ends_with('/') {
            continue;
        }
        total += f.size() as i64;
        members.push(i);
    }
    if members.is_empty() {
        return Err(format!(
            "cursor bridge install: wheel contains no {BRIDGE_ARCHIVE_PREFIX}"
        ));
    }

    let dst_clean = clean(dst);
    let mut done: i64 = 0;
    for i in members {
        let mut f = zip.by_index(i).map_err(|e| e.to_string())?;
        let name = String::from_utf8_lossy(f.name_raw()).into_owned();
        let rel = &name[BRIDGE_ARCHIVE_PREFIX.len()..];
        // Zip-slip: a crafted member must not write outside dst.
        let target = clean(&dst.join(go_path_clean(rel)));
        if !target.starts_with(&dst_clean) || target == dst_clean {
            return Err(format!(
                "cursor bridge install: refusing unsafe path {name:?} in wheel"
            ));
        }
        if let Some(dir) = target.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let mut mode = f.unix_mode().unwrap_or(0) & 0o777;
        if mode == 0 {
            mode = 0o644;
        }
        let mut out = std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .mode(mode)
            .open(&target)
            .map_err(|e| e.to_string())?;
        std::io::copy(&mut std::io::Read::take(&mut f, MEMBER_LIMIT), &mut out)
            .map_err(|e| e.to_string())?;
        done += f.size() as i64;
        on_progress(done, total);
    }
    Ok(())
}

/// Go `path.Clean` on a slash path (lexical; leading `..` kept).
fn go_path_clean(p: &str) -> PathBuf {
    clean(Path::new(p))
}

#[cfg(test)]
mod tests;
