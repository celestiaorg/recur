//! Mermaid and pdf.js in the platform WebKit: WKWebView on macOS, WebKitGTK on Linux.
//! `harness = false` so WKWebView gets the main thread. Linux needs a display (CI runs this
//! under `xvfb-run`) and skips without one. The PDF page must match the canvas pdf.js draws on
//! macOS (`tests/golden/page-pdf.png`; `RECUR_UPDATE_GOLDEN=1` rewrites it on macOS).

use std::sync::Arc;

use recur_content::mermaid::{Origin, render_blocking};
use recur_content::pdf::engine;
use recur_ui::{Theme, ThemeId};

const PDF: &[u8] = include_bytes!("../../../tools/ui-parity/content/page.pdf");
const FLOW: &str = include_str!("../../../tools/ui-parity/content/flow.mmd");
const GOLDEN: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/golden/page-pdf.png");

fn main() {
    let no_display = ["DISPLAY", "WAYLAND_DISPLAY"]
        .iter()
        .all(|v| std::env::var_os(v).is_none_or(|s| s.is_empty()));
    if cfg!(target_os = "linux") && no_display {
        println!("webkit: skipped (no display)");
        return;
    }
    mermaid_flowchart();
    pdf_page();
    #[cfg(target_os = "linux")]
    html_page();
    println!("webkit: ok");
}

fn mermaid_flowchart() {
    let d = render_blocking(Origin::File, FLOW, &Theme::new(ThemeId::Charcoal), 2.)
        .expect("mermaid render");
    let mut words: Vec<&str> = d
        .runs
        .iter()
        .flat_map(|r| r.text.split_whitespace())
        .collect();
    words.sort_unstable();
    let mut want: Vec<&str> =
        "Write markdown Has fences? mermaid code Render diagram Highlight Show in chat"
            .split(' ')
            .collect();
    want.sort_unstable();
    assert_eq!(words, want);
    let (_, _, w, h) = d.view_box;
    assert!(w > 200. && h > 300., "viewBox {:?}", d.view_box);
    println!("mermaid: viewBox {:?}", d.view_box);
}

fn pdf_page() {
    let (doc, opened) = engine::open_blocking(Arc::from(PDF));
    let opened = opened.expect("pdf open");
    assert_eq!((opened.pages, opened.width, opened.height), (1, 420., 300.));
    let page = engine::render_blocking(&doc, 1, 1., 2., 1)
        .expect("pdf render")
        .expect("not cancelled");
    assert_eq!((page.pixel_width, page.pixel_height), (840, 600));
    let mut rgba = page.image.as_bytes(0).expect("frame").to_vec();
    for p in rgba.chunks_exact_mut(4) {
        p.swap(0, 2);
    }
    if cfg!(target_os = "macos") && std::env::var_os("RECUR_UPDATE_GOLDEN").is_some() {
        image::save_buffer(GOLDEN, &rgba, 840, 600, image::ColorType::Rgba8).expect("write golden");
    }
    let golden = image::open(GOLDEN).expect("golden").to_rgba8();
    let off = rgba
        .chunks_exact(4)
        .zip(golden.as_raw().chunks_exact(4))
        .filter(|(a, b)| a.iter().zip(b.iter()).any(|(x, y)| x.abs_diff(*y) > 32))
        .count();
    println!("pdf: {off} of {} pixels differ from macOS", 840 * 600);
    // Glyph and curve edges anti-alias differently (CoreGraphics on macOS, Skia in WebKitGTK:
    // about 1.1% of this page); the page layout, shapes and colors must not.
    assert!(off < 840 * 600 / 50, "{off} pixels differ from macOS");
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
