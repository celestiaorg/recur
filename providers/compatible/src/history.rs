//! Instructions and Responses input rebuilt from the Recur transcript every
//! turn.

use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::path::Path;

use base64::Engine as _;
use recur_agent::attach::MAX_CHAT_ATTACHMENT_BYTES;
use recur_agent::{TurnRequest, UserImage};
use recur_domain::{
    BLOCK_IMAGE, BLOCK_TEXT, BLOCK_THINKING, BLOCK_TOOL_RESULT, BLOCK_TOOL_USE, ImagePayload,
    Message, STREAM_PENDING, STREAM_STREAMING,
};

use crate::prompt::RECUR_CODING_PROMPT;
use crate::types::{
    InputContent, InputItem, assistant_text, function_call_item, function_output_item, user_text,
};

/// The serialized input is capped so a long chat cannot exceed request
/// limits; the oldest groups drop first.
pub const MAX_HISTORY_BYTES: usize = 8 << 20;

/// Codex's default AGENTS.md budget.
pub const MAX_REPO_RULES_BYTES: u64 = 32 << 10;

/// The AutoResume continuation display (engine `chat_errors.go`). Its turn's
/// prompt is only a continue note, so the original user row stays in input.
pub const CHAT_RESUME_DISPLAY: &str = "Recur restarted: continuing";

/// The base prompt, environment, repo rules, then the turn's overlays.
pub fn harness_instructions(req: &TurnRequest) -> String {
    let wt = req.worktree.to_string_lossy();
    join_system_parts(&[
        RECUR_CODING_PROMPT,
        &harness_env(&wt),
        &repo_rules(&wt),
        &req.system_prompt,
        &req.append_system_prompt,
        &req.turn_append_prompt,
    ])
}

/// The worktree, platform and date block; empty without a worktree.
pub fn harness_env(worktree: &str) -> String {
    if worktree.is_empty() {
        return String::new();
    }
    let (os, arch) = go_platform();
    format!(
        "## Environment\nWorktree (Bash cwd): {worktree}\nPlatform: {os}/{arch}\nToday's date: {}",
        chrono::Local::now().format("%Y-%m-%d")
    )
}

/// This build's OS and arch as GOOS / GOARCH names (`darwin`, `amd64`).
pub fn go_platform() -> (&'static str, &'static str) {
    let os = match std::env::consts::OS {
        "macos" => "darwin",
        other => other,
    };
    let arch = match std::env::consts::ARCH {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        "x86" => "386",
        other => other,
    };
    (os, arch)
}

/// The worktree's AGENTS.md (else CLAUDE.md), which Claude Code, Codex and
/// Cursor load on their own. Regular files only: a symlinked AGENTS.md could
/// point outside the worktree and ship a secret to the provider.
pub fn repo_rules(worktree: &str) -> String {
    if worktree.is_empty() {
        return String::new();
    }
    for name in ["AGENTS.md", "CLAUDE.md"] {
        let p = Path::new(worktree).join(name);
        match std::fs::symlink_metadata(&p) {
            Ok(m) if m.file_type().is_file() => {}
            _ => continue,
        }
        let Ok(f) = std::fs::File::open(&p) else {
            continue;
        };
        let mut b = Vec::new();
        let _ = f.take(MAX_REPO_RULES_BYTES).read_to_end(&mut b);
        let body = String::from_utf8_lossy(&b);
        let body = body.trim();
        if !body.is_empty() {
            return format!("## Repo instructions ({name})\n{body}");
        }
    }
    String::new()
}

/// Non-blank trimmed parts joined by a blank line.
pub fn join_system_parts(parts: &[&str]) -> String {
    parts
        .iter()
        .map(|p| p.trim())
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// The Responses input for this turn: the transcript after the fresh_session
/// cutoff, without unfinished rows or the current user row, then the prompt.
/// Stored images resolve under `data_root`.
pub fn harness_history(
    msgs: &[Message],
    req: &TurnRequest,
    data_root: Option<&Path>,
) -> Vec<InputItem> {
    let mut start = 0;
    if !req.history_after_id.is_empty()
        && let Some(i) = msgs.iter().position(|m| m.id == req.history_after_id)
    {
        start = i + 1;
    }
    let mut out = Vec::new();
    let mut call_ids: HashMap<String, String> = HashMap::new();
    for m in &msgs[start..] {
        if skip_current_user(m, req) {
            continue;
        }
        if m.stream_status == STREAM_PENDING || m.stream_status == STREAM_STREAMING {
            continue;
        }
        out.extend(items_from_message(m, &mut call_ids, data_root));
    }
    out.push(current_user_item(&req.prompt, &req.images));
    cap_history(out)
}

/// Drops the in-progress / retried user row so the prompt is not appended
/// twice. Hidden continuations (switch_mode remount, restart) keep that row:
/// their prompt is only a continue note, not the original request.
fn skip_current_user(m: &Message, req: &TurnRequest) -> bool {
    if req.user_message_id.is_empty() || m.id != req.user_message_id {
        return false;
    }
    !(req.skip_record_user && hidden_continuation(req))
}

fn hidden_continuation(req: &TurnRequest) -> bool {
    is_mode_switch_continue_display(&req.display_text) || req.display_text == CHAT_RESUME_DISPLAY
}

/// The `Switched to <Mode>: continuing` display of a switch_mode remount
/// (engine `switch_mode.go`).
pub fn is_mode_switch_continue_display(s: &str) -> bool {
    s.starts_with("Switched to ") && s.ends_with(": continuing")
}

fn image_item(mime: &str, data: &str) -> InputContent {
    let mime = if mime.is_empty() { "image/png" } else { mime };
    InputContent {
        r#type: "input_image".into(),
        image_url: format!("data:{mime};base64,{data}"),
        ..Default::default()
    }
}

fn items_from_message(
    m: &Message,
    call_ids: &mut HashMap<String, String>,
    data_root: Option<&Path>,
) -> Vec<InputItem> {
    let mut out = Vec::new();
    let mut text = String::new();
    let role = if m.role.is_empty() { "user" } else { &m.role };
    let flush = |text: &mut String, out: &mut Vec<InputItem>| {
        if text.is_empty() {
            return;
        }
        out.push(if role == "assistant" {
            assistant_text(text)
        } else {
            user_text(text)
        });
        text.clear();
    };
    for b in m.blocks() {
        match b.r#type.as_str() {
            BLOCK_TEXT => {
                let Some(p) = b.as_text() else { continue };
                if p.text.trim().is_empty() {
                    continue;
                }
                if !text.is_empty() {
                    text.push('\n');
                }
                text.push_str(&p.text);
            }
            // Chain-of-thought is not replayed.
            BLOCK_THINKING => {}
            BLOCK_IMAGE => {
                flush(&mut text, &mut out);
                let Some(p) = b.as_image() else { continue };
                let Some(img) = user_image_from_stored(&p, data_root) else {
                    continue;
                };
                out.push(InputItem {
                    role: "user".into(),
                    content: vec![image_item(&img.mime, &img.data)],
                    ..Default::default()
                });
            }
            BLOCK_TOOL_USE => {
                flush(&mut text, &mut out);
                let Some(p) = b.as_tool_use() else { continue };
                let call_id = if p.provider_tool_use_id.is_empty() {
                    p.id.clone()
                } else {
                    p.provider_tool_use_id.clone()
                };
                call_ids.insert(p.id.clone(), call_id.clone());
                out.push(function_call_item(&call_id, &p.name, &p.input));
            }
            BLOCK_TOOL_RESULT => {
                flush(&mut text, &mut out);
                let Some(p) = b.as_tool_result() else {
                    continue;
                };
                let call_id = match call_ids.get(&p.tool_use_id) {
                    Some(c) if !c.is_empty() => c.clone(),
                    _ => p.tool_use_id.clone(),
                };
                out.push(function_output_item(&call_id, &p.result));
            }
            _ => {}
        }
    }
    flush(&mut text, &mut out);
    out
}

/// Reloads a persisted chat attachment so a replay sends the same bytes the
/// original turn used (engine `userImageFromStored`).
pub fn user_image_from_stored(p: &ImagePayload, data_root: Option<&Path>) -> Option<UserImage> {
    if p.id.is_empty() || p.path.is_empty() {
        return None;
    }
    let root = data_root?.to_string_lossy();
    let full = recur_domain::path::join(&[&root, &p.path]);
    let raw = std::fs::read(full).ok()?;
    if raw.is_empty() || raw.len() > MAX_CHAT_ATTACHMENT_BYTES {
        return None;
    }
    Some(UserImage {
        id: p.id.clone(),
        name: p.name.clone(),
        mime: p.mime.clone(),
        data: base64::engine::general_purpose::STANDARD.encode(raw),
        path: p.path.clone(),
    })
}

fn current_user_item(prompt: &str, images: &[UserImage]) -> InputItem {
    let mut item = InputItem {
        role: "user".into(),
        ..Default::default()
    };
    if !prompt.trim().is_empty() {
        item.content.push(InputContent {
            r#type: "input_text".into(),
            text: prompt.into(),
            ..Default::default()
        });
    }
    for img in images.iter().filter(|i| !i.data.is_empty()) {
        item.content.push(image_item(&img.mime, &img.data));
    }
    if item.content.is_empty() {
        item.content = vec![InputContent {
            r#type: "input_text".into(),
            text: prompt.into(),
            ..Default::default()
        }];
    }
    item
}

fn encoded_len(items: &[InputItem]) -> Option<usize> {
    serde_json::to_string(items).ok().map(|s| s.len())
}

/// Drops the oldest history groups until the encoded input fits
/// [`MAX_HISTORY_BYTES`]; the current user item always stays.
pub fn cap_history(input: Vec<InputItem>) -> Vec<InputItem> {
    match encoded_len(&input) {
        Some(n) if n > MAX_HISTORY_BYTES => {}
        _ => return input,
    }
    if input.len() <= 1 {
        return input;
    }
    let mut head = input;
    let Some(keep) = head.pop() else {
        return head;
    };
    let mut start = 0;
    while start < head.len() {
        start += next_history_group(&head[start..]);
        let mut cand = head[start..].to_vec();
        cand.push(keep.clone());
        match encoded_len(&cand) {
            Some(n) if n > MAX_HISTORY_BYTES => {}
            _ => return cand,
        }
    }
    vec![keep]
}

/// How many leading items must drop together so a function_call is never
/// split from its output, a parallel call batch stays whole, and an
/// assistant turn keeps its calls.
pub fn next_history_group(input: &[InputItem]) -> usize {
    let Some(first) = input.first() else {
        return 0;
    };
    if first.r#type == "function_call_output" {
        return 1;
    }
    if first.r#type != "function_call" {
        let mut i = 1;
        while i < input.len()
            && (input[i].r#type == "function_call" || input[i].r#type == "function_call_output")
        {
            i += 1;
        }
        return i;
    }
    let mut need: HashSet<&str> = HashSet::from([first.call_id.as_str()]);
    let mut i = 1;
    while i < input.len() {
        let it = &input[i];
        match it.r#type.as_str() {
            "function_call" => {
                need.insert(&it.call_id);
                i += 1;
            }
            "function_call_output" => {
                if !need.remove(it.call_id.as_str()) {
                    return i;
                }
                i += 1;
                if need.is_empty() {
                    return i;
                }
            }
            _ => return i,
        }
    }
    i
}

#[cfg(test)]
mod tests;
