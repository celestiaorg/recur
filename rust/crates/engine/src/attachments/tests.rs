//! Disk cases of Go `attachments_test.go`.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use recur_agent::attach::{ensure_markers, marker};
use recur_domain::{ChatAttachment, ImagePayload};

use super::*;

fn png() -> String {
    STANDARD.encode([
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x00,
    ])
}

fn att(id: &str, mime: &str, data: &str) -> ChatAttachment {
    ChatAttachment {
        id: id.into(),
        name: "shot.png".into(),
        mime: mime.into(),
        data: data.into(),
    }
}

#[test]
fn prepare_writes_and_keeps_markers() {
    let root = tempfile::tempdir().unwrap();
    let text = format!("look at this\n{}\nespecially the header", marker("img1"));
    let (got, imgs) = prepare_chat_attachments(
        root.path(),
        "WS1",
        &text,
        &[att("img1", "image/png", &png())],
    )
    .unwrap();
    assert_eq!(got, text);
    assert_eq!(imgs.len(), 1);
    assert_eq!(
        (imgs[0].id.as_str(), imgs[0].mime.as_str()),
        ("img1", "image/png")
    );
    assert_eq!(imgs[0].path, "workspaces/WS1/attachments/img1.png");
    assert!(root.path().join(&imgs[0].path).exists());
    assert_eq!(imgs[0].data, png());
}

#[test]
fn prepare_appends_missing_markers_and_skips_bad_input() {
    let root = tempfile::tempdir().unwrap();
    let (text, imgs) = prepare_chat_attachments(
        root.path(),
        "WS1",
        "  look  ",
        &[
            att("a b!", "IMAGE/JPG", &png()),
            att("ab", "image/png", &png()),
            att("bad", "image/tiff", &png()),
            att("empty", "image/png", ""),
            att("junk", "image/png", "!!!"),
        ],
    )
    .unwrap();
    assert_eq!(text, format!("look\n{}", marker("ab")));
    assert_eq!(
        imgs.len(),
        1,
        "duplicate sanitized id and bad inputs skipped"
    );
    assert_eq!(imgs[0].mime, "image/jpeg");
    assert_eq!(imgs[0].path, "workspaces/WS1/attachments/ab.jpg");
    let (text, imgs) = prepare_chat_attachments(root.path(), "WS1", " plain ", &[]).unwrap();
    assert_eq!((text.as_str(), imgs.len()), ("plain", 0));
}

#[test]
fn prepare_rejects_oversized_images() {
    let root = tempfile::tempdir().unwrap();
    let big = STANDARD.encode(vec![0u8; MAX_CHAT_ATTACHMENT_BYTES + 1]);
    let e = prepare_chat_attachments(root.path(), "WS1", "x", &[att("big", "image/png", &big)])
        .unwrap_err();
    assert_eq!(
        e.0,
        format!(
            "attachment big too large ({} bytes)",
            MAX_CHAT_ATTACHMENT_BYTES + 1
        )
    );
}

#[test]
fn ensure_markers_and_workspace_images() {
    let root = tempfile::tempdir().unwrap();
    let (_, imgs) = prepare_chat_attachments(
        root.path(),
        "WS1",
        "look",
        &[att("img1", "image/png", &png())],
    )
    .unwrap();
    assert!(ensure_markers("look", &imgs).contains(&marker("img1")));
    let loaded = workspace_user_images(root.path(), "WS1");
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].id, "img1");
    assert!(!loaded[0].data.is_empty());
    assert!(workspace_user_images(root.path(), "").is_empty());
}

#[test]
fn stored_images_reload() {
    let root = tempfile::tempdir().unwrap();
    let (_, imgs) = prepare_chat_attachments(
        root.path(),
        "WS1",
        &marker("img1"),
        &[att("img1", "image/png", &png())],
    )
    .unwrap();
    let got = user_image_from_stored(
        root.path(),
        &ImagePayload {
            id: imgs[0].id.clone(),
            name: imgs[0].name.clone(),
            mime: imgs[0].mime.clone(),
            path: imgs[0].path.clone(),
        },
    )
    .unwrap();
    assert_eq!(
        (got.id.as_str(), got.data.as_str()),
        ("img1", imgs[0].data.as_str())
    );
    let by_id = user_image_by_id(root.path(), "WS1", "img1").unwrap();
    assert_eq!(by_id.mime, "image/png");
    assert_eq!(by_id.name, "img1.png");
    assert_eq!(
        user_images_by_ids(root.path(), "WS1", &["img1".into(), "gone".into()]).len(),
        1
    );
    assert!(
        user_image_from_stored(
            root.path(),
            &ImagePayload {
                id: "gone".into(),
                path: "workspaces/WS1/attachments/missing.png".into(),
                ..Default::default()
            }
        )
        .is_none()
    );
}
