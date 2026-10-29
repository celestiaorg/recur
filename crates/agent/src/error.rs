//! Provider errors and the transient-vs-genuine failure classification (D56).
//!
//! An agent turn fails for two very different reasons. A GENUINE failure (the
//! engine could not commit, a rail was violated, the agent produced nothing
//! usable) would just fail again. A TRANSIENT failure (spend limit, rate
//! limit, provider 5xx, a dropped connection) says nothing about the work;
//! the loop simply cannot continue right now, so the UI offers Retry from the
//! checkpoint.
//!
//! Classification keys on SUBSTRINGS of the provider's own message rather
//! than error codes: the CLIs surface these as prose on stderr or as an error
//! subtype, and the wording varies by build and plan. Unknown failures are
//! NOT retryable.

use std::error::Error;

use crate::event::TurnResult;

/// Matched case-insensitively against a failure's full text (error + result
/// subtype + text + stderr tail). Each names a condition expected to clear on
/// its own or after the user changes a limit.
pub const TRANSIENT_MARKERS: &[&str] = &[
    // Spend / usage / quota ceilings.
    "spend limit",
    "monthly spend",
    "usage limit",
    "usage_limit",
    "rate limit",
    "rate_limit",
    "quota",
    "credit balance",
    "insufficient credit",
    "billing",
    "/usage-credits",
    "upgrade your plan",
    "too many requests",
    "resource_exhausted", // Cursor run status frame, e.g. "[resource_exhausted] Error"
    // Provider-side transients.
    "overloaded",
    "overloaded_error",
    "api_error",
    "internal server error",
    "service unavailable",
    "temporarily unavailable",
    "bad gateway",
    "gateway timeout",
    "server error",
    "529",
    "503",
    "502",
    "504",
    "429",
    // Transport / process transients.
    "connection reset",
    "connection refused",
    "broken pipe",
    "eof",
    "i/o timeout",
    "timeout",
    "timed out",
    "tls handshake",
    "no such host",
    "network is unreachable",
    // Cursor Send treated a dropped stream as success, then Run invents this.
    "ended without a result",
    // Codex ChatGPT session expired; Retry works after signing in again.
    "failed to refresh token",
    "login expired",
];

/// Keep in sync with frontend `CODEX_LOGIN_EXPIRED`.
pub const CODEX_LOGIN_EXPIRED: &str =
    "Codex login expired: sign in with ChatGPT in Settings → Providers → Codex, then retry.";

/// How a provider failed, as far as the provider can tell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    /// The turn's cancellation token fired.
    Canceled,
    /// The provider knows this clears on its own (Cursor Connect codes).
    Transient,
    Auth,
    /// The provider cannot run (not installed, not configured, bridge down).
    Unavailable,
    Failed,
}

/// A failed turn. `partial` is whatever result the turn produced before it
/// failed; its stderr, subtype and text feed [`classify`] and
/// [`failure_reason`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct ProviderError {
    pub kind: ErrorKind,
    pub message: String,
    pub partial: Box<TurnResult>,
}

impl ProviderError {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            partial: Box::default(),
        }
    }

    /// Cancellation (`context canceled`).
    pub fn canceled() -> Self {
        Self::new(ErrorKind::Canceled, "context canceled")
    }

    pub fn failed(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Failed, message)
    }

    pub fn with_partial(mut self, partial: TurnResult) -> Self {
        self.partial = Box::new(partial);
        self
    }

    pub fn is_canceled(&self) -> bool {
        self.kind == ErrorKind::Canceled
    }

    /// Whether Retry is worth offering: the provider-set kind first, then the
    /// shared marker list over the error and its partial result.
    pub fn transient(&self) -> bool {
        classify(Some(self), &self.partial).1
    }
}

/// Decides whether a failed turn is transient (retryable from the checkpoint)
/// and returns the best human-readable reason. The reason prefers the
/// provider's own words over the internal subtype. `err` may be any error; a
/// [`ProviderError`] of kind [`ErrorKind::Transient`] anywhere in its source
/// chain is transient.
pub fn classify(err: Option<&(dyn Error + 'static)>, res: &TurnResult) -> (String, bool) {
    let reason = failure_reason(err, res);
    let mut cur = err;
    while let Some(e) = cur {
        if e.downcast_ref::<ProviderError>()
            .is_some_and(|p| p.kind == ErrorKind::Transient)
        {
            return (reason, true);
        }
        cur = e.source();
    }
    let hay = [err_text(err).as_str(), &res.subtype, &res.text, &res.stderr]
        .join("\n")
        .to_lowercase();
    if !known_agent_error(&hay).is_empty() {
        return (reason, true);
    }
    let transient = TRANSIENT_MARKERS.iter().any(|m| hay.contains(m));
    (reason, transient)
}

/// Whether `s` names a transient condition (scanning a plain phase error).
pub fn has_transient_marker(s: &str) -> bool {
    let hay = s.to_lowercase();
    TRANSIENT_MARKERS.iter().any(|m| hay.contains(m))
}

/// The message shown for a failed turn: the provider's own text (stderr tail,
/// then the error, then the result text), falling back to the subtype.
pub fn failure_reason(err: Option<&(dyn Error + 'static)>, res: &TurnResult) -> String {
    let text = err_text(err);
    let hay = [res.stderr.as_str(), &text, &res.text].join("\n");
    let known = known_agent_error(&hay);
    if !known.is_empty() {
        return known.to_owned();
    }
    let s = first_line(&res.stderr);
    if !s.is_empty() {
        return tidy(&s);
    }
    // Prefer the transport error over the result text: when the Cursor
    // bridge dies mid-turn, text is often the last assistant prose while the
    // error carries the real failure.
    if err.is_some() {
        let s = strip_vt(&text);
        let s = s.trim();
        if !s.is_empty() {
            return tidy(s);
        }
    }
    if res.is_error {
        let s = first_line(&res.text);
        if !s.is_empty() {
            return tidy(&s);
        }
    }
    if !res.subtype.is_empty() {
        return res.subtype.clone();
    }
    "the agent turn failed".to_owned()
}

/// Maps CLI dumps Recur recognizes onto a complete sentence; empty otherwise.
// simplify: one Codex auth phrase; add rows if another dump needs a sentence.
pub fn known_agent_error(s: &str) -> &'static str {
    let low = strip_vt(s).to_lowercase();
    if low.contains("failed to refresh token")
        || (low.contains("codex_login") && low.contains("401"))
    {
        return CODEX_LOGIN_EXPIRED;
    }
    ""
}

/// Strips terminal escapes and a CLI log prefix (`2026-…Z ERROR codex::x - `)
/// from one error line, dropping a trailing JSON dump after such a prefix.
pub fn tidy(s: &str) -> String {
    let stripped = strip_vt(s);
    let s = stripped.trim();
    let t = strip_cli_log_prefix(s);
    if t == s {
        return s.to_owned();
    }
    match t.find('{') {
        Some(i) if i > 0 => t[..i].trim().to_owned(),
        _ => t.to_owned(),
    }
}

fn strip_cli_log_prefix(s: &str) -> &str {
    match s.find(" - ") {
        Some(i) if i > 0 && cli_log_head(&s[..i]) => s[i + 3..].trim(),
        _ => s,
    }
}

fn cli_log_head(head: &str) -> bool {
    let u = format!(" {} ", head.to_uppercase());
    if ![
        " ERROR ",
        " WARN ",
        " WARNING ",
        " INFO ",
        " DEBUG ",
        " TRACE ",
    ]
    .iter()
    .any(|l| u.contains(l))
    {
        return false;
    }
    head.contains("::") || (head.contains('T') && head.contains('Z'))
}

/// The first non-empty line, trimmed and capped at 300 bytes, so a
/// multi-page stderr dump does not become a status line.
pub fn first_line(s: &str) -> String {
    const MAX: usize = 300;
    let s = strip_vt(s);
    for ln in s.split('\n') {
        let ln = ln.trim();
        if ln.is_empty() {
            continue;
        }
        if ln.len() > MAX {
            return format!("{}…", &ln[..ln.floor_char_boundary(MAX)]);
        }
        return ln.to_owned();
    }
    String::new()
}

fn err_text(err: Option<&(dyn Error + 'static)>) -> String {
    err.map(|e| e.to_string()).unwrap_or_default()
}

/// Drops CSI/OSC/DCS and other C0 controls except newline, tab and CR.
fn strip_vt(s: &str) -> String {
    let p = s.as_bytes();
    let mut out = Vec::with_capacity(p.len());
    let mut i = 0;
    while i < p.len() {
        let c = p[i];
        match c {
            0x1b => i = skip_esc(p, i),
            0x9b => i = skip_until_final(p, i + 1),
            0x9d => i = skip_osc(p, i + 1),
            b'\n' | b'\t' | b'\r' => {
                out.push(c);
                i += 1;
            }
            _ if c < 0x20 || c == 0x7f => i += 1,
            _ if c < 0x80 => {
                out.push(c);
                i += 1;
            }
            _ => match decode_rune(&p[i..]) {
                Some(n) => {
                    out.extend_from_slice(&p[i..i + n]);
                    i += n;
                }
                None => i += 1,
            },
        }
    }
    // Only ASCII bytes and whole decoded runes were copied.
    String::from_utf8(out).unwrap_or_default()
}

/// The byte width of the valid rune at the start of `p`, if any.
fn decode_rune(p: &[u8]) -> Option<usize> {
    let end = p.len().min(4);
    let valid = match std::str::from_utf8(&p[..end]) {
        Ok(s) => s,
        Err(e) => std::str::from_utf8(&p[..e.valid_up_to()]).ok()?,
    };
    valid.chars().next().map(char::len_utf8)
}

fn skip_esc(p: &[u8], i: usize) -> usize {
    if i + 1 >= p.len() {
        return p.len();
    }
    match p[i + 1] {
        b'[' => skip_until_final(p, i + 2),
        b']' => skip_osc(p, i + 2),
        b'P' | b'X' | b'^' | b'_' => skip_st(p, i + 2),
        b'(' if i + 2 < p.len() => i + 3,
        b'(' => p.len(),
        _ => i + 2,
    }
}

fn skip_until_final(p: &[u8], mut i: usize) -> usize {
    while i < p.len() {
        if (0x40..=0x7e).contains(&p[i]) {
            return i + 1;
        }
        i += 1;
    }
    i
}

fn skip_osc(p: &[u8], mut i: usize) -> usize {
    while i < p.len() {
        if p[i] == 0x07 {
            return i + 1;
        }
        if p[i] == 0x1b && i + 1 < p.len() && p[i + 1] == b'\\' {
            return i + 2;
        }
        i += 1;
    }
    i
}

fn skip_st(p: &[u8], mut i: usize) -> usize {
    while i < p.len() {
        if p[i] == 0x1b && i + 1 < p.len() && p[i + 1] == b'\\' {
            return i + 2;
        }
        i += 1;
    }
    i
}

#[cfg(test)]
mod tests {
    use super::*;

    fn res(stderr: &str) -> TurnResult {
        TurnResult {
            stderr: stderr.into(),
            ..Default::default()
        }
    }

    #[test]
    fn provider_kind_wins_over_markers() {
        let e = ProviderError::new(
            ErrorKind::Transient,
            "cursor bridge: [internal] internal error",
        );
        let (reason, transient) = classify(Some(&e), &TurnResult::default());
        assert!(transient);
        assert_eq!(reason, "cursor bridge: [internal] internal error");
        assert!(e.transient());
        assert!(!ProviderError::failed("compile error").transient());
    }

    #[test]
    fn spend_limit_on_stderr_is_transient_with_provider_words() {
        let (reason, transient) = classify(
            None,
            &res("\n  You've hit your org's monthly spend limit\nmore"),
        );
        assert!(transient);
        assert_eq!(reason, "You've hit your org's monthly spend limit");
    }

    #[test]
    fn codex_login_is_one_sentence() {
        let r = res("2026-01-01T00:00:00Z ERROR codex_login: refresh 401 {\"a\":1}");
        assert_eq!(classify(None, &r), (CODEX_LOGIN_EXPIRED.to_owned(), true));
    }

    #[test]
    fn reason_fallbacks() {
        let e = ProviderError::failed("\u{1b}[31mspawn claude: ENOENT\u{1b}[0m");
        let r = TurnResult {
            text: "I'll check".into(),
            ..Default::default()
        };
        assert_eq!(failure_reason(Some(&e), &r), "spawn claude: ENOENT");
        let r = TurnResult {
            is_error: true,
            text: "bad thing\nsecond".into(),
            subtype: "error_during_execution".into(),
            ..Default::default()
        };
        assert_eq!(failure_reason(None, &r), "bad thing");
        let r = TurnResult {
            subtype: "error_max_turns".into(),
            ..Default::default()
        };
        assert_eq!(classify(None, &r), ("error_max_turns".to_owned(), false));
        assert_eq!(
            failure_reason(None, &TurnResult::default()),
            "the agent turn failed"
        );
    }

    #[test]
    fn tidy_strips_log_prefix_and_json() {
        assert_eq!(
            tidy("2026-05-01T10:00:00.1Z ERROR codex_core::exec - stream failed {\"x\":1}"),
            "stream failed"
        );
        assert_eq!(tidy("plain - message {x}"), "plain - message {x}");
        assert_eq!(tidy("WARN app::mod - {only json}"), "{only json}");
    }

    #[test]
    fn first_line_caps_bytes() {
        let long = "é".repeat(200);
        let got = first_line(&long);
        assert!(got.ends_with('…'));
        assert_eq!(got, format!("{}…", "é".repeat(150)));
        assert_eq!(first_line("\n\n  "), "");
    }

    #[test]
    fn strip_vt_matches_go() {
        assert_eq!(
            strip_vt("a\u{1b}[1;31mb\u{1b}]0;t\u{7}c\u{1b}Pq\u{1b}\\d\u{1b}(Be"),
            "abcde"
        );
        assert_eq!(strip_vt("x\u{0}\u{7f}\ty\r\n"), "x\ty\r\n");
        assert_eq!(strip_vt("\u{1b}é!"), "!");
    }

    #[test]
    fn source_chain_is_walked() {
        #[derive(Debug, thiserror::Error)]
        #[error("phase failed: {0}")]
        struct Wrap(#[source] ProviderError);
        let w = Wrap(ProviderError::new(ErrorKind::Transient, "blip"));
        assert!(classify(Some(&w), &TurnResult::default()).1);
        assert!(has_transient_marker("dial tcp: I/O TIMEOUT"));
        assert!(!has_transient_marker("merge conflict"));
    }
}
