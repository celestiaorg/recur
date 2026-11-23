use recur_engine::view::chat::{ChatAttachmentDraft, capture_display_text, expand_captures};

fn draft(id: &str, capture: &str) -> ChatAttachmentDraft {
    ChatAttachmentDraft {
        id: id.into(),
        name: "x.png".into(),
        mime: "image/png".into(),
        data_url: "data:image/png;base64,AA".into(),
        capture: capture.into(),
    }
}

#[test]
fn each_capture_follows_its_marker_on_the_wire_only() {
    let marked = "fix ⟦attachment:a⟧ and ⟦attachment:b⟧\n⟦attachment:c⟧";
    let drafts = [
        draft("a", "CTX-A"),
        draft("b", ""),
        draft("c", "CTX-C"),
        draft("gone", "CTX-X"),
    ];
    assert_eq!(
        expand_captures(marked, &drafts),
        "fix ⟦attachment:a⟧\nCTX-A\n and ⟦attachment:b⟧\n⟦attachment:c⟧\nCTX-C\n"
    );
    assert_eq!(expand_captures("plain", &[draft("a", "CTX")]), "plain");

    assert_eq!(
        capture_display_text("", marked, &drafts),
        marked,
        "the bubble shows what was typed"
    );
    assert_eq!(capture_display_text("/review", marked, &drafts), "/review");
    assert_eq!(
        capture_display_text("", marked, &[draft("b", "")]),
        "",
        "no capture: the engine derives it"
    );
}
