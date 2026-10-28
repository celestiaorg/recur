//! Stream-json stdin user messages (headless ref §4) and the Claude half of
//! Go `attachments.go`: images become content blocks at their
//! `⟦attachment:id⟧` markers, and an instruction prefix keeps the ids visible
//! to the model.

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;

use recur_agent::UserImage;
use recur_agent::attach::{images_in_marker_order, marker, ordinal_word, parse_markers};
use serde::Serialize;
use serde_json::{Value, json};

/// One NDJSON user turn: `{"type":"user","message":{…},"parent_tool_use_id":null}`
/// plus the trailing newline.
pub fn user_message_line(content: &str, imgs: &[UserImage]) -> String {
    #[derive(Serialize)]
    struct Line {
        #[serde(rename = "type")]
        typ: &'static str,
        message: Value,
        parent_tool_use_id: Option<()>,
    }
    let line = Line {
        typ: "user",
        message: json!({"content": user_content(content, imgs), "role": "user"}),
        parent_tool_use_id: None,
    };
    let mut out = serde_json::to_string(&line).unwrap_or_default();
    out.push('\n');
    out
}

/// The message content: the plain string when there are no images, otherwise
/// an Anthropic content-block array with text and images interleaved at the
/// markers (unmatched images last), behind the attachment-id instruction.
pub fn user_content(text: &str, imgs: &[UserImage]) -> Value {
    if imgs.is_empty() {
        return text.into();
    }
    let by_id: HashMap<&str, &UserImage> = imgs.iter().map(|i| (i.id.as_str(), i)).collect();
    let markers = parse_markers(text);
    let mut blocks = Vec::new();
    if markers.is_empty() {
        if !text.trim().is_empty() {
            blocks.push(text_block(text));
        }
        blocks.extend(imgs.iter().map(image_block));
        return with_id_instruction(text, imgs, blocks);
    }
    let mut last = 0;
    let mut seen = HashSet::new();
    for m in &markers {
        if m.start > last {
            blocks.push(text_block(&text[last..m.start]));
        }
        let id = m.id(text);
        match by_id.get(id) {
            Some(img) => {
                blocks.push(image_block(img));
                seen.insert(id);
            }
            None => blocks.push(text_block(&text[m.start..m.end])),
        }
        last = m.end;
    }
    if last < text.len() {
        blocks.push(text_block(&text[last..]));
    }
    blocks.extend(
        imgs.iter()
            .filter(|i| !seen.contains(i.id.as_str()))
            .map(image_block),
    );
    with_id_instruction(text, imgs, blocks)
}

fn text_block(text: &str) -> Value {
    json!({"text": text, "type": "text"})
}

fn image_block(img: &UserImage) -> Value {
    json!({
        "source": {"data": img.data, "media_type": img.mime, "type": "base64"},
        "type": "image",
    })
}

fn with_id_instruction(text: &str, imgs: &[UserImage], mut blocks: Vec<Value>) -> Value {
    let instr = attachment_id_instruction(text, imgs);
    if !instr.is_empty() {
        blocks.insert(0, text_block(&instr));
    }
    Value::Array(blocks)
}

/// Surfaces the `⟦attachment:id⟧` ids to Claude. The image blocks replace the
/// markers in the body, so without this prefix the model never sees the ids
/// `submit_plan.attachments` needs.
pub fn attachment_id_instruction(text: &str, imgs: &[UserImage]) -> String {
    let ordered = images_in_marker_order(text, imgs);
    if ordered.is_empty() {
        return String::new();
    }
    let mut b = String::from(
        "<system_instruction>\nThe user attached image(s). Markers like ⟦attachment:id⟧ identify each image for tools such as submit_plan.attachments; they are NOT files to open or search for.\n",
    );
    for (i, img) in ordered.iter().enumerate() {
        let n = i + 1;
        let _ = writeln!(
            b,
            "- {} is image {n} ({} attached image)",
            marker(&img.id),
            ordinal_word(n)
        );
    }
    b.push_str("</system_instruction>\n");
    b
}

#[cfg(test)]
mod tests {
    use super::*;

    fn img(id: &str) -> UserImage {
        UserImage {
            id: id.into(),
            name: format!("{id}.png"),
            mime: "image/png".into(),
            data: "iVBORw0KGgo=".into(),
            path: format!("workspaces/WS1/attachments/{id}.png"),
        }
    }

    #[test]
    fn plain_text_without_images() {
        assert_eq!(
            user_message_line("hi <b> & co", &[]),
            "{\"type\":\"user\",\"message\":{\"content\":\"hi <b> & co\",\"role\":\"user\"},\"parent_tool_use_id\":null}\n"
        );
    }

    /// Go `TestPrepareChatAttachmentsAndClaudeContent` (content half).
    #[test]
    fn interleaves_images_at_markers() {
        let text = format!("look at this\n{}\nespecially the header", marker("img1"));
        let raw = serde_json::to_string(&user_content(&text, &[img("img1")])).unwrap();
        assert!(raw.contains(r#""type":"image""#), "{raw}");
        assert!(raw.contains("look at this") && raw.contains("especially the header"));
        assert!(raw.contains("⟦attachment:img1⟧") && raw.contains("submit_plan"));
        let blocks = user_content(&text, &[img("img1")]);
        let kinds: Vec<_> = blocks
            .as_array()
            .unwrap()
            .iter()
            .map(|b| b["type"].as_str().unwrap())
            .collect();
        assert_eq!(kinds, ["text", "text", "image", "text"]);
    }

    #[test]
    fn unknown_markers_stay_text_and_unmatched_images_trail() {
        let text = format!("{}{}", marker("other"), marker("a"));
        let blocks = user_content(&text, &[img("b"), img("a")]);
        let arr = blocks.as_array().unwrap();
        assert_eq!(arr.len(), 4);
        assert_eq!(arr[1]["text"], "⟦attachment:other⟧");
        assert_eq!(arr[2]["type"], "image");
        assert_eq!(arr[3]["type"], "image");
        let instr = arr[0]["text"].as_str().unwrap();
        assert!(instr.contains("- ⟦attachment:a⟧ is image 1 (first attached image)\n"));
        assert!(instr.contains("- ⟦attachment:b⟧ is image 2 (second attached image)\n"));
    }

    #[test]
    fn no_markers_sends_text_then_images() {
        let blocks = user_content("  ", &[img("a")]);
        let arr = blocks.as_array().unwrap();
        assert_eq!(arr.len(), 2, "blank text is dropped: {blocks}");
        assert_eq!(arr[1]["type"], "image");
    }
}
