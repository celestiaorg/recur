//! `/skill-name` host activation (Go `skill_mentions.go`): each mention's
//! SKILL.md body becomes a `<skill_content>` block ahead of the user text,
//! so every provider gets the instructions without native slash expansion.
//! Also the skill-guidance and comment-hygiene overlays the router adds to
//! every turn that can use them, and `join_append_prompt`.

use std::sync::LazyLock;
use std::time::Duration;

use recur_agent::TurnRequest;
use recur_agent_tools::names::mcp as tool;
use recur_domain::AgentSkill;
use regex::Regex;

use crate::deadline::deadline_token;

/// Skill name candidates; boundaries are checked by hand (no lookaround).
/// Keep in sync with the frontend `SKILL_SLASH_RE`.
static SKILL_SLASH_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"/([a-z][a-z0-9-]*)").expect("skill slash pattern"));

const MAX_ACTIVATED_SKILLS: usize = 8;
pub(crate) const MAX_SKILL_BODY_BYTES: usize = 100_000;
/// Bounds the skill catalog walk.
pub const SKILLS_CATALOG_TIMEOUT: Duration = Duration::from_secs(2);
/// Bounds one skill activation.
pub const SKILL_ACTIVATE_TIMEOUT: Duration = Duration::from_secs(2);

/// A short pointer to the skills tools (the catalog is not inlined).
pub const SKILLS_TOOLS_APPEND_PROMPT: &str = "The following skills provide specialized instructions for specific tasks. When a task matches a skill's description, call mcp__recur__list_skills for the catalog, then mcp__recur__activate_skill with the skill's name to load its full instructions. If the user message already includes <skill_content>, follow that and do not re-activate. For questions about using Recur itself, activate recur-help.";

/// Marks the comment-hygiene overlay as already present.
pub const COMMENT_HYGIENE_NEEDLE: &str = "changelogs, reasoning, or internal dialogue";

/// Always on for write-capable turns: Recur does not replace the providers'
/// system prompts, so this overlay is what reaches the models.
pub const COMMENT_HYGIENE_APPEND_PROMPT: &str = "Code that explains itself needs no comments. Comment only when the code is genuinely complex or the intent would not be obvious to a future reader. Do not put notes, changelogs, reasoning, or internal dialogue in code comments: not line comments, block comments, or new docstrings. Restating the next line is not explaining intent. No \"changed\", \"updated to\", \"previously\", \"NOTE:\", \"I considered\", \"per the ticket\". If you added one of those in this edit, delete it before you finish. Keep comments that were already in the file unless the user asked to remove them.";

fn is_ws_byte(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\r')
}

fn is_skill_name_continue(b: u8) -> bool {
    b == b'-' || b == b'/' || b == b'_' || b.is_ascii_alphanumeric()
}

/// Whether the match at `start..end` is a standalone `/name` token.
fn bounded(text: &str, start: usize, end: usize) -> bool {
    let bytes = text.as_bytes();
    if start > 0 && !is_ws_byte(bytes[start - 1]) {
        return false;
    }
    !(end < bytes.len() && is_skill_name_continue(bytes[end]))
}

/// Unique `/skill-name` tokens in first-seen order (not paths like
/// `/Users/…` or `dir/skill`).
pub fn collect_skill_mentions(text: &str) -> Vec<String> {
    if text.is_empty() || !text.contains('/') {
        return Vec::new();
    }
    let mut out: Vec<String> = Vec::new();
    for c in SKILL_SLASH_RE.captures_iter(text) {
        let (Some(m), Some(name)) = (c.get(0), c.get(1)) else {
            continue;
        };
        let name = name.as_str();
        if name.is_empty() || out.iter().any(|n| n == name) {
            continue;
        }
        if !bounded(text, m.start(), m.end()) {
            continue;
        }
        out.push(name.to_owned());
    }
    out
}

/// Host-activates `/skill-name` mentions: loads each user-invocable skill's
/// body and prepends `<skill_content>` blocks; activated (and disabled)
/// tokens are stripped from the user body. `enabled` filters activation
/// (`None` allows all).
pub fn wrap_activated_skills(
    text: &str,
    worktree: &str,
    enabled: Option<&dyn Fn(&str) -> bool>,
) -> String {
    let mut names = collect_skill_mentions(text);
    if names.is_empty() {
        return text.to_owned();
    }
    names.truncate(MAX_ACTIVATED_SKILLS);
    let cancel = deadline_token(SKILL_ACTIVATE_TIMEOUT);
    let all: Vec<AgentSkill> = match recur_code::list_skills(&cancel, worktree) {
        Ok(all) if !all.is_empty() => all,
        _ => return text.to_owned(),
    };
    let mut blocks: Vec<String> = Vec::new();
    let mut activated: Vec<String> = Vec::new();
    let mut disabled: Vec<String> = Vec::new();
    for name in &names {
        let Some(sk) = all.iter().find(|s| &s.name == name) else {
            continue;
        };
        if sk.path.is_empty() || !sk.user_invocable {
            continue;
        }
        if let Some(f) = enabled
            && !f(name)
        {
            disabled.push(name.clone());
            continue;
        }
        let body = recur_code::read_skill_body(&sk.path);
        if body.trim().is_empty() {
            continue;
        }
        blocks.push(format_skill_content(name, &sk.path, &body));
        activated.push(name.clone());
    }
    if blocks.is_empty() && disabled.is_empty() {
        return text.to_owned();
    }
    let strip: Vec<String> = activated.iter().chain(disabled.iter()).cloned().collect();
    let body = strip_activated_skill_mentions(text, &strip);
    let mut out = String::from("<system_instruction>\n");
    if !blocks.is_empty() {
        out.push_str(
            "The user activated skill(s) via /name. Follow each skill's instructions below.\n",
        );
        out.push_str(
            "Do not search for these skills or re-load them — their content is already included.\n",
        );
    }
    if !disabled.is_empty() {
        out.push_str(
            "These skill mentions were ignored because they are disabled in Settings/Project: ",
        );
        out.push_str(&disabled.join(", "));
        out.push_str(".\nDo not search for or load these skills.\n");
    }
    out.push_str("</system_instruction>");
    for b in &blocks {
        out.push_str("\n\n");
        out.push_str(b);
    }
    if !body.trim().is_empty() {
        out.push_str("\n\n");
        out.push_str(&body);
    }
    out
}

/// A skill body wrapped for host activation or `activate_skill` (same shape
/// either way), with its directory and bundled resources.
pub fn format_skill_content(name: &str, skill_md_path: &str, body: &str) -> String {
    let body = if body.len() > MAX_SKILL_BODY_BYTES {
        let mut cut = MAX_SKILL_BODY_BYTES;
        while !body.is_char_boundary(cut) {
            cut -= 1;
        }
        format!("{}\n\n…(skill truncated)…", &body[..cut])
    } else {
        body.to_owned()
    };
    let dir = recur_bundles::go::dir(skill_md_path);
    let mut b = format!("<skill_content name={}>\n", recur_bundles::go::quote(name));
    b.push_str(&body);
    b.push('\n');
    b.push_str(&format!("\nSkill directory: {dir}\n"));
    b.push_str("Relative paths in this skill are relative to the skill directory.\n");
    let res = recur_code::list_skill_resources(skill_md_path, 40);
    if !res.is_empty() {
        b.push_str("\n<skill_resources>\n");
        for r in res {
            b.push_str(&format!("  <file>{r}</file>\n"));
        }
        b.push_str("</skill_resources>\n");
    }
    b.push_str("</skill_content>");
    b
}

/// Removes the named `/skill` tokens, eating one adjacent space or tab so
/// "foo /skill bar" reads "foo bar" without collapsing indentation.
pub fn strip_activated_skill_mentions(text: &str, names: &[String]) -> String {
    if names.is_empty() {
        return text.to_owned();
    }
    let bytes = text.as_bytes();
    let mut b = String::with_capacity(text.len());
    let mut last = 0usize;
    for c in SKILL_SLASH_RE.captures_iter(text) {
        let (Some(m), Some(name)) = (c.get(0), c.get(1)) else {
            continue;
        };
        if !names.iter().any(|n| n == name.as_str()) {
            continue;
        }
        let (start, end) = (m.start(), m.end());
        if !bounded(text, start, end) {
            continue;
        }
        // Never step before `last`: adjacent "/a /b" share one space.
        let (mut cut_start, mut cut_end) = (start, end);
        if cut_end < bytes.len() && matches!(bytes[cut_end], b' ' | b'\t') {
            cut_end += 1;
        } else if cut_start > last && matches!(bytes[cut_start - 1], b' ' | b'\t') {
            cut_start -= 1;
        }
        b.push_str(&text[last..cut_start]);
        last = cut_end;
    }
    b.push_str(&text[last..]);
    b.trim().to_owned()
}

fn allows(req: &TurnRequest, name: &str) -> bool {
    req.allowed_tools
        .as_ref()
        .is_some_and(|t| t.iter().any(|n| n == name))
}

/// Points turns that can list skills at the skills tools (once).
pub fn apply_skill_guidance(req: &mut TurnRequest) {
    if !allows(req, tool::LIST_SKILLS) {
        return;
    }
    if req.system_prompt.contains(tool::LIST_SKILLS)
        || req.append_system_prompt.contains(tool::LIST_SKILLS)
    {
        return;
    }
    req.append_system_prompt =
        join_append_prompt(&req.append_system_prompt, SKILLS_TOOLS_APPEND_PROMPT);
}

/// The comment-hygiene overlay on turns that can edit files (once). Empty
/// allowlists (title/commit one-shots) stay off.
pub fn apply_comment_hygiene(req: &mut TurnRequest) {
    let can_edit = req.allowed_tools.as_ref().is_some_and(|t| {
        t.iter()
            .any(|n| matches!(n.as_str(), "Edit" | "Write" | "NotebookEdit"))
    });
    if !can_edit {
        return;
    }
    if req.system_prompt.contains(COMMENT_HYGIENE_NEEDLE)
        || req.append_system_prompt.contains(COMMENT_HYGIENE_NEEDLE)
    {
        return;
    }
    req.append_system_prompt =
        join_append_prompt(&req.append_system_prompt, COMMENT_HYGIENE_APPEND_PROMPT);
}

/// Joins two overlays with a blank line; either may be empty.
pub fn join_append_prompt(base: &str, extra: &str) -> String {
    let (base, extra) = (base.trim(), extra.trim());
    match (base.is_empty(), extra.is_empty()) {
        (true, _) => extra.to_owned(),
        (_, true) => base.to_owned(),
        _ => format!("{base}\n\n{extra}"),
    }
}

#[cfg(test)]
#[path = "skills_tests.rs"]
mod tests;
