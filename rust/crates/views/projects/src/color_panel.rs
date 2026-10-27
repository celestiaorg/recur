//! `<input type=color>`: the system color panel, opened at `hex`, reporting each pick as
//! `#rrggbb` while it stays open (WebKit fires `input` the same way).

use std::rc::Rc;

use gpui_kit::{App, Window};

pub type OnPick = Rc<dyn Fn(String, &mut App)>;

#[cfg(target_os = "macos")]
pub fn open(hex: &str, on_pick: OnPick, window: &mut Window, cx: &mut App) {
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSColor, NSColorPanel, NSColorSpace};

    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let rgb = crate::pic::parse_hex_color(hex);
    let comp = |i: usize| {
        rgb.get(i..i + 2)
            .and_then(|h| u8::from_str_radix(h, 16).ok())
            .unwrap_or(0) as f64
            / 255.
    };
    let panel = NSColorPanel::sharedColorPanel(mtm);
    panel.setShowsAlpha(false);
    panel.setContinuous(true);
    panel.setColor(&NSColor::colorWithSRGBRed_green_blue_alpha(
        comp(1),
        comp(3),
        comp(5),
        1.,
    ));
    panel.makeKeyAndOrderFront(None);
    let read = || {
        let panel = NSColorPanel::sharedColorPanel(MainThreadMarker::new()?);
        if !panel.isVisible() {
            return None;
        }
        let c = panel
            .color()
            .colorUsingColorSpace(&NSColorSpace::sRGBColorSpace());
        Some(c.map(|c| {
            let b = |v: f64| (v.clamp(0., 1.) * 255.).round() as u8;
            format!(
                "#{:02x}{:02x}{:02x}",
                b(c.redComponent()),
                b(c.greenComponent()),
                b(c.blueComponent())
            )
        }))
    };
    let generation = {
        let g = cx.default_global::<PanelGen>();
        g.0 += 1;
        g.0
    };
    let mut last = rgb;
    window
        .spawn(cx, async move |cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(60))
                    .await;
                let live = cx
                    .update(|_, cx| cx.default_global::<PanelGen>().0 == generation)
                    .unwrap_or(false);
                if !live {
                    return;
                }
                let Some(picked) = read() else { return };
                if let Some(h) = picked
                    && h != last
                {
                    last = h.clone();
                    let f = on_pick.clone();
                    if cx.update(|_, cx| f(h, cx)).is_err() {
                        return;
                    }
                }
            }
        })
        .detach();
}

// simplify: GPUI has no color dialog off macOS, so the custom swatch only offers the system
// panel there. Upgrade: an in-app HSV picker in recur_ui.
#[cfg(not(target_os = "macos"))]
pub fn open(_hex: &str, _on_pick: OnPick, _window: &mut Window, _cx: &mut App) {}

/// The open panel's session; a newer `open` retires the older poll.
#[cfg(target_os = "macos")]
#[derive(Default)]
struct PanelGen(u64);

#[cfg(target_os = "macos")]
impl gpui_kit::Global for PanelGen {}
