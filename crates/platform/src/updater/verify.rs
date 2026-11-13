//! Digest and signature checks for a downloaded artifact. The trust root is
//! always the configured public key; a release cannot pick its own.

use digest::Digest;
use sha2::{Sha256, Sha512};

use super::{UpdaterError, Verification};

/// The streaming hash computed while downloading.
pub(super) enum Hasher {
    Sha256(Sha256),
    Sha512(Sha512),
}

impl Hasher {
    pub(super) fn for_algo(algo: &str) -> Result<Self, UpdaterError> {
        match algo {
            "" | "sha256" => Ok(Self::Sha256(Sha256::new())),
            "sha512" => Ok(Self::Sha512(Sha512::new())),
            other => Err(fail(format!("unknown digest algorithm {other:?}"))),
        }
    }

    pub(super) fn update(&mut self, data: &[u8]) {
        match self {
            Self::Sha256(h) => h.update(data),
            Self::Sha512(h) => h.update(data),
        }
    }

    pub(super) fn finish(self) -> Vec<u8> {
        match self {
            Self::Sha256(h) => h.finalize().to_vec(),
            Self::Sha512(h) => h.finalize().to_vec(),
        }
    }
}

fn fail(msg: impl std::fmt::Display) -> UpdaterError {
    UpdaterError::Failed(format!("updater: {msg}"))
}

/// Every present check must pass: a digest must match, and a signature must
/// verify under `config_key`. A signature without an algorithm, or without a
/// configured key, fails closed.
pub(super) fn run_verification(
    computed: &[u8],
    v: &Verification,
    config_key: &[u8],
) -> Result<(), UpdaterError> {
    if !v.digest.is_empty() && !constant_time_eq(computed, &v.digest) {
        return Err(fail("digest mismatch"));
    }
    if !v.signature.is_empty() && v.signature_algo.is_empty() {
        return Err(fail("signature present but signatureAlgo missing"));
    }
    if v.signature.is_empty() {
        return Ok(());
    }
    let algo = v.signature_algo.as_str();
    if !matches!(algo, "ed25519" | "ed25519ph") {
        return Err(fail(format!("unsupported signature algorithm {algo:?}")));
    }
    if config_key.is_empty() {
        return Err(fail("signature requires a public key but none configured"));
    }
    match algo {
        "ed25519" => {
            let key = parse_ed25519_public(config_key)?;
            let sig = ed25519_dalek::Signature::from_slice(&v.signature)
                .map_err(|_| fail("ed25519 signature did not verify"))?;
            // Raw Ed25519 signs the message, here the digest.
            ed25519_dalek::Verifier::verify(&key, computed, &sig)
                .map_err(|_| fail("ed25519 signature did not verify"))
        }
        _ => {
            let key = parse_ed25519_public(config_key)?;
            let prehash: [u8; 64] = computed.try_into().map_err(|_| {
                fail(format!(
                    "ed25519ph requires sha512 digest, got {} bytes",
                    computed.len()
                ))
            })?;
            let sig = ed25519_dalek::Signature::from_slice(&v.signature)
                .map_err(|e| fail(format!("ed25519ph signature did not verify: {e}")))?;
            key.verify_prehashed(Prehashed(prehash), None, &sig)
                .map_err(|e| fail(format!("ed25519ph signature did not verify: {e}")))
        }
    }
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// A finished SHA-512 handed to Ed25519ph, which takes the hash state as a
/// `Digest`. The updater already streamed the hash during download.
#[derive(Clone)]
pub(super) struct Prehashed(pub(super) [u8; 64]);

impl Default for Prehashed {
    fn default() -> Self {
        Self([0; 64])
    }
}

impl digest::HashMarker for Prehashed {}

impl digest::Update for Prehashed {
    fn update(&mut self, _data: &[u8]) {}
}

impl digest::OutputSizeUser for Prehashed {
    type OutputSize = digest::consts::U64;
}

impl digest::FixedOutput for Prehashed {
    fn finalize_into(self, out: &mut digest::Output<Self>) {
        out.copy_from_slice(&self.0);
    }
}

/// The DER `SubjectPublicKeyInfo` header of an Ed25519 key; the raw 32-byte key follows.
const ED25519_SPKI_PREFIX: [u8; 12] = [
    0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00,
];

/// A raw 32-byte key, or an Ed25519 `SubjectPublicKeyInfo` as DER or PEM.
fn parse_ed25519_public(raw: &[u8]) -> Result<ed25519_dalek::VerifyingKey, UpdaterError> {
    let der = pem_decode(raw);
    let der = der.as_deref().unwrap_or(raw);
    let key: [u8; 32] = der
        .strip_prefix(&ED25519_SPKI_PREFIX[..])
        .unwrap_or(der)
        .try_into()
        .map_err(|_| fail("ed25519 public key parse: not an Ed25519 key"))?;
    ed25519_dalek::VerifyingKey::from_bytes(&key)
        .map_err(|e| fail(format!("ed25519 public key parse: {e}")))
}

/// The first PEM block's bytes (leading text and `Key: value` headers are
/// skipped).
pub(super) fn pem_decode(raw: &[u8]) -> Option<Vec<u8>> {
    use base64::Engine;
    let text = std::str::from_utf8(raw).ok()?;
    let start = text.find("-----BEGIN ")?;
    let rest = &text[start..];
    let line_end = rest.find('\n')?;
    if !rest[..line_end].trim_end().ends_with("-----") {
        return None;
    }
    let body = &rest[line_end + 1..];
    let end = body.find("-----END ")?;
    let b64: String = body[..end]
        .lines()
        .filter(|l| !l.contains(':'))
        .flat_map(|l| l.chars().filter(|c| !c.is_whitespace()))
        .collect();
    base64::engine::general_purpose::STANDARD.decode(b64).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signature_without_algo_fails_closed() {
        let v = Verification {
            signature: b"sig-bytes".to_vec(),
            ..Verification::default()
        };
        assert!(run_verification(&[], &v, b"key").is_err());
    }

    #[test]
    fn shipped_public_key_parses() {
        let pem = include_bytes!("../../../../build/updater.key.pub");
        parse_ed25519_public(pem).expect("shipped updater key is an ed25519 PKIX PEM");
        // A P-256 SubjectPublicKeyInfo header is not an Ed25519 key.
        let p256 = [&[0x30, 0x59, 0x30, 0x13, 0x06, 0x07][..], &[0; 85]].concat();
        let err = parse_ed25519_public(&p256).expect_err("not ed25519");
        assert!(err.to_string().contains("not an Ed25519 key"), "{err}");
    }

    #[test]
    fn digest_only_and_unknown_algorithms() {
        let digest = Sha256::digest(b"x").to_vec();
        let ok = Verification {
            digest_algo: "sha256".into(),
            digest: digest.clone(),
            ..Verification::default()
        };
        run_verification(&digest, &ok, &[]).expect("digest only");
        let err = run_verification(&[0; 32], &ok, &[]).expect_err("mismatch");
        assert!(err.to_string().contains("digest mismatch"));
        let bad_algo = Verification {
            signature_algo: "rsa".into(),
            signature: vec![1],
            ..Verification::default()
        };
        let err = run_verification(&digest, &bad_algo, b"k").expect_err("unsupported");
        assert!(err.to_string().contains("unsupported signature algorithm"));
        assert!(Hasher::for_algo("md5").is_err());
    }
}
