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
    if !matches!(algo, "ed25519" | "ed25519ph" | "ecdsa-p256") {
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
        "ed25519ph" => {
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
        _ => {
            let key = parse_ecdsa_p256_public(config_key)?;
            let sig = split_ecdsa_sig(&v.signature)?;
            p256::ecdsa::signature::hazmat::PrehashVerifier::verify_prehash(&key, computed, &sig)
                .map_err(|_| fail("ecdsa-p256 signature did not verify"))
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

const OID_ED25519: &str = "1.3.101.112";
const OID_EC_PUBLIC_KEY: &str = "1.2.840.10045.2.1";
const OID_RSA: &str = "1.2.840.113549.1.1.1";
const OID_P256: &str = "1.2.840.10045.3.1.7";

/// The key type for an OID, used in wrong-type errors.
fn key_type(oid: &str) -> &'static str {
    match oid {
        OID_ED25519 => "Ed25519",
        OID_EC_PUBLIC_KEY => "ECDSA",
        OID_RSA => "RSA",
        _ => "unknown",
    }
}

/// The subject public key info from DER, or from the first PEM block.
fn parse_spki(raw: &[u8]) -> Result<(String, Option<String>, Vec<u8>), String> {
    let der = pem_decode(raw).unwrap_or_else(|| raw.to_vec());
    let spki =
        spki::SubjectPublicKeyInfoRef::try_from(der.as_slice()).map_err(|e| e.to_string())?;
    let oid = spki.algorithm.oid.to_string();
    let params = spki.algorithm.parameters_oid().ok().map(|o| o.to_string());
    let key = spki
        .subject_public_key
        .as_bytes()
        .ok_or("public key bit string is not byte aligned")?
        .to_vec();
    Ok((oid, params, key))
}

fn parse_ed25519_public(raw: &[u8]) -> Result<ed25519_dalek::VerifyingKey, UpdaterError> {
    let key: [u8; 32] = if let Ok(bytes) = <[u8; 32]>::try_from(raw) {
        bytes
    } else {
        let (oid, _, key) =
            parse_spki(raw).map_err(|e| fail(format!("ed25519 public key parse: {e}")))?;
        if oid != OID_ED25519 {
            return Err(fail(format!(
                "ed25519 public key has wrong type {}",
                key_type(&oid)
            )));
        }
        key.as_slice()
            .try_into()
            .map_err(|_| fail("ed25519 public key parse: bad key length"))?
    };
    ed25519_dalek::VerifyingKey::from_bytes(&key)
        .map_err(|e| fail(format!("ed25519 public key parse: {e}")))
}

fn parse_ecdsa_p256_public(raw: &[u8]) -> Result<p256::ecdsa::VerifyingKey, UpdaterError> {
    let (oid, params, key) =
        parse_spki(raw).map_err(|e| fail(format!("ecdsa public key parse: {e}")))?;
    if oid != OID_EC_PUBLIC_KEY {
        return Err(fail(format!(
            "ecdsa public key has wrong type {}",
            key_type(&oid)
        )));
    }
    let curve = match params.as_deref() {
        Some(OID_P256) => None,
        Some("1.3.132.0.33") => Some("P-224"),
        Some("1.3.132.0.34") => Some("P-384"),
        Some("1.3.132.0.35") => Some("P-521"),
        _ => {
            return Err(fail(
                "ecdsa public key parse: x509: unsupported elliptic curve",
            ));
        }
    };
    if let Some(name) = curve {
        return Err(fail(format!("ecdsa-p256 requires P-256 key, got {name}")));
    }
    p256::ecdsa::VerifyingKey::from_sec1_bytes(&key)
        .map_err(|e| fail(format!("ecdsa public key parse: {e}")))
}

/// Raw `r||s` (64 bytes) or an ASN.1 DER SEQUENCE with nothing after it.
fn split_ecdsa_sig(sig: &[u8]) -> Result<p256::ecdsa::Signature, UpdaterError> {
    if sig.len() == 64 {
        return p256::ecdsa::Signature::from_slice(sig)
            .map_err(|_| fail("ecdsa-p256 signature did not verify"));
    }
    let total = der_sequence_len(sig)
        .ok_or_else(|| fail("ecdsa signature format unrecognised: asn1: syntax error"))?;
    if total < sig.len() {
        return Err(fail("ecdsa signature has trailing data"));
    }
    p256::ecdsa::Signature::from_der(sig)
        .map_err(|e| fail(format!("ecdsa signature format unrecognised: {e}")))
}

/// The encoded length of a leading DER SEQUENCE, or `None` when `b` does not
/// start with a complete one.
fn der_sequence_len(b: &[u8]) -> Option<usize> {
    if b.first() != Some(&0x30) {
        return None;
    }
    let first = *b.get(1)? as usize;
    let (len, header) = if first < 0x80 {
        (first, 2)
    } else {
        let n = first & 0x7f;
        if n == 0 || n > 2 || b.len() < 2 + n {
            return None;
        }
        let len = b[2..2 + n]
            .iter()
            .fold(0usize, |acc, &x| acc << 8 | x as usize);
        (len, 2 + n)
    };
    let total = header + len;
    (total <= b.len()).then_some(total)
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
        let err = parse_ecdsa_p256_public(pem).expect_err("not ecdsa");
        assert!(err.to_string().contains("wrong type Ed25519"), "{err}");
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

    #[test]
    fn der_lengths() {
        assert_eq!(der_sequence_len(&[0x30, 0x02, 0x01, 0x00]), Some(4));
        assert_eq!(der_sequence_len(&[0x30, 0x02, 0x01, 0x00, 0xde]), Some(4));
        assert_eq!(der_sequence_len(&[0x30, 0x05, 0x01]), None);
        assert_eq!(der_sequence_len(&[0x02, 0x01]), None);
        assert_eq!(der_sequence_len(&[0x30, 0x81, 0x01, 0x00]), Some(4));
    }
}
