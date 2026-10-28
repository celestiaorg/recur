//! The Linux HTML preview's WebKitGTK page. Linux needs a display (CI runs this under
//! `xvfb-run`) and skips without one.

fn main() {
    let no_display = ["DISPLAY", "WAYLAND_DISPLAY"]
        .iter()
        .all(|v| std::env::var_os(v).is_none_or(|s| s.is_empty()));
    if cfg!(target_os = "linux") && no_display {
        println!("webkit: skipped (no display)");
        return;
    }
    #[cfg(target_os = "linux")]
    html_page();
    #[cfg(target_os = "linux")]
    html_hover_and_select();
    println!("webkit: ok");
}

/// The Linux HTML preview's offscreen page: laid out at the asked size, snapshotted, driven by
/// forwarded input, and kept on its document (no navigation, no popups).
#[cfg(target_os = "linux")]
fn html_page() {
    use recur_content::html_gtk::{Page, PageEvent};
    use serde_json::json;
    use std::time::{Duration, Instant};

    const DOC: &str = r#"<!doctype html><html><body style="margin:0;background:#f00">
<button id=b style="position:absolute;left:10px;top:10px;width:80px;height:30px"
  onclick="document.body.style.background='#00f';parent.postMessage({type:'clicked'})">Go</button>
<input id=i style="position:absolute;left:10px;top:60px;width:120px">
<script>
  window.open('https://example.com');
  setTimeout(() => { location.href = 'https://example.com/away'; parent.postMessage({type:'tried'}) }, 50);
</script></body></html>"#;

    let (tx, mut rx) = futures::channel::mpsc::unbounded();
    let page = Page::new(DOC, tx).expect("page");
    page.resize(200., 100., 1.);
    page.set_active(true);
    let next = |rx: &mut futures::channel::mpsc::UnboundedReceiver<PageEvent>,
                want: &dyn Fn(&PageEvent) -> bool|
     -> PageEvent {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            match rx.try_recv() {
                Ok(ev) if want(&ev) => return ev,
                Ok(_) => {}
                Err(_) => {
                    assert!(Instant::now() < deadline, "timed out");
                    std::thread::sleep(Duration::from_millis(20));
                }
            }
        }
    };
    let center = |ev: &PageEvent| match ev {
        PageEvent::Frame {
            width,
            height,
            bgra,
        } if (*width, *height) == (200, 100) => {
            let i = ((80 * width + 150) * 4) as usize;
            Some((bgra[i + 2], bgra[i + 1], bgra[i]))
        }
        _ => None,
    };
    next(&mut rx, &|e| {
        center(e).is_some_and(|(r, g, b)| r > 200 && g < 50 && b < 50)
    });
    // The page tried to navigate away and open a popup; the click below still lands in this
    // document, so both were refused.
    next(
        &mut rx,
        &|e| matches!(e, PageEvent::Message(m) if m.contains("tried")),
    );

    page.input(&json!({"type": "down", "x": 50, "y": 25, "button": 0, "detail": 1}));
    page.input(&json!({"type": "up", "x": 50, "y": 25, "button": 0, "detail": 1}));
    next(
        &mut rx,
        &|e| matches!(e, PageEvent::Message(m) if m.contains("clicked")),
    );
    next(&mut rx, &|e| {
        center(e).is_some_and(|(r, _, b)| b > 200 && r < 50)
    });

    page.input(&json!({"type": "down", "x": 40, "y": 70, "button": 0, "detail": 1}));
    page.input(&json!({"type": "up", "x": 40, "y": 70, "button": 0, "detail": 1}));
    for k in ["h", "i", "!"] {
        page.input(&json!({"type": "key", "key": k}));
    }
    page.input(&json!({"type": "key", "key": "Backspace"}));
    page.input(&json!({"type": "key", "key": "a", "ctrl": true, "selectAll": true}));
    page.input(&json!({"type": "key", "key": "c", "ctrl": true, "copy": true}));
    let copied = next(
        &mut rx,
        &|e| matches!(e, PageEvent::Reply(serde_json::Value::String(s)) if s.starts_with('\u{1}')),
    );
    let PageEvent::Reply(serde_json::Value::String(text)) = copied else {
        unreachable!()
    };
    assert_eq!(
        &text[1..],
        "hi",
        "typed into the input, Backspace removed the !"
    );
    println!("html page: ok");
}

/// What untrusted input cannot do on its own, through the shim: `:hover` styles follow the
/// pointer, and a `<select>` answers its options for the host's menu and takes a pick.
#[cfg(target_os = "linux")]
fn html_hover_and_select() {
    use recur_content::html_gtk::{Page, PageEvent};
    use serde_json::{Value, json};
    use std::time::{Duration, Instant};

    const DOC: &str = r#"<!doctype html><html><head><style>
#h{position:absolute;left:0;top:0;width:100px;height:100px;background:#f00}
@media screen{#h:hover{background:#0f0}}
</style></head><body style="margin:0">
<div id=h></div>
<select id=s style="position:absolute;left:120px;top:10px"
  onchange="parent.postMessage({type:'changed',value:this.value})">
  <option value=a>Apple</option><option value=b disabled>Banana</option><option value=c>Cherry</option>
</select></body></html>"#;

    let (tx, mut rx) = futures::channel::mpsc::unbounded();
    let page = Page::new(DOC, tx).expect("page");
    page.resize(200., 100., 1.);
    page.set_active(true);
    let next = |rx: &mut futures::channel::mpsc::UnboundedReceiver<PageEvent>,
                want: &dyn Fn(&PageEvent) -> bool|
     -> PageEvent {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            match rx.try_recv() {
                Ok(ev) if want(&ev) => return ev,
                Ok(_) => {}
                Err(_) => {
                    assert!(Instant::now() < deadline, "timed out");
                    std::thread::sleep(Duration::from_millis(20));
                }
            }
        }
    };
    let at = |ev: &PageEvent| match ev {
        PageEvent::Frame {
            width,
            height,
            bgra,
        } if (*width, *height) == (200, 100) => {
            let i = ((50 * width + 50) * 4) as usize;
            Some((bgra[i + 2], bgra[i + 1], bgra[i]))
        }
        _ => None,
    };
    next(&mut rx, &|e| {
        at(e).is_some_and(|(r, g, _)| r > 200 && g < 50)
    });
    page.input(&json!({"type": "move", "x": 50, "y": 50}));
    next(&mut rx, &|e| {
        at(e).is_some_and(|(r, g, _)| g > 200 && r < 50)
    });
    page.input(&json!({"type": "leave"}));
    next(&mut rx, &|e| {
        at(e).is_some_and(|(r, g, _)| r > 200 && g < 50)
    });

    page.input(&json!({"type": "down", "x": 130, "y": 20, "button": 0, "detail": 1}));
    let menu = next(
        &mut rx,
        &|e| matches!(e, PageEvent::Reply(v) if v.get("select").is_some()),
    );
    let PageEvent::Reply(v) = menu else {
        unreachable!()
    };
    let options: Vec<(String, bool)> = v["select"]["options"]
        .as_array()
        .expect("options")
        .iter()
        .map(|o| {
            (
                o["label"].as_str().unwrap_or("").to_string(),
                o["disabled"] == Value::Bool(true),
            )
        })
        .collect();
    assert_eq!(
        options,
        [
            ("Apple".into(), false),
            ("Banana".into(), true),
            ("Cherry".into(), false)
        ]
    );
    assert_eq!(v["select"]["index"], json!(0));
    page.input(&json!({"type": "up", "x": 130, "y": 20, "button": 0, "detail": 1}));
    page.input(&json!({"type": "pick", "index": 2}));
    next(
        &mut rx,
        &|e| matches!(e, PageEvent::Message(m) if m.contains("changed") && m.contains("\"c\"")),
    );
    println!("html hover and select: ok");
}
