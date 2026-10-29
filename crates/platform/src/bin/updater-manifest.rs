//! Writes and checks the signed update manifest for a release:
//!
//!     updater-manifest manifest --version V [--name N] [--key FILE] [--url-prefix URL] --output FILE ARTIFACT...
//!     updater-manifest verify --manifest FILE --publickey FILE [--dir DIR]

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use recur_platform::updater::manifest;

fn main() -> ExitCode {
    match run(std::env::args().skip(1).collect()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("updater-manifest: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: Vec<String>) -> Result<(), String> {
    let (cmd, rest) = args
        .split_first()
        .ok_or("usage: updater-manifest manifest|verify ...")?;
    let mut flags = HashMap::new();
    let mut files = Vec::new();
    let mut it = rest.iter();
    while let Some(a) = it.next() {
        match a.strip_prefix("--") {
            Some(name) => {
                let v = it.next().ok_or_else(|| format!("--{name} needs a value"))?;
                flags.insert(name.to_string(), v.clone());
            }
            None => files.push(PathBuf::from(a)),
        }
    }
    let flag = |name: &str| flags.get(name).map(String::as_str).unwrap_or_default();
    let read = |name: &str| -> Result<Vec<u8>, String> {
        let path = flag(name);
        if path.is_empty() {
            return Err(format!("--{name} is required"));
        }
        std::fs::read(path).map_err(|e| format!("{path}: {e}"))
    };
    match cmd.as_str() {
        "manifest" => {
            let key = match flag("key") {
                "" => None,
                _ => Some(manifest::load_signing_key(&read("key")?)?),
            };
            let out = manifest::write_manifest(
                &manifest::ManifestOptions {
                    version: flag("version"),
                    name: flag("name"),
                    url_prefix: flag("url-prefix"),
                    key: key.as_ref(),
                },
                &files,
            )?;
            let path = flag("output");
            if path.is_empty() {
                return Err("--output is required".into());
            }
            std::fs::write(path, out).map_err(|e| format!("{path}: {e}"))?;
            eprintln!(
                "wrote {path}: {} artifact(s), {}",
                files.len(),
                if key.is_some() { "signed" } else { "unsigned" }
            );
            Ok(())
        }
        "verify" => {
            let manifest_path = flag("manifest");
            let dir = match flag("dir") {
                "" => Path::new(manifest_path)
                    .parent()
                    .unwrap_or(Path::new("."))
                    .to_path_buf(),
                d => PathBuf::from(d),
            };
            for line in manifest::verify_manifest(&read("manifest")?, &dir, &read("publickey")?)? {
                println!("{line}");
            }
            Ok(())
        }
        other => Err(format!(
            "unknown command {other:?}; want manifest or verify"
        )),
    }
}
