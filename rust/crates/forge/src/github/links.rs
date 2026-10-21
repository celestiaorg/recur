//! GitHub web links the PR views show (`lib/comment-permalink.ts`, the
//! GitHub half of `lib/pr-checks.ts`).

use recur_domain::PrComment;
use url::Url;

use super::checks::checks_tab_url;
use crate::view::is_pending_check;

fn page_base(pr_url: &str) -> String {
    let s = pr_url.split('#').next().unwrap_or_default();
    s.trim_end_matches('/').trim().to_owned()
}

/// Permalink for a conversation row: the stored URL, else a fragment
/// derived from the Recur row id.
pub fn comment_permalink(pr_url: &str, c: &PrComment) -> String {
    let stored = c.url.trim();
    if !stored.is_empty() {
        return stored.to_owned();
    }
    let base = page_base(pr_url);
    if base.is_empty() {
        return String::new();
    }
    if c.kind == "description" || c.id == "gh-description" {
        return base;
    }
    let id = c.id.as_str();
    let node = [
        "gh-inline-",
        "gh-comment-",
        "gh-review-action-",
        "gh-review-",
    ]
    .iter()
    .find_map(|p| id.strip_prefix(p))
    .unwrap_or(id);
    let db = node_database_id(node);
    if db.is_empty() {
        return String::new();
    }
    if id.starts_with("gh-inline-") {
        format!("{base}#discussion_r{db}")
    } else if id.starts_with("gh-comment-") {
        format!("{base}#issuecomment-{db}")
    } else if id.starts_with("gh-review-") {
        format!("{base}#pullrequestreview-{db}")
    } else {
        String::new()
    }
}

/// The database id in a numeric REST id or a GraphQL node id (`PRR_…` or
/// legacy base64 `NNN:TypeNNN`), as the UI derives it; empty when none.
pub fn node_database_id(id: &str) -> String {
    let raw = id.trim();
    if raw.is_empty() {
        return String::new();
    }
    if raw.bytes().all(|b| b.is_ascii_digit()) {
        return raw.to_owned();
    }
    if let Some(split) = raw.find('_')
        && split > 0
        && split <= 8
        && raw[..split].bytes().all(|b| b.is_ascii_uppercase())
    {
        let Some(bytes) = decode_b64(&raw[split + 1..]).filter(|b| b.len() >= 5) else {
            return String::new();
        };
        let n = big_decimal(&bytes[4..]);
        return if n == "0" { String::new() } else { n };
    }
    let Some(bytes) = decode_b64(raw) else {
        return String::new();
    };
    if bytes.iter().any(|&b| !(32..=126).contains(&b)) {
        return String::new();
    }
    let text = String::from_utf8_lossy(&bytes);
    let Some(i) = text.find(':') else {
        return String::new();
    };
    let rest = &text[i + 1..];
    let letters = rest.bytes().take_while(u8::is_ascii_alphabetic).count();
    let digits = &rest[letters..];
    if letters == 0 || digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return String::new();
    }
    digits.to_owned()
}

/// `atob` over URL-safe or standard base64 (forgiving: trailing bits ignored).
fn decode_b64(s: &str) -> Option<Vec<u8>> {
    use base64::Engine as _;
    use base64::engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig};
    const FORGIVING: GeneralPurpose = GeneralPurpose::new(
        &base64::alphabet::STANDARD,
        GeneralPurposeConfig::new()
            .with_decode_allow_trailing_bits(true)
            .with_decode_padding_mode(DecodePaddingMode::Indifferent),
    );
    let mut padded = s.replace('-', "+").replace('_', "/");
    let m = padded.len() % 4;
    if m != 0 {
        padded.push_str(&"=".repeat(4 - m));
    }
    FORGIVING.decode(padded).ok()
}

/// Big-endian bytes as an unbounded decimal string.
fn big_decimal(bytes: &[u8]) -> String {
    let mut num: Vec<u8> = bytes.to_vec();
    let mut digits = Vec::new();
    while num.iter().any(|&b| b != 0) {
        let mut rem = 0u32;
        for b in &mut num {
            let cur = (rem << 8) | u32::from(*b);
            *b = (cur / 10) as u8;
            rem = cur % 10;
        }
        digits.push(b'0' + rem as u8);
    }
    if digits.is_empty() {
        return "0".to_owned();
    }
    digits.reverse();
    String::from_utf8(digits).unwrap_or_default()
}

fn repo_url(pr_url: &str) -> String {
    let raw = page_base(pr_url);
    if raw.is_empty() {
        return String::new();
    }
    let Ok(u) = Url::parse(&raw) else {
        return String::new();
    };
    let parts: Vec<&str> = u.path().split('/').filter(|s| !s.is_empty()).collect();
    if parts.len() < 2 {
        return String::new();
    }
    format!(
        "{}/{}/{}",
        u.origin().ascii_serialization(),
        parts[0],
        parts[1]
    )
}

/// Commit page for a sha on the PR's repo.
pub fn commit_url(pr_url: &str, sha: &str) -> String {
    let oid = sha.trim();
    if oid.is_empty() {
        return String::new();
    }
    let repo = repo_url(pr_url);
    if repo.is_empty() {
        return String::new();
    }
    format!("{repo}/commit/{oid}")
}

/// `encodeURIComponent`.
fn encode_component(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// File view at `git_ref` on the PR's repo, with `#L<line>` when positive.
pub fn blob_url(pr_url: &str, git_ref: &str, path: &str, line: i64) -> String {
    let r = git_ref.trim();
    let p = path.trim_start_matches('/');
    if r.is_empty() || p.is_empty() {
        return String::new();
    }
    let repo = repo_url(pr_url);
    if repo.is_empty() {
        return String::new();
    }
    let path: Vec<String> = p.split('/').map(encode_component).collect();
    let anchor = if line > 0 {
        format!("#L{line}")
    } else {
        String::new()
    };
    format!("{repo}/blob/{r}/{}{anchor}", path.join("/"))
}

fn is_actions_url(url: &str) -> bool {
    match Url::parse(url) {
        Ok(u) => u.path().contains("/actions/runs/"),
        Err(_) => url.contains("/actions/runs/"),
    }
}

/// Link for a check row. Pending Actions job pages do not show queued runs,
/// so pending rows (and rows without a link) open the PR's Checks tab; a
/// check-run permalink gains `pr=<n>`.
pub fn check_run_url(url: &str, pr_url: &str, pr_number: i64, status: &str) -> String {
    let tab = checks_tab_url(pr_url);
    let pending = is_pending_check(status);
    let raw = url.trim();
    if pending && (raw.is_empty() || is_actions_url(raw)) {
        return if tab.is_empty() { raw.to_owned() } else { tab };
    }
    if raw.is_empty() {
        return tab;
    }
    if pr_number <= 0 {
        return raw.to_owned();
    }
    let Ok(mut u) = Url::parse(raw) else {
        return raw.to_owned();
    };
    if !u.query_pairs().any(|(k, _)| k == "pr") && !u.path().contains("/pull/") {
        u.query_pairs_mut()
            .append_pair("pr", &pr_number.to_string());
    }
    u.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    const PR: &str = "https://github.com/acme/app/pull/12";

    fn c(id: &str, kind: &str, url: &str) -> PrComment {
        PrComment {
            id: id.into(),
            kind: kind.into(),
            url: url.into(),
            ..Default::default()
        }
    }

    fn encode_node(prefix: &str, type_id: u32, db: u32) -> String {
        use base64::Engine as _;
        let mut buf = type_id.to_be_bytes().to_vec();
        buf.extend(db.to_be_bytes());
        format!(
            "{prefix}_{}",
            base64::engine::general_purpose::STANDARD_NO_PAD.encode(buf)
        )
    }

    #[test]
    fn permalinks() {
        assert_eq!(
            comment_permalink(
                PR,
                &c(
                    "c1",
                    "",
                    "https://github.com/acme/app/pull/12#issuecomment-9"
                )
            ),
            "https://github.com/acme/app/pull/12#issuecomment-9"
        );
        assert_eq!(
            comment_permalink(
                PR,
                &c(
                    "gh-description",
                    "description",
                    &format!("{PR}#issue-5215972950")
                )
            ),
            format!("{PR}#issue-5215972950")
        );
        assert_eq!(
            comment_permalink(PR, &c("gh-description", "description", "")),
            PR
        );
        assert_eq!(
            comment_permalink(PR, &c("gh-inline-99", "", "")),
            format!("{PR}#discussion_r99")
        );
        assert_eq!(
            comment_permalink(PR, &c("gh-comment-8", "", "")),
            format!("{PR}#issuecomment-8")
        );
        assert_eq!(
            comment_permalink(PR, &c("gh-review-3", "", "")),
            format!("{PR}#pullrequestreview-3")
        );
        assert_eq!(
            comment_permalink(PR, &c("gh-review-action-3", "", "")),
            format!("{PR}#pullrequestreview-3")
        );
        let id = encode_node("PRR", 8, 3864739160);
        assert_eq!(
            comment_permalink(PR, &c(&format!("gh-review-{id}"), "", "")),
            format!("{PR}#pullrequestreview-3864739160")
        );
        assert_eq!(comment_permalink("", &c("c1", "", "")), "");
        assert_eq!(comment_permalink(PR, &c("c1", "", "")), "");
        assert_eq!(comment_permalink(PR, &c("gh-review-PRR_c", "", "")), "");
    }

    #[test]
    fn node_ids_like_the_ui() {
        use base64::Engine as _;
        assert_eq!(node_database_id("99"), "99");
        assert_eq!(
            node_database_id(&encode_node("PRR", 8, 3864739160)),
            "3864739160"
        );
        let legacy = base64::engine::general_purpose::STANDARD.encode("012:PullRequestReview42");
        assert_eq!(node_database_id(&legacy), "42");
        assert_eq!(node_database_id("PRR_c"), "");
        assert_eq!(big_decimal(&[0xff; 12]), "79228162514264337593543950335");
    }

    #[test]
    fn commit_and_blob_urls() {
        assert_eq!(
            commit_url(PR, "a3f73e2abcdef"),
            "https://github.com/acme/app/commit/a3f73e2abcdef"
        );
        assert_eq!(
            commit_url("https://github.com/acme/app/pull/12/#event-1", "abc1234"),
            "https://github.com/acme/app/commit/abc1234"
        );
        assert_eq!(commit_url("", "abc1234"), "");
        assert_eq!(commit_url(PR, ""), "");
        assert_eq!(commit_url("not a url", "abc1234"), "");
        assert_eq!(
            blob_url(PR, "abc1234", "src/a b.ts", 7),
            "https://github.com/acme/app/blob/abc1234/src/a%20b.ts#L7"
        );
        assert_eq!(blob_url("", "abc1234", "a.ts", 0), "");
        assert_eq!(blob_url(PR, "", "a.ts", 0), "");
        assert_eq!(blob_url(PR, "abc1234", "", 0), "");
    }

    #[test]
    fn check_run_links() {
        let pr = "https://github.com/acme/app/pull/11";
        assert_eq!(
            check_run_url(
                "https://github.com/acme/app/actions/runs/1/job/2",
                pr,
                11,
                "pending"
            ),
            "https://github.com/acme/app/pull/11/checks"
        );
        assert_eq!(
            check_run_url("", pr, 11, "queued"),
            "https://github.com/acme/app/pull/11/checks"
        );
        assert_eq!(
            check_run_url("https://github.com/acme/app/runs/99", pr, 11, "pending"),
            "https://github.com/acme/app/runs/99?pr=11"
        );
        assert_eq!(
            check_run_url(
                "https://github.com/acme/app/actions/runs/1/job/2",
                "",
                11,
                "pending"
            ),
            "https://github.com/acme/app/actions/runs/1/job/2"
        );
        assert_eq!(
            checks_tab_url("https://github.com/acme/app/pull/11/"),
            "https://github.com/acme/app/pull/11/checks"
        );
    }
}
