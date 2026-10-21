//! Provider-neutral XML replay of a transcript (session fork, mid-chat
//! provider switch). Tool results are nested inside the `<tool_use>` they
//! answer; ids are pairing-only and never emitted.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use crate::session::{
    BLOCK_COMPACT, BLOCK_FALLBACK, BLOCK_IMAGE, BLOCK_TEXT, BLOCK_THINKING, BLOCK_TOOL_RESULT,
    BLOCK_TOOL_USE, ContentBlock, Message, ToolResultPayload,
};

/// Rendering options for [`transcript_xml`] and [`Message::transcript_xml`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TranscriptOptions {
    /// Drops `<thinking>` blocks. Prior-model chain-of-thought can confuse a
    /// different model on fork or provider switch.
    pub omit_thinking: bool,
    /// Caps tool-use input and tool-result bodies at 8 KiB so one huge
    /// Read/Bash payload cannot dominate a fork replay. Prose is left intact.
    pub clip_tools: bool,
    /// Bounds the document: the oldest complete `<message>` elements are
    /// dropped first and every block body is capped. 0 means unlimited.
    pub max_bytes: usize,
    /// Resolves relative image paths against this root so a replay reader can
    /// open the attachment with its own file tools.
    pub image_root: String,
}

const BLOCK_CAP: usize = 8 << 10;

fn body_cap(opt: &TranscriptOptions) -> usize {
    if opt.max_bytes > 0 || opt.clip_tools {
        BLOCK_CAP
    } else {
        0
    }
}

type ToolIo = (HashMap<String, ToolResultPayload>, HashSet<String>);

fn index_tool_io(msgs: &[Message]) -> ToolIo {
    let mut results = HashMap::new();
    let mut uses = HashSet::new();
    for c in msgs.iter().flat_map(Message::blocks) {
        if let Some(p) = c.as_tool_use()
            && !p.id.is_empty()
        {
            uses.insert(p.id);
        }
        if let Some(p) = c.as_tool_result()
            && !p.tool_use_id.is_empty()
        {
            results.insert(p.tool_use_id.clone(), p);
        }
    }
    (results, uses)
}

/// Renders msgs as a `<transcript>` document, oldest first. Tool results on
/// later user-carrier rows fold into the assistant `<tool_use>` they answer.
/// Consecutive assistant turns merge into one `<message>`. A user row that
/// mixes a tool result with real text folds the result and emits the text as
/// a user `<message>`, which also ends the assistant merge. Empty when
/// nothing renders.
pub fn transcript_xml(msgs: &[Message], opt: &TranscriptOptions) -> String {
    const PREFIX: &str = "<transcript>\n";
    const SUFFIX: &str = "</transcript>\n";
    if msgs.is_empty() {
        return String::new();
    }
    let cap = body_cap(opt);
    let io = index_tool_io(msgs);
    let mut raw: Vec<(&str, String)> = Vec::with_capacity(msgs.len());
    for m in msgs {
        let body = m.transcript_body(cap, &io, opt);
        if !body.is_empty() {
            raw.push((&m.role, body));
        }
    }
    let parts: Vec<String> = fold_assistant_turns(raw)
        .into_iter()
        .map(|(role, body)| wrap_message(role, &body))
        .collect();
    if parts.is_empty() {
        return String::new();
    }
    if opt.max_bytes == 0 {
        return format!("{PREFIX}{}{SUFFIX}", parts.concat());
    }

    let mut used = PREFIX.len() + SUFFIX.len();
    let mut start = parts.len();
    while start > 0 {
        let next = parts[start - 1].len();
        let note = if start > 1 {
            omitted_tag_len(start - 1)
        } else {
            0
        };
        if used + next + note > opt.max_bytes && start < parts.len() {
            break;
        }
        start -= 1;
        used += next;
    }
    let mut b = String::with_capacity(used + 64);
    b.push_str(PREFIX);
    if start > 0 {
        b.push_str("<omitted");
        write_attr(&mut b, "n", &start.to_string());
        b.push_str("/>\n");
    }
    for p in &parts[start..] {
        b.push_str(p);
    }
    b.push_str(SUFFIX);
    b
}

fn omitted_tag_len(n: usize) -> usize {
    r#"<omitted n=""/>"#.len() + n.to_string().len() + 1
}

/// Merges adjacent assistant parts: once tool-result carriers are folded, a
/// multi-round tool loop reads better as one message. A leftover user body
/// (steer text that shared a row with a folded result) splits the merge.
fn fold_assistant_turns(parts: Vec<(&str, String)>) -> Vec<(&str, String)> {
    let mut out: Vec<(&str, String)> = Vec::with_capacity(parts.len());
    for (role, body) in parts {
        if let Some(last) = out.last_mut()
            && last.0 == "assistant"
            && role == "assistant"
        {
            last.1.push_str(&body);
            continue;
        }
        out.push((role, body));
    }
    out
}

fn wrap_message(role: &str, body: &str) -> String {
    let mut b = String::from("<message");
    write_attr(&mut b, "role", role);
    b.push_str(">\n");
    b.push_str(body);
    b.push_str("</message>\n");
    b
}

impl Message {
    /// Renders this one message as a structured `<message>` element. Tool
    /// results on the same message nest inside the matching `<tool_use>`.
    /// Empty when nothing renders.
    pub fn transcript_xml(&self, opt: &TranscriptOptions) -> String {
        let io = index_tool_io(std::slice::from_ref(self));
        let body = self.transcript_body(body_cap(opt), &io, opt);
        if body.is_empty() {
            return String::new();
        }
        wrap_message(&self.role, &body)
    }

    fn transcript_body(&self, cap: usize, io: &ToolIo, opt: &TranscriptOptions) -> String {
        let mut body = String::new();
        for c in self.blocks() {
            write_block(c, &mut body, cap, io, opt);
        }
        body
    }
}

fn write_block(
    c: &ContentBlock,
    b: &mut String,
    cap: usize,
    (results, uses): &ToolIo,
    opt: &TranscriptOptions,
) {
    let prose_cap = if opt.clip_tools && opt.max_bytes == 0 {
        0
    } else {
        cap
    };
    match c.r#type.as_str() {
        BLOCK_TEXT => {
            let Some(p) = c.as_text() else {
                return write_unknown(c, b, cap);
            };
            let text = if p.text.trim().is_empty() {
                p.display_text
            } else {
                p.text
            };
            if text.trim().is_empty() {
                return;
            }
            write_elem(b, "text", clip(&text, prose_cap), &[], false);
        }
        BLOCK_THINKING => {
            if opt.omit_thinking {
                return;
            }
            let Some(p) = c.as_thinking() else {
                return write_unknown(c, b, cap);
            };
            let blank = p.thinking.trim().is_empty();
            let tokens = p.estimated_tokens.to_string();
            let attrs: &[(&str, &str)] = if p.estimated_tokens > 0 && blank {
                &[("tokens", &tokens)]
            } else {
                &[]
            };
            if blank && attrs.is_empty() {
                return;
            }
            write_elem(b, "thinking", clip(&p.thinking, prose_cap), attrs, false);
        }
        BLOCK_TOOL_USE => {
            let Some(p) = c.as_tool_use() else {
                return write_unknown(c, b, cap);
            };
            write_tool_use(b, &p.name, &p.input, results.get(&p.id), cap);
        }
        BLOCK_TOOL_RESULT => {
            let Some(p) = c.as_tool_result() else {
                return write_unknown(c, b, cap);
            };
            // Folded into the matching <tool_use> when that call is in this transcript.
            if uses.contains(&p.tool_use_id) {
                return;
            }
            write_tool_use(b, "", "", Some(&p), cap);
        }
        BLOCK_IMAGE => {
            let Some(p) = c.as_image() else {
                return write_unknown(c, b, cap);
            };
            b.push_str("<image");
            write_attr(b, "name", &p.name);
            write_attr(b, "mime", &p.mime);
            let path = if !opt.image_root.is_empty()
                && !p.path.is_empty()
                && !Path::new(&p.path).is_absolute()
            {
                clean_path(&format!("{}/{}", opt.image_root, p.path))
            } else {
                p.path
            };
            write_attr(b, "path", &path);
            b.push_str("/>\n");
        }
        BLOCK_COMPACT => {
            let Some(p) = c.as_compact() else {
                return write_unknown(c, b, cap);
            };
            let pre = p.pre_tokens.to_string();
            let mut attrs = vec![("status", p.status.as_str())];
            if !p.trigger.is_empty() {
                attrs.push(("trigger", &p.trigger));
            }
            if p.pre_tokens > 0 {
                attrs.push(("pre_tokens", &pre));
            }
            write_elem(b, "compact", clip(&p.summary, prose_cap), &attrs, false);
        }
        BLOCK_FALLBACK => {
            let Some(p) = c.as_fallback() else {
                return write_unknown(c, b, cap);
            };
            write_elem(
                b,
                "fallback",
                String::new(),
                &[("from", &p.from), ("to", &p.to)],
                false,
            );
        }
        _ => write_unknown(c, b, cap),
    }
}

fn write_tool_use(
    b: &mut String,
    name: &str,
    input: &str,
    result: Option<&ToolResultPayload>,
    cap: usize,
) {
    b.push_str("<tool_use");
    write_attr(b, "name", name);
    b.push_str(">\n");
    if !input.trim().is_empty() && input != "{}" {
        write_elem(b, "input", clip(input, cap), &[], true);
    }
    if let Some(r) = result {
        let mut attrs = Vec::new();
        if r.is_error {
            attrs.push(("is_error", "true"));
        }
        if r.truncated {
            attrs.push(("truncated", "true"));
        }
        write_elem(b, "result", clip(&r.result, cap), &attrs, true);
    }
    b.push_str("</tool_use>\n");
}

fn write_unknown(c: &ContentBlock, b: &mut String, cap: usize) {
    if c.r#type.is_empty() {
        return;
    }
    write_elem(
        b,
        "block",
        clip(&c.content_json, cap),
        &[("type", &c.r#type)],
        true,
    );
}

/// Writes `<tag attrs>body</tag>`. Opaque payloads (tool input/result) always
/// use CDATA; other bodies use CDATA only when they contain markup.
fn write_elem(b: &mut String, tag: &str, body: String, attrs: &[(&str, &str)], cdata: bool) {
    b.push('<');
    b.push_str(tag);
    for (k, v) in attrs {
        write_attr(b, k, v);
    }
    if body.is_empty() {
        b.push_str("/>\n");
        return;
    }
    b.push('>');
    if cdata || needs_cdata(&body) {
        write_cdata(b, &body);
    } else {
        b.push_str(&body);
    }
    b.push_str("</");
    b.push_str(tag);
    b.push_str(">\n");
}

fn write_attr(b: &mut String, key: &str, val: &str) {
    if val.is_empty() {
        return;
    }
    b.push(' ');
    b.push_str(key);
    b.push_str("=\"");
    for ch in val.chars() {
        match ch {
            '&' => b.push_str("&amp;"),
            '<' => b.push_str("&lt;"),
            '"' => b.push_str("&quot;"),
            _ => b.push(ch),
        }
    }
    b.push('"');
}

fn needs_cdata(s: &str) -> bool {
    s.contains(['<', '>', '&'])
}

/// Wraps s in CDATA, splitting on `]]>` so the terminator never appears
/// inside a single section (the only CDATA escape).
fn write_cdata(b: &mut String, mut s: &str) {
    while let Some(i) = s.find("]]>") {
        b.push_str("<![CDATA[");
        b.push_str(&s[..i + 2]);
        b.push_str("]]>");
        s = &s[i + 2..];
    }
    b.push_str("<![CDATA[");
    b.push_str(s);
    b.push_str("]]>");
}

fn clip(s: &str, cap: usize) -> String {
    if cap == 0 || s.len() <= cap {
        return s.to_owned();
    }
    let mut end = cap;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n…(truncated)…", &s[..end])
}

/// Go `filepath.Clean` for slash-separated paths: the shortest lexically
/// equivalent path.
fn clean_path(path: &str) -> String {
    let rooted = path.starts_with('/');
    let mut out: Vec<&str> = Vec::new();
    for seg in path.split('/') {
        match seg {
            "" | "." => {}
            ".." => match out.last() {
                Some(&last) if last != ".." => {
                    out.pop();
                }
                _ if rooted => {}
                _ => out.push(".."),
            },
            seg => out.push(seg),
        }
    }
    let joined = out.join("/");
    match (rooted, joined.is_empty()) {
        (true, _) => format!("/{joined}"),
        (false, true) => ".".to_owned(),
        (false, false) => joined,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn msg(role: &str, content: Vec<ContentBlock>) -> Message {
        Message {
            role: role.into(),
            content: Some(content),
            ..Default::default()
        }
    }

    fn text(s: &str) -> ContentBlock {
        ContentBlock::text(s)
    }

    fn tool_use(id: &str, pid: &str, name: &str, input: &str) -> ContentBlock {
        ContentBlock::tool_use(id, pid, name, input)
    }

    fn result(id: &str, r: &str, err: bool) -> ContentBlock {
        ContentBlock::tool_result(id, r, err)
    }

    fn xml(msgs: &[Message]) -> String {
        transcript_xml(msgs, &TranscriptOptions::default())
    }

    #[test]
    fn nests_tool_result() {
        let got = xml(&[
            msg("user", vec![text("hello")]),
            msg(
                "assistant",
                vec![
                    ContentBlock::thinking("look at the tree", "", 0),
                    tool_use("tu-1", "toolu_1", "Bash", r#"{"command":"ls"}"#),
                ],
            ),
            msg("user", vec![result("tu-1", "file.txt", false)]),
            msg("assistant", vec![text("here is the listing")]),
        ]);
        let want = r#"<transcript>
<message role="user">
<text>hello</text>
</message>
<message role="assistant">
<thinking>look at the tree</thinking>
<tool_use name="Bash">
<input><![CDATA[{"command":"ls"}]]></input>
<result><![CDATA[file.txt]]></result>
</tool_use>
<text>here is the listing</text>
</message>
</transcript>
"#;
        assert_eq!(got, want);
    }

    #[test]
    fn preamble_text_then_tool_then_reply() {
        let got = xml(&[
            msg("user", vec![text("is auth ok?")]),
            msg(
                "assistant",
                vec![
                    text("Let me check."),
                    tool_use("tu-1", "toolu_read1", "Read", r#"{"file_path":"auth.go"}"#),
                ],
            ),
            msg("user", vec![result("tu-1", "package auth", false)]),
            msg("assistant", vec![text("Auth looks fine.")]),
        ]);
        let want = r#"<transcript>
<message role="user">
<text>is auth ok?</text>
</message>
<message role="assistant">
<text>Let me check.</text>
<tool_use name="Read">
<input><![CDATA[{"file_path":"auth.go"}]]></input>
<result><![CDATA[package auth]]></result>
</tool_use>
<text>Auth looks fine.</text>
</message>
</transcript>
"#;
        assert_eq!(got, want);
    }

    #[test]
    fn parallel_tool_uses() {
        let got = xml(&[
            msg("user", vec![text("read both")]),
            msg(
                "assistant",
                vec![
                    tool_use("tu-a", "toolu_a", "Read", r#"{"file_path":"a.go"}"#),
                    tool_use("tu-b", "toolu_b", "Read", r#"{"file_path":"b.go"}"#),
                ],
            ),
            msg(
                "user",
                vec![
                    result("tu-a", "package a", false),
                    result("tu-b", "package b", false),
                ],
            ),
            msg("assistant", vec![text("both look fine")]),
        ]);
        let want = r#"<transcript>
<message role="user">
<text>read both</text>
</message>
<message role="assistant">
<tool_use name="Read">
<input><![CDATA[{"file_path":"a.go"}]]></input>
<result><![CDATA[package a]]></result>
</tool_use>
<tool_use name="Read">
<input><![CDATA[{"file_path":"b.go"}]]></input>
<result><![CDATA[package b]]></result>
</tool_use>
<text>both look fine</text>
</message>
</transcript>
"#;
        assert_eq!(got, want);
    }

    #[test]
    fn multi_round_tools() {
        let got = xml(&[
            msg("user", vec![text("fix the test")]),
            msg(
                "assistant",
                vec![tool_use(
                    "tu-1",
                    "toolu_1",
                    "Read",
                    r#"{"file_path":"x_test.go"}"#,
                )],
            ),
            msg("user", vec![result("tu-1", "func TestX", false)]),
            msg(
                "assistant",
                vec![tool_use(
                    "tu-2",
                    "toolu_2",
                    "Bash",
                    r#"{"command":"go test"}"#,
                )],
            ),
            msg("user", vec![result("tu-2", "PASS", false)]),
            msg("assistant", vec![text("tests pass")]),
        ]);
        let want = r#"<transcript>
<message role="user">
<text>fix the test</text>
</message>
<message role="assistant">
<tool_use name="Read">
<input><![CDATA[{"file_path":"x_test.go"}]]></input>
<result><![CDATA[func TestX]]></result>
</tool_use>
<tool_use name="Bash">
<input><![CDATA[{"command":"go test"}]]></input>
<result><![CDATA[PASS]]></result>
</tool_use>
<text>tests pass</text>
</message>
</transcript>
"#;
        assert_eq!(got, want);
    }

    #[test]
    fn multi_round_tools_with_interleaved_text() {
        let got = xml(&[
            msg("user", vec![text("fix the test")]),
            msg(
                "assistant",
                vec![tool_use(
                    "tu-1",
                    "toolu_1",
                    "Read",
                    r#"{"file_path":"x_test.go"}"#,
                )],
            ),
            msg("user", vec![result("tu-1", "func TestX", false)]),
            msg(
                "assistant",
                vec![
                    text("now I'll check the next file"),
                    tool_use("tu-2", "toolu_2", "Bash", r#"{"command":"go test"}"#),
                ],
            ),
            msg("user", vec![result("tu-2", "PASS", false)]),
            msg("assistant", vec![text("tests pass")]),
        ]);
        let want = r#"<transcript>
<message role="user">
<text>fix the test</text>
</message>
<message role="assistant">
<tool_use name="Read">
<input><![CDATA[{"file_path":"x_test.go"}]]></input>
<result><![CDATA[func TestX]]></result>
</tool_use>
<text>now I'll check the next file</text>
<tool_use name="Bash">
<input><![CDATA[{"command":"go test"}]]></input>
<result><![CDATA[PASS]]></result>
</tool_use>
<text>tests pass</text>
</message>
</transcript>
"#;
        assert_eq!(got, want);
    }

    #[test]
    fn two_multi_round_turns() {
        let got = xml(&[
            msg("user", vec![text("fix the test")]),
            msg(
                "assistant",
                vec![tool_use(
                    "tu-1",
                    "toolu_1",
                    "Read",
                    r#"{"file_path":"x_test.go"}"#,
                )],
            ),
            msg("user", vec![result("tu-1", "func TestX", false)]),
            msg(
                "assistant",
                vec![
                    text("now I'll run it"),
                    tool_use("tu-2", "toolu_2", "Bash", r#"{"command":"go test"}"#),
                ],
            ),
            msg("user", vec![result("tu-2", "PASS", false)]),
            msg("assistant", vec![text("tests pass")]),
            msg("user", vec![text("also lint it")]),
            msg(
                "assistant",
                vec![tool_use(
                    "tu-3",
                    "toolu_3",
                    "Read",
                    r#"{"file_path":"x.go"}"#,
                )],
            ),
            msg("user", vec![result("tu-3", "package x", false)]),
            msg(
                "assistant",
                vec![
                    text("linting"),
                    tool_use("tu-4", "toolu_4", "Bash", r#"{"command":"go vet"}"#),
                ],
            ),
            msg("user", vec![result("tu-4", "ok", false)]),
            msg("assistant", vec![text("lint clean")]),
        ]);
        let want = r#"<transcript>
<message role="user">
<text>fix the test</text>
</message>
<message role="assistant">
<tool_use name="Read">
<input><![CDATA[{"file_path":"x_test.go"}]]></input>
<result><![CDATA[func TestX]]></result>
</tool_use>
<text>now I'll run it</text>
<tool_use name="Bash">
<input><![CDATA[{"command":"go test"}]]></input>
<result><![CDATA[PASS]]></result>
</tool_use>
<text>tests pass</text>
</message>
<message role="user">
<text>also lint it</text>
</message>
<message role="assistant">
<tool_use name="Read">
<input><![CDATA[{"file_path":"x.go"}]]></input>
<result><![CDATA[package x]]></result>
</tool_use>
<text>linting</text>
<tool_use name="Bash">
<input><![CDATA[{"command":"go vet"}]]></input>
<result><![CDATA[ok]]></result>
</tool_use>
<text>lint clean</text>
</message>
</transcript>
"#;
        assert_eq!(got, want);
    }

    #[test]
    fn steer_on_tool_result_breaks_assistant_merge() {
        let got = xml(&[
            msg("user", vec![text("fix the test")]),
            msg(
                "assistant",
                vec![tool_use(
                    "tu-1",
                    "toolu_1",
                    "Read",
                    r#"{"file_path":"x_test.go"}"#,
                )],
            ),
            msg(
                "user",
                vec![
                    result("tu-1", "func TestX", false),
                    text("actually, skip the test file"),
                ],
            ),
            msg(
                "assistant",
                vec![
                    text("ok, skipping"),
                    tool_use("tu-2", "toolu_2", "Read", r#"{"file_path":"x.go"}"#),
                ],
            ),
            msg("user", vec![result("tu-2", "package x", false)]),
            msg("assistant", vec![text("looking at x.go instead")]),
        ]);
        let want = r#"<transcript>
<message role="user">
<text>fix the test</text>
</message>
<message role="assistant">
<tool_use name="Read">
<input><![CDATA[{"file_path":"x_test.go"}]]></input>
<result><![CDATA[func TestX]]></result>
</tool_use>
</message>
<message role="user">
<text>actually, skip the test file</text>
</message>
<message role="assistant">
<text>ok, skipping</text>
<tool_use name="Read">
<input><![CDATA[{"file_path":"x.go"}]]></input>
<result><![CDATA[package x]]></result>
</tool_use>
<text>looking at x.go instead</text>
</message>
</transcript>
"#;
        assert_eq!(got, want);
    }

    #[test]
    fn tool_error_then_retry() {
        let got = xml(&[
            msg("user", vec![text("run tests")]),
            msg(
                "assistant",
                vec![tool_use(
                    "tu-1",
                    "toolu_1",
                    "Bash",
                    r#"{"command":"go test ./..."}"#,
                )],
            ),
            msg("user", vec![result("tu-1", "exit 1: no go.mod", true)]),
            msg(
                "assistant",
                vec![tool_use(
                    "tu-2",
                    "toolu_2",
                    "Bash",
                    r#"{"command":"go test ./app"}"#,
                )],
            ),
            msg(
                "user",
                vec![ContentBlock::truncated_tool_result(
                    "tu-2",
                    &format!("ok\n{}", ".".repeat(40)),
                    false,
                    true,
                )],
            ),
            msg("assistant", vec![text("green")]),
        ]);
        let want = r#"<transcript>
<message role="user">
<text>run tests</text>
</message>
<message role="assistant">
<tool_use name="Bash">
<input><![CDATA[{"command":"go test ./..."}]]></input>
<result is_error="true"><![CDATA[exit 1: no go.mod]]></result>
</tool_use>
<tool_use name="Bash">
<input><![CDATA[{"command":"go test ./app"}]]></input>
<result truncated="true"><![CDATA[ok
........................................]]></result>
</tool_use>
<text>green</text>
</message>
</transcript>
"#;
        assert_eq!(got, want);
    }

    #[test]
    fn interrupted_tool_use() {
        let got = xml(&[
            msg("user", vec![text("list files")]),
            msg(
                "assistant",
                vec![
                    ContentBlock::thinking("ls the tree", "", 0),
                    tool_use("tu-1", "toolu_1", "Bash", r#"{"command":"ls"}"#),
                ],
            ),
        ]);
        let want = r#"<transcript>
<message role="user">
<text>list files</text>
</message>
<message role="assistant">
<thinking>ls the tree</thinking>
<tool_use name="Bash">
<input><![CDATA[{"command":"ls"}]]></input>
</tool_use>
</message>
</transcript>
"#;
        assert_eq!(got, want);
    }

    #[test]
    fn image_root_resolves_relative_path() {
        let msgs = [msg(
            "user",
            vec![
                ContentBlock::image(
                    "img-1",
                    "shot.png",
                    "image/png",
                    "workspaces/w/attachments/img-1.png",
                ),
                ContentBlock::image("img-2", "abs.png", "image/png", "/tmp/abs.png"),
                ContentBlock::image("img-3", "dots.png", "image/png", "./a/../b.png"),
            ],
        )];
        let got = transcript_xml(
            &msgs,
            &TranscriptOptions {
                image_root: "/data/".into(),
                ..Default::default()
            },
        );
        for want in [
            r#"path="/data/workspaces/w/attachments/img-1.png""#,
            r#"path="/tmp/abs.png""#,
            r#"path="/data/b.png""#,
        ] {
            assert!(got.contains(want), "missing {want} in:\n{got}");
        }
    }

    #[test]
    fn user_image_and_caption() {
        let got = xml(&[
            msg(
                "user",
                vec![
                    text("what's wrong here"),
                    ContentBlock::image(
                        "img-1",
                        "shot.png",
                        "image/png",
                        "workspaces/w/attachments/img-1.png",
                    ),
                ],
            ),
            msg("assistant", vec![text("the button is clipped")]),
        ]);
        let want = r#"<transcript>
<message role="user">
<text>what's wrong here</text>
<image name="shot.png" mime="image/png" path="workspaces/w/attachments/img-1.png"/>
</message>
<message role="assistant">
<text>the button is clipped</text>
</message>
</transcript>
"#;
        assert!(!got.contains("id="), "leaked image id:\n{got}");
        assert_eq!(got, want);
    }

    #[test]
    fn prefers_wire_text_over_display() {
        let got = xml(&[
            msg(
                "user",
                vec![ContentBlock::text_display(
                    "review @internal/x_test.go",
                    "review the flaky suite",
                )],
            ),
            msg("assistant", vec![text("I'll start with the suite.")]),
        ]);
        assert!(!got.contains("review the flaky suite"), "{got}");
        assert!(
            got.contains("<text>review @internal/x_test.go</text>"),
            "{got}"
        );
        let display_only = xml(&[msg("user", vec![ContentBlock::text_display("  ", "shown")])]);
        assert!(
            display_only.contains("<text>shown</text>"),
            "{display_only}"
        );
    }

    #[test]
    fn compact_then_continue() {
        let got = xml(&[
            msg(
                "assistant",
                vec![ContentBlock::compact(
                    "done",
                    "auto",
                    "older turns folded",
                    167219,
                )],
            ),
            msg("user", vec![text("continue")]),
            msg("assistant", vec![text("picking up from the compact")]),
        ]);
        let want = r#"<transcript>
<message role="assistant">
<compact status="done" trigger="auto" pre_tokens="167219">older turns folded</compact>
</message>
<message role="user">
<text>continue</text>
</message>
<message role="assistant">
<text>picking up from the compact</text>
</message>
</transcript>
"#;
        assert_eq!(got, want);
    }

    #[test]
    fn fork_replay_keeps_tools_drops_thinking() {
        let msgs = [
            msg("user", vec![text("hello")]),
            msg(
                "assistant",
                vec![
                    ContentBlock::thinking("look at the tree", "", 0),
                    tool_use("tu-1", "toolu_1", "Bash", r#"{"command":"ls"}"#),
                ],
            ),
            msg("user", vec![result("tu-1", "file.txt", false)]),
            msg(
                "assistant",
                vec![
                    ContentBlock::thinking("tell them the listing", "", 0),
                    text("here is the listing"),
                ],
            ),
        ];
        let got = transcript_xml(
            &msgs,
            &TranscriptOptions {
                omit_thinking: true,
                ..Default::default()
            },
        );
        let want = r#"<transcript>
<message role="user">
<text>hello</text>
</message>
<message role="assistant">
<tool_use name="Bash">
<input><![CDATA[{"command":"ls"}]]></input>
<result><![CDATA[file.txt]]></result>
</tool_use>
<text>here is the listing</text>
</message>
</transcript>
"#;
        assert_eq!(got, want);
    }

    #[test]
    fn message_includes_every_block_type() {
        let m = Message {
            role: "assistant".into(),
            model_id: "claude-sonnet-4-6".into(),
            provider: crate::PROVIDER_CLAUDE_CODE.into(),
            content: Some(vec![
                ContentBlock::thinking("look at the tree", "sig", 0),
                tool_use("tu-1", "toolu_1", "Bash", r#"{"command":"ls"}"#),
                text("here is the listing"),
                ContentBlock::image(
                    "img-1",
                    "shot.png",
                    "image/png",
                    "workspaces/w/attachments/img-1.png",
                ),
                ContentBlock::compact("done", "auto", "older turns folded", 1200),
                ContentBlock::fallback("claude-fable-5", "claude-opus-4-8"),
                ContentBlock::raw("evidence", r#"{"path":"x.png"}"#),
            ]),
            ..Default::default()
        };
        let got = m.transcript_xml(&TranscriptOptions::default());
        for needle in ["id=", "tool_use_id=", "<tool_result", "model=", "provider="] {
            assert!(!got.contains(needle), "leaked {needle}\n{got}");
        }
        for want in [
            r#"<message role="assistant">"#,
            "<thinking>look at the tree</thinking>",
            r#"<tool_use name="Bash">"#,
            r#"<input><![CDATA[{"command":"ls"}]]></input>"#,
            "<text>here is the listing</text>",
            r#"<image name="shot.png" mime="image/png" path="workspaces/w/attachments/img-1.png"/>"#,
            r#"<compact status="done" trigger="auto" pre_tokens="1200">older turns folded</compact>"#,
            r#"<fallback from="claude-fable-5" to="claude-opus-4-8"/>"#,
            r#"<block type="evidence"><![CDATA[{"path":"x.png"}]]></block>"#,
            "</message>",
        ] {
            assert!(got.contains(want), "missing {want}\n{got}");
        }
    }

    #[test]
    fn message_tool_result_and_redacted_thinking() {
        let m = msg(
            "user",
            vec![
                ContentBlock::thinking("", "", 80),
                ContentBlock::truncated_tool_result("tu-1", "diff --git a/x", true, true),
            ],
        );
        let got = m.transcript_xml(&TranscriptOptions::default());
        assert!(got.contains(r#"<thinking tokens="80"/>"#), "{got}");
        assert!(
            got.contains(
                r#"<result is_error="true" truncated="true"><![CDATA[diff --git a/x]]></result>"#
            ),
            "{got}"
        );
        assert!(
            !got.contains("tool_use_id=") && !got.contains("<tool_result"),
            "{got}"
        );
    }

    #[test]
    fn message_uses_cdata_for_markup() {
        let got = msg("user", vec![text(r#"use <text> & "quotes""#)])
            .transcript_xml(&TranscriptOptions::default());
        assert!(
            got.contains(r#"<text><![CDATA[use <text> & "quotes"]]></text>"#),
            "{got}"
        );
    }

    #[test]
    fn message_splits_cdata_terminator() {
        let got = msg("user", vec![text("a]]>b")]).transcript_xml(&TranscriptOptions::default());
        assert!(
            got.contains("<text><![CDATA[a]]]]><![CDATA[>b]]></text>"),
            "{got}"
        );
    }

    #[test]
    fn message_empty() {
        let m = Message {
            role: "assistant".into(),
            ..Default::default()
        };
        assert_eq!(m.transcript_xml(&TranscriptOptions::default()), "");
        assert_eq!(xml(&[]), "");
        assert_eq!(xml(&[msg("user", vec![text("  ")])]), "");
    }

    #[test]
    fn keeps_orphan_tool_result() {
        let got = xml(&[
            msg("user", vec![text("hello")]),
            msg("user", vec![result("tu-1", "huge diff", false)]),
            msg("assistant", vec![text("hi back")]),
        ]);
        assert!(
            got.starts_with("<transcript>\n") && got.ends_with("</transcript>\n"),
            "{got}"
        );
        assert!(
            got.contains("<result><![CDATA[huge diff]]></result>"),
            "{got}"
        );
        let (user, tool, asst) = (
            got.find("hello").unwrap(),
            got.find("huge diff").unwrap(),
            got.find("hi back").unwrap(),
        );
        assert!(user < tool && tool < asst, "{got}");
    }

    #[test]
    fn max_bytes_drops_oldest() {
        let msgs = [
            msg("user", vec![text(&format!("turn-one-{}", "a".repeat(80)))]),
            msg("user", vec![text(&format!("turn-two-{}", "b".repeat(80)))]),
            msg("user", vec![text("turn-three")]),
        ];
        let full = xml(&msgs);
        let got = transcript_xml(
            &msgs,
            &TranscriptOptions {
                max_bytes: full.len() / 2,
                ..Default::default()
            },
        );
        assert!(!got.is_empty());
        assert!(
            !(got.contains("turn-one-") && got.contains("turn-two-") && got.contains("turn-three")),
            "{got}"
        );
        assert!(got.contains("turn-three"), "{got}");
        assert!(got.contains("<omitted"), "{got}");
    }

    #[test]
    fn max_bytes_keeps_newest_even_over_budget() {
        let msgs = [
            msg("user", vec![text("one")]),
            msg("user", vec![text("two")]),
        ];
        let got = transcript_xml(
            &msgs,
            &TranscriptOptions {
                max_bytes: 1,
                ..Default::default()
            },
        );
        assert_eq!(
            got,
            "<transcript>\n<omitted n=\"1\"/>\n<message role=\"user\">\n<text>two</text>\n</message>\n</transcript>\n"
        );
    }

    #[test]
    fn omit_thinking() {
        let msgs = [
            msg("user", vec![text("hello")]),
            msg(
                "assistant",
                vec![
                    ContentBlock::thinking("look at the tree", "", 0),
                    text("here is the listing"),
                ],
            ),
            msg(
                "assistant",
                vec![ContentBlock::thinking("only thoughts", "", 0)],
            ),
        ];
        let got = transcript_xml(
            &msgs,
            &TranscriptOptions {
                omit_thinking: true,
                ..Default::default()
            },
        );
        assert!(
            !got.contains("<thinking")
                && !got.contains("look at the tree")
                && !got.contains("only thoughts"),
            "{got}"
        );
        assert!(
            got.contains("<text>hello</text>") && got.contains("<text>here is the listing</text>"),
            "{got}"
        );
        assert_eq!(got.matches("<message").count(), 2, "{got}");
        assert!(xml(&msgs).contains("<thinking>look at the tree</thinking>"));
    }

    #[test]
    fn clip_tools_caps_tool_bodies_not_prose() {
        let huge = "x".repeat(20 << 10);
        let prose = format!("keep-this-user-turn-{}", "y".repeat(12 << 10));
        let msgs = [
            msg("user", vec![text(&prose)]),
            msg(
                "assistant",
                vec![tool_use("t1", "", "Read", r#"{"path":"big.go"}"#)],
            ),
            msg("user", vec![result("t1", &huge, false)]),
        ];
        let got = transcript_xml(
            &msgs,
            &TranscriptOptions {
                clip_tools: true,
                ..Default::default()
            },
        );
        assert!(got.contains(&prose), "dropped user prose");
        assert!(!got.contains(&huge), "left the full tool result intact");
        assert!(got.contains("…(truncated)…"));
        assert!(!got.contains("<omitted"));
    }

    #[test]
    fn max_bytes_zero_matches_unlimited() {
        let msgs = [msg("user", vec![text("hello")])];
        assert_eq!(
            transcript_xml(
                &msgs,
                &TranscriptOptions {
                    max_bytes: 0,
                    ..Default::default()
                }
            ),
            xml(&msgs)
        );
    }

    #[test]
    fn clip_respects_char_boundaries() {
        assert_eq!(clip("héllo", 2), "h\n…(truncated)…");
        assert_eq!(clip("hello", 0), "hello");
        assert_eq!(clip("hello", 5), "hello");
    }

    #[test]
    fn clean_path_cases() {
        for (input, want) in [
            ("/data//a/./b/../c", "/data/a/c"),
            ("/../a", "/a"),
            ("a/../../b", "../b"),
            ("a/..", "."),
            ("/", "/"),
        ] {
            assert_eq!(clean_path(input), want, "{input}");
        }
    }
}
