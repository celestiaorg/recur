//! The publishing side of the update manifest (`updater-manifest` bin, run
//! by the release workflow): writes the signed manifest
//! [`super::EndpointProvider`] reads, and re-checks one against the files on
//! disk with the same decoding and verification the app runs after a
//! download. Ed25519ph over each artifact's SHA-512, schema version 1.

use std::path::{Path, PathBuf};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use serde::Serialize;

use super::endpoint::{Manifest, build_verification, filename_from_url};
use super::verify::{Hasher, Prehashed, pem_decode, run_verification};

const SCHEMA_VERSION: i64 = 1;

/// PKCS#8 v1 DER for an Ed25519 key, before the 32-byte seed (what Go's
/// `x509.MarshalPKCS8PrivateKey` writes).
const PKCS8_ED25519_PREFIX: [u8; 16] = [
    0x30, 0x2e, 0x02, 0x01, 0x00, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x04, 0x22, 0x04, 0x20,
];

/// What `manifest` writes besides the artifacts.
pub struct ManifestOptions<'a> {
    pub version: &'a str,
    pub name: &'a str,
    pub url_prefix: &'a str,
    pub key: Option<&'a ed25519_dalek::SigningKey>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct OutManifest<'a> {
    schema_version: i64,
    version: &'a str,
    #[serde(skip_serializing_if = "str::is_empty")]
    name: &'a str,
    published_at: String,
    artifacts: Vec<OutArtifact>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct OutArtifact {
    url: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    platform: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    arch: String,
    size: u64,
    digest_algo: &'static str,
    digest: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    signature_algo: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    signature: Option<String>,
}

/// The manifest JSON (two-space indent, trailing newline) for `files`, each
/// digested with SHA-512 and, with a key, signed Ed25519ph.
pub fn write_manifest(opts: &ManifestOptions, files: &[PathBuf]) -> Result<String, String> {
    if opts.version.is_empty() {
        return Err("a release version is required".into());
    }
    if files.is_empty() {
        return Err("at least one artifact file is required".into());
    }
    let mut artifacts = Vec::new();
    for path in files {
        let (digest, size) = hash_file(path, "sha512")?;
        let filename = path
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| format!("{}: not a file name", path.display()))?;
        let signature = opts.key.map(|k| {
            let sig = k
                .sign_prehashed(
                    Prehashed(digest.as_slice().try_into().expect("sha512")),
                    None,
                )
                .expect("ed25519ph without context");
            STANDARD.encode(sig.to_bytes())
        });
        artifacts.push(OutArtifact {
            url: if opts.url_prefix.is_empty() {
                filename.to_string()
            } else {
                format!("{}/{filename}", opts.url_prefix.trim_end_matches('/'))
            },
            platform: infer_platform(filename),
            arch: infer_arch(filename),
            size,
            digest_algo: "sha512",
            digest: STANDARD.encode(&digest),
            signature_algo: signature.as_ref().map(|_| "ed25519ph"),
            signature,
        });
    }
    let m = OutManifest {
        schema_version: SCHEMA_VERSION,
        version: opts.version,
        name: opts.name,
        published_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        artifacts,
    };
    let mut out = serde_json::to_string_pretty(&m).map_err(|e| e.to_string())?;
    out.push('\n');
    Ok(out)
}

/// Re-checks every artifact in `manifest` against the file of the same name
/// in `dir` (size, digest, signature under `public_key`), as the app does
/// after a download. Returns one line per artifact.
pub fn verify_manifest(
    manifest: &[u8],
    dir: &Path,
    public_key: &[u8],
) -> Result<Vec<String>, String> {
    let m: Manifest =
        serde_json::from_slice(manifest).map_err(|e| format!("decode manifest: {e}"))?;
    if m.schema_version > SCHEMA_VERSION {
        return Err(format!(
            "manifest schemaVersion {} is newer than {SCHEMA_VERSION}",
            m.schema_version
        ));
    }
    if m.version.is_empty() {
        return Err("manifest is missing the version".into());
    }
    if m.artifacts.is_empty() {
        return Err("manifest has no artifacts".into());
    }
    let mut lines = Vec::new();
    for a in &m.artifacts {
        let name = if a.filename.is_empty() {
            filename_from_url(&a.url)
        } else {
            a.filename.clone()
        };
        let check = || -> Result<(), String> {
            let v = build_verification(a)?.ok_or("artifact has neither digest nor signature")?;
            let algo = if v.signature_algo == "ed25519ph" {
                "sha512"
            } else {
                v.digest_algo.as_str()
            };
            let (digest, size) = hash_file(&dir.join(&name), algo)?;
            if a.size != 0 && a.size as u64 != size {
                return Err(format!(
                    "size mismatch: manifest says {}, file is {size}",
                    a.size
                ));
            }
            run_verification(&digest, &v, public_key).map_err(|e| e.to_string())
        };
        check().map_err(|e| format!("{name}: {e}"))?;
        lines.push(format!("{name}: ok"));
    }
    Ok(lines)
}

/// An Ed25519 private key: a PKCS#8 PEM (`openssl genpkey -algorithm ed25519`), or
/// base64 of the raw 64-byte key or 32-byte seed.
pub fn load_signing_key(raw: &[u8]) -> Result<ed25519_dalek::SigningKey, String> {
    let seed: Option<[u8; 32]> = if let Some(der) = pem_decode(raw) {
        der.strip_prefix(&PKCS8_ED25519_PREFIX[..])
            .and_then(|s| s.try_into().ok())
    } else {
        let text = std::str::from_utf8(raw).unwrap_or_default().trim();
        STANDARD.decode(text).ok().and_then(|b| match b.len() {
            32 | 64 => b[..32].try_into().ok(),
            _ => None,
        })
    };
    seed.map(|s| ed25519_dalek::SigningKey::from_bytes(&s))
        .ok_or_else(|| "not a PKCS#8 PEM or base64 Ed25519 private key".into())
}

fn hash_file(path: &Path, algo: &str) -> Result<(Vec<u8>, u64), String> {
    let mut h = Hasher::for_algo(algo).map_err(|e| e.to_string())?;
    let mut f = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut buf = vec![0; 1 << 16];
    let mut size = 0;
    loop {
        let n = std::io::Read::read(&mut f, &mut buf)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        if n == 0 {
            return Ok((h.finish(), size));
        }
        h.update(&buf[..n]);
        size += n as u64;
    }
}

/// Go `GOOS` from names like `Recur-1.2.3-macos-universal.zip`, then from
/// the extension; empty matches every platform.
fn infer_platform(name: &str) -> String {
    let lower = name.to_lowercase();
    for (goos, re) in [
        (
            "darwin",
            r"(?:^|[^a-z0-9])(?:darwin|macos|osx|mac)(?:[^a-z0-9]|$)",
        ),
        (
            "windows",
            r"(?:^|[^a-z0-9])(?:windows|win64|win32|win)(?:[^a-z0-9]|$)",
        ),
        (
            "linux",
            r"(?:^|[^a-z0-9])(?:linux64|linux32|linux)(?:[^a-z0-9]|$)",
        ),
    ] {
        if regex::Regex::new(re)
            .expect("platform pattern")
            .is_match(&lower)
        {
            return goos.into();
        }
    }
    let ext = lower.rsplit_once('.').map(|(_, e)| e).unwrap_or_default();
    match ext {
        "exe" | "msi" | "msix" => "windows",
        "dmg" | "pkg" => "darwin",
        "appimage" | "deb" | "rpm" => "linux",
        _ => "",
    }
    .into()
}

/// Go `GOARCH` from the name; `universal` and unknown names match every arch.
fn infer_arch(name: &str) -> String {
    let lower = name.to_lowercase();
    // Ordered: `_` is a boundary, so `x86` would match inside `x86_64` first.
    for (goarch, re) in [
        (
            "amd64",
            r"(?:^|[^a-z0-9])(?:amd64|x86_64|x64)(?:[^a-z0-9]|$)",
        ),
        ("arm64", r"(?:^|[^a-z0-9])(?:arm64|aarch64)(?:[^a-z0-9]|$)"),
        (
            "386",
            r"(?:^|[^a-z0-9])(?:i386|386|x86|ia32)(?:[^a-z0-9]|$)",
        ),
    ] {
        if regex::Regex::new(re)
            .expect("arch pattern")
            .is_match(&lower)
        {
            return goarch.into();
        }
    }
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pem(label: &str, der: &[u8]) -> Vec<u8> {
        format!(
            "-----BEGIN {label}-----\n{}\n-----END {label}-----\n",
            STANDARD.encode(der)
        )
        .into_bytes()
    }

    /// What the release workflow does: sign a manifest with a PKCS#8 key,
    /// then check it with the PKIX public key the app pins.
    #[test]
    fn written_manifest_passes_the_app_verifier() {
        let seed = [7u8; 32];
        let key_pem = pem("PRIVATE KEY", &[&PKCS8_ED25519_PREFIX[..], &seed].concat());
        let key = load_signing_key(&key_pem).expect("pkcs8 pem");
        // SubjectPublicKeyInfo for Ed25519, then the raw key.
        let spki_prefix = [
            0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00,
        ];
        let pub_pem = pem(
            "PUBLIC KEY",
            &[&spki_prefix[..], key.verifying_key().as_bytes()].concat(),
        );

        let dir = tempfile::tempdir().unwrap();
        let zip = dir.path().join("RecurNext-1.2.3-macos-universal.zip");
        std::fs::write(&zip, b"release bytes").unwrap();
        let json = write_manifest(
            &ManifestOptions {
                version: "1.2.3",
                name: "Recur Next 1.2.3",
                url_prefix: "https://example.com/download/v1.2.3/",
                key: Some(&key),
            },
            std::slice::from_ref(&zip),
        )
        .unwrap();

        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        let a = &v["artifacts"][0];
        assert_eq!(v["schemaVersion"], 1);
        assert_eq!(
            a["url"],
            "https://example.com/download/v1.2.3/RecurNext-1.2.3-macos-universal.zip"
        );
        assert_eq!(
            (a["platform"].as_str(), a.get("arch")),
            (Some("darwin"), None)
        );
        assert_eq!(
            (a["digestAlgo"].as_str(), a["signatureAlgo"].as_str()),
            (Some("sha512"), Some("ed25519ph"))
        );

        let lines = verify_manifest(json.as_bytes(), dir.path(), &pub_pem).expect("verifies");
        assert_eq!(lines, ["RecurNext-1.2.3-macos-universal.zip: ok"]);

        std::fs::write(&zip, b"release bytez").unwrap();
        let err = verify_manifest(json.as_bytes(), dir.path(), &pub_pem).unwrap_err();
        assert!(err.contains("digest mismatch"), "{err}");

        let other = ed25519_dalek::SigningKey::from_bytes(&[9; 32]);
        std::fs::write(&zip, b"release bytes").unwrap();
        let err = verify_manifest(
            json.as_bytes(),
            dir.path(),
            other.verifying_key().as_bytes(),
        )
        .unwrap_err();
        assert!(err.contains("did not verify"), "{err}");
    }

    #[test]
    fn platform_and_arch_from_names() {
        assert_eq!(infer_platform("recur-next-1.0.0-linux-amd64.deb"), "linux");
        assert_eq!(infer_platform("App.dmg"), "darwin");
        assert_eq!(infer_platform("notes.zip"), "");
        assert_eq!(infer_arch("app-linux-x86_64.tar.gz"), "amd64");
        assert_eq!(infer_arch("app-darwin-aarch64.zip"), "arm64");
        assert_eq!(infer_arch("app-macos-universal.zip"), "");
    }

    #[test]
    fn signing_key_forms() {
        let seed = [3u8; 32];
        let want = ed25519_dalek::SigningKey::from_bytes(&seed);
        let raw64 = [&seed[..], want.verifying_key().as_bytes()].concat();
        for raw in [STANDARD.encode(seed), STANDARD.encode(&raw64)] {
            assert_eq!(load_signing_key(raw.as_bytes()).unwrap().to_bytes(), seed);
        }
        assert!(load_signing_key(b"nope").is_err());
    }
}
