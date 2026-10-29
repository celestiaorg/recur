//! `VideoPlayer` in a real window (macOS: AVFoundation needs the app's main thread): the clip
//! loads to its first frame, Space plays and pauses, Right seeks. Exits non-zero on failure.
//!
//! `cargo run -p recur-content --example video_smoke`

use std::time::Duration;

use gpui_kit::{
    AnyWindowHandle, App, AppContext as _, AsyncApp, Bounds, Context, Entity, Focusable as _,
    IntoElement, Keystroke, ParentElement as _, Render, Styled as _, Window, WindowBounds,
    WindowOptions, div, point, px, size,
};
use recur_content::video::{VideoOptions, VideoPlayer};

const CLIP: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/red-blue.mp4");

struct Root(Entity<VideoPlayer>);

impl Render for Root {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().p_4().child(self.0.clone())
    }
}

fn main() {
    gpui_kit::application()
        .with_assets(recur_ui::Assets)
        .run(|cx: &mut App| {
            recur_ui::init(cx);
            let mut player = None;
            let window = cx
                .open_window(
                    WindowOptions {
                        window_bounds: Some(WindowBounds::Windowed(Bounds {
                            origin: point(px(80.), px(80.)),
                            size: size(px(480.), px(320.)),
                        })),
                        focus: false,
                        ..Default::default()
                    },
                    |_, cx| {
                        let p = cx.new(|cx| VideoPlayer::new(CLIP, VideoOptions::default(), cx));
                        player = Some(p.clone());
                        cx.new(|_| Root(p))
                    },
                )
                .expect("open window");
            let player = player.expect("player");
            let window: AnyWindowHandle = window.into();
            cx.spawn(async move |cx: &mut AsyncApp| {
                let ok = run(&player, window, cx).await;
                println!("{}", if ok { "PASS" } else { "FAIL" });
                cx.update(|cx| cx.quit());
                if !ok {
                    std::process::exit(1);
                }
            })
            .detach();
        });
}

async fn frames(cx: &mut AsyncApp, window: AnyWindowHandle, ms: u64) {
    let end = std::time::Instant::now() + Duration::from_millis(ms);
    while std::time::Instant::now() < end {
        cx.background_executor()
            .timer(Duration::from_millis(30))
            .await;
        let _ = cx.update_window(window, |_, window, cx| window.draw(cx).clear(cx));
    }
}

async fn run(p: &Entity<VideoPlayer>, window: AnyWindowHandle, cx: &mut AsyncApp) -> bool {
    let status = |cx: &mut AsyncApp| p.read_with(cx, |p, _| p.status());
    for _ in 0..100 {
        frames(cx, window, 50).await;
        if status(cx).ready {
            break;
        }
    }
    let s = status(cx);
    println!("loaded: {s:?}");
    if !s.ready || s.size != (160, 90) {
        return false;
    }
    let focus = p.read_with(cx, |p, cx| p.focus_handle(cx));
    let _ = cx.update_window(window, |_, window, cx| window.focus(&focus, cx));
    let key = |k: &str, cx: &mut AsyncApp| {
        let ks = Keystroke::parse(k).expect("keystroke");
        let _ = cx.update_window(window, |_, window, cx| window.dispatch_keystroke(ks, cx));
    };
    key("space", cx);
    frames(cx, window, 600).await;
    let played = status(cx);
    println!("after Space and 600ms: {played:?}");
    key("space", cx);
    frames(cx, window, 100).await;
    let paused = status(cx);
    frames(cx, window, 300).await;
    let held = status(cx);
    println!("paused: {held:?}");
    key("right", cx);
    frames(cx, window, 200).await;
    let seeked = status(cx);
    println!("after Right: {seeked:?}");
    played.playing
        && played.position > 0.2
        && !paused.playing
        && (held.position - paused.position).abs() < 0.05
        && seeked.position >= 1.9
}
