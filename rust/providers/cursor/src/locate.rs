//! Where the bridge lives (Go `cursor/locate.go`, `manifest.go`, and the
//! root helpers of `attribution.go`).

use std::path::{Component, Path, PathBuf};

/// The install directory under Recur's data dir, laid out like the vendored
/// copy in Cursor's Python wheel so the same launcher works:
///
/// ```text
/// <dataDir>/cursor-bridge/
///   bin/cursor-sdk-bridge   (sh launcher)
///   bin/node                (bundled runtime)
///   dist/…                  (the SDK JS)
///   manifest.json
/// ```
pub const BRIDGE_DIR_NAME: &str = "cursor-bridge";

/// The smoke-test seam: point it at an unpacked cursor-sdk wheel and tests
/// that call [`resolve_launcher`] drive that copy. Production [`locate`]
/// ignores it.
pub const ENV_BRIDGE_PATH: &str = "RECUR_CURSOR_BRIDGE";

/// The launcher inside a bridge install.
pub fn launcher_rel_path() -> PathBuf {
    Path::new("bin").join("cursor-sdk-bridge")
}

/// Recur's own `<dataDir>/cursor-bridge` launcher. Env overrides, the
/// settings path and pip wheels are not consulted: those copies skip the
/// attribution patch and keep native getAgentSkills.
pub fn locate(data_dir: &Path) -> Result<PathBuf, String> {
    if data_dir.as_os_str().is_empty() {
        return Err("cursor bridge not found — install it from Settings".into());
    }
    let p = data_dir.join(BRIDGE_DIR_NAME);
    resolve_candidate(&p).ok_or_else(|| {
        format!(
            "cursor bridge not found at {} — install it from Settings",
            p.display()
        )
    })
}

/// A launcher path or bridge root to its launcher. Smoke tests use this with
/// `$RECUR_CURSOR_BRIDGE`; agent runs must use [`locate`].
pub fn resolve_launcher(p: &Path) -> Option<PathBuf> {
    resolve_candidate(p)
}

fn resolve_candidate(p: &Path) -> Option<PathBuf> {
    if p.as_os_str().is_empty() {
        return None;
    }
    if is_executable_file(p) {
        return Some(p.to_path_buf());
    }
    let launcher = p.join(launcher_rel_path());
    is_executable_file(&launcher).then_some(launcher)
}

pub(crate) fn is_executable_file(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p).is_ok_and(|m| !m.is_dir() && m.permissions().mode() & 0o111 != 0)
}

/// The install directory of a launcher or root, only when the launcher sits
/// at `<root>/bin/cursor-sdk-bridge` (the layout Install produces). A bare
/// test launcher outside `bin/` has no root.
pub fn bridge_root(bin_or_root: &Path) -> Option<PathBuf> {
    let resolved = resolve_candidate(bin_or_root)?;
    let dir = resolved.parent()?;
    if dir.file_name()? != "bin" {
        return None;
    }
    Some(clean(&dir.join("..")))
}

/// [`bridge_root`], only when that root is Recur's own
/// `<dataDir>/cursor-bridge` install.
pub fn owned_bridge_root(data_dir: &Path, bin_or_root: &Path) -> Option<PathBuf> {
    let root = bridge_root(bin_or_root)?;
    if data_dir.as_os_str().is_empty() {
        return None;
    }
    (root == clean(&data_dir.join(BRIDGE_DIR_NAME))).then_some(root)
}

/// The cursor-sdk version from an installed bridge's manifest.json. The
/// bridge's own GetVersion is the shell version (always "1.0.0"); the pin
/// users care about is sdkVersion.
pub fn sdk_version(bin_or_root: &Path) -> String {
    #[derive(serde::Deserialize, Default)]
    #[serde(default)]
    struct Manifest {
        #[serde(
            rename = "sdkVersion",
            deserialize_with = "crate::gojson::null_default"
        )]
        sdk_version: String,
    }
    let Some(root) = bridge_root(bin_or_root) else {
        return String::new();
    };
    let Ok(data) = std::fs::read(root.join("manifest.json")) else {
        return String::new();
    };
    serde_json::from_slice::<Option<Manifest>>(&data)
        .ok()
        .flatten()
        .map(|m| crate::gojson::trim(&m.sdk_version).to_owned())
        .unwrap_or_default()
}

/// Go `filepath.Clean`: lexical, `..` resolved against earlier elements, an
/// empty result is ".".
pub(crate) fn clean(p: &Path) -> PathBuf {
    let mut out: Vec<Component<'_>> = Vec::new();
    for c in p.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => match out.last() {
                Some(Component::Normal(_)) => {
                    out.pop();
                }
                Some(Component::RootDir) => {}
                _ => out.push(c),
            },
            other => out.push(other),
        }
    }
    if out.is_empty() {
        return PathBuf::from(".");
    }
    out.iter().collect()
}

/// Go `filepath.Abs`: joined to the working directory when relative, then
/// cleaned.
pub(crate) fn abs(p: &Path) -> std::io::Result<PathBuf> {
    if p.is_absolute() {
        return Ok(clean(p));
    }
    Ok(clean(&std::env::current_dir()?.join(p)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_exec(p: &Path) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        recur_exec::write_executable(p, "#!/bin/sh\n", 0o755).unwrap();
    }

    // Go TestLocateOnlyDataDir.
    #[test]
    fn locate_only_data_dir() {
        let data = tempfile::tempdir().unwrap();
        let ext_dir = tempfile::tempdir().unwrap();
        let external = ext_dir.path().join("cursor-sdk-bridge");
        write_exec(&external);
        // Locate never consults RECUR_CURSOR_BRIDGE; nothing to set.
        assert!(locate(data.path()).is_err());

        let owned = data
            .path()
            .join(BRIDGE_DIR_NAME)
            .join("bin")
            .join("cursor-sdk-bridge");
        write_exec(&owned);
        assert_eq!(locate(data.path()).unwrap(), owned);
        assert_eq!(resolve_launcher(&external), Some(external));
        assert!(locate(Path::new("")).is_err());
    }

    // Go TestSDKVersionFromManifest.
    #[test]
    fn sdk_version_from_manifest() {
        let root = tempfile::tempdir().unwrap();
        let launcher = root.path().join("bin").join("cursor-sdk-bridge");
        write_exec(&launcher);
        std::fs::write(
            root.path().join("manifest.json"),
            "{\n\t\t\"bridgeVersion\": \"1.0.0\",\n\t\t\"sdkVersion\": \"1.0.27\"\n\t}",
        )
        .unwrap();
        assert_eq!(sdk_version(&launcher), "1.0.27");
        assert_eq!(sdk_version(root.path()), "1.0.27");
        assert_eq!(sdk_version(Path::new("")), "");
    }

    // Go TestBridgeRootRequiresBinLayout.
    #[test]
    fn bridge_root_requires_bin_layout() {
        let dir = tempfile::tempdir().unwrap();
        let bare = dir.path().join("cursor-sdk-bridge");
        write_exec(&bare);
        assert_eq!(bridge_root(&bare), None);

        let root = dir.path().join("cursor-bridge");
        let launcher = root.join("bin").join("cursor-sdk-bridge");
        write_exec(&launcher);
        assert_eq!(bridge_root(&launcher), Some(root.clone()));
        assert_eq!(bridge_root(&root), Some(root));
    }

    // Go TestOwnedBridgeRootOnlyRecurDataDir.
    #[test]
    fn owned_bridge_root_only_recur_data_dir() {
        let data = tempfile::tempdir().unwrap();
        let owned = data.path().join(BRIDGE_DIR_NAME);
        let launcher = owned.join("bin").join("cursor-sdk-bridge");
        write_exec(&launcher);
        assert_eq!(owned_bridge_root(data.path(), &launcher), Some(owned));

        let other = tempfile::tempdir().unwrap();
        let ext = other
            .path()
            .join("cursor-bridge")
            .join("bin")
            .join("cursor-sdk-bridge");
        write_exec(&ext);
        assert_eq!(owned_bridge_root(data.path(), &ext), None);
    }

    #[test]
    fn clean_matches_go() {
        for (input, want) in [
            ("/a/b/../c", "/a/c"),
            ("/../x", "/x"),
            ("a/../../b", "../b"),
            ("", "."),
            ("./", "."),
            ("/a/./b/", "/a/b"),
        ] {
            assert_eq!(clean(Path::new(input)), PathBuf::from(want), "{input}");
        }
    }
}
