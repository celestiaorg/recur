//! UI parity: render recur-ui gallery scenes headlessly at device scale 2 and diff them
//! against the Svelte reference captures (`pnpm exec playwright test` in this directory).
//!
//! ```text
//! ui-parity render [scene…]   write out/candidate/<scene>.png
//! ui-parity diff [scene…]     compare out/reference with out/candidate, write out/diff heatmaps
//! ui-parity check [scene…]    render, then diff; exits non-zero when a scene is over tolerance
//! ```

use anyhow::{Context as _, Result, bail};
use gpui_kit::test::{TestEventExt, TestWindowExt};
use gpui_kit::{
    AnyView, AppContext, Bounds, Context, FocusHandle, HeadlessAppContext, InteractiveElement,
    IntoElement, Keystroke, MouseButton, ParentElement, Render, Styled, Window, WindowBounds,
    WindowOptions, div, point, px, size,
};
use image::{Rgba, RgbaImage};
use recur_ui::gallery::{self, SceneSpec, Step};
use recur_ui::theme::Theme;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn file_name(scene: &str) -> String {
    format!("{}.png", scene.replace('/', "__"))
}

/// The scene's window content. Its focus handle stands in for the page's document focus, so
/// key steps reach the window before any control is focused.
struct SceneRoot(AnyView, FocusHandle);

impl Render for SceneRoot {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .track_focus(&self.1)
            .child(recur_ui::style::window_root(self.0.clone()))
    }
}

/// Playwright key names → GPUI keystrokes.
fn keystroke(key: &str) -> String {
    match key {
        // Safari Tab-focuses only fields; Option+Tab reaches every control, as GPUI's Tab does.
        "Tab" | "Alt+Tab" => "tab".into(),
        "Shift+Tab" => "shift-tab".into(),
        "Enter" => "enter".into(),
        "Escape" => "escape".into(),
        "ArrowDown" => "down".into(),
        "ArrowUp" => "up".into(),
        "ArrowLeft" => "left".into(),
        "ArrowRight" => "right".into(),
        "Home" => "home".into(),
        "End" => "end".into(),
        "Space" | " " => "space".into(),
        "Backspace" => "backspace".into(),
        other => other.replace('+', "-").to_lowercase(),
    }
}

/// Every scene: recur-ui's widgets, then the terminal's.
fn all_specs() -> Vec<SceneSpec> {
    let mut specs = gallery::specs();
    specs.extend(recur_terminal_view::gallery::specs());
    specs
}

fn exists(name: &str) -> bool {
    gallery::exists(name) || recur_terminal_view::gallery::exists(name)
}

fn build(name: &str, window: &mut Window, cx: &mut gpui_kit::App) -> Option<AnyView> {
    if recur_terminal_view::gallery::exists(name) {
        return recur_terminal_view::gallery::build(name, window, cx);
    }
    gallery::build(name, window, cx)
}

fn render(specs: &[SceneSpec], out: &Path) -> Result<()> {
    std::fs::create_dir_all(out)?;
    let mut cx = HeadlessAppContext::with_platform(
        gpui_kit::platform::current_platform(true).text_system(),
        Arc::new(recur_ui::Assets),
        gpui_kit::platform::current_headless_renderer,
    );
    cx.update(|cx| {
        gpui_kit::init(cx);
        recur_ui::init(cx);
        recur_terminal_view::init(cx);
    });
    for spec in specs {
        cx.update(|cx| {
            Theme::set(spec.theme_id(), cx);
            recur_ui::motion::freeze(spec.animation_time.map(Duration::from_millis), cx);
        });
        let name = spec.name.clone();
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds {
                origin: point(px(0.), px(0.)),
                size: size(px(spec.width), px(spec.height)),
            })),
            focus: true,
            show: false,
            ..Default::default()
        };
        let handle = cx.update(|cx| {
            cx.open_window(options, |window, cx| {
                let view = build(&name, window, cx)
                    .unwrap_or_else(|| cx.new(|_| recur_ui::gallery::Missing).into());
                let document = cx.focus_handle();
                window.focus(&document, cx);
                let scene = cx.new(|_| SceneRoot(view, document));
                // An app window's root: Tab and Shift+Tab move focus.
                cx.new(|cx| gpui_kit::base::Root::new(scene, window, cx))
            })
        })?;
        let any = handle.into();
        // The capture page has focus; so does the scene window, before any input.
        cx.update_window(any, |_, window, _| window.activate_window())?;
        cx.run_until_parked();
        cx.update_window(any, |_, window, cx| window.render_frame(cx))?;
        for step in &spec.steps {
            match step {
                Step::Wait(ms) => {
                    cx.advance_clock(Duration::from_millis(*ms));
                    cx.run_until_parked();
                }
                step => {
                    cx.update_window(any, |_, window, cx| match step {
                        Step::Move([x, y]) => window.pointer_move(point(px(*x), px(*y)), None, cx),
                        Step::Down([x, y]) => {
                            window.pointer_move(point(px(*x), px(*y)), None, cx);
                            window.pointer_down(point(px(*x), px(*y)), MouseButton::Left, cx);
                        }
                        Step::Up([x, y]) => {
                            window.pointer_move(point(px(*x), px(*y)), None, cx);
                            window.pointer_up(point(px(*x), px(*y)), MouseButton::Left, cx);
                        }
                        Step::Click([x, y]) => {
                            let p = point(px(*x), px(*y));
                            window.pointer_move(p, None, cx);
                            window.pointer_down(p, MouseButton::Left, cx);
                            window.pointer_up(p, MouseButton::Left, cx);
                        }
                        Step::Key(k) => {
                            let ks = keystroke(k);
                            if Keystroke::parse(&ks).is_ok() {
                                window.key_down(&ks, false, cx);
                                window.key_up(&ks, cx);
                            }
                        }
                        Step::Type(text) => {
                            for ch in text.chars() {
                                let ks = if ch == ' ' {
                                    "space".to_string()
                                } else {
                                    ch.to_string()
                                };
                                window.key_down(&ks, false, cx);
                                window.key_up(&ks, cx);
                            }
                        }
                        Step::Wait(_) => {}
                    })?;
                    cx.run_until_parked();
                }
            }
            cx.update_window(any, |_, window, cx| window.render_frame(cx))?;
        }
        cx.run_until_parked();
        cx.update_window(any, |_, window, cx| window.render_frame(cx))?;
        let shot = cx
            .capture_screenshot(any)
            .with_context(|| format!("capture {}", spec.name))?;
        shot.save(out.join(file_name(&spec.name)))?;
        cx.update_window(any, |_, window, _| window.remove_window())?;
        cx.run_until_parked();
        println!("rendered {}", spec.name);
    }
    Ok(())
}

/// Per-scene diff counts. Pixels whose channels differ by at most one level are equal (8-bit
/// compositing quantization); the rest are text (inside a reference text box), anti-aliased
/// fringe (pixelmatch's detection, a soft edge, or gradient dither), or hard.
struct DiffResult {
    name: String,
    hard: u64,
    text: u64,
    aa: u64,
    max_delta: u8,
    tolerance: u64,
    hard_tolerance: u64,
    error: Option<String>,
}

impl DiffResult {
    fn pass(&self) -> bool {
        self.error.is_none()
            && self.hard <= self.hard_tolerance
            && self.text + self.aa <= self.tolerance
    }
}

/// Pixelmatch's YIQ brightness of a pixel blended over white.
fn luma(p: &Rgba<u8>) -> f64 {
    let a = p.0[3] as f64 / 255.;
    let blend = |c: u8| 255. + (c as f64 - 255.) * a;
    0.298_895_31 * blend(p.0[0]) + 0.586_622_47 * blend(p.0[1]) + 0.114_482_23 * blend(p.0[2])
}

/// Pixelmatch `hasManySiblings`: more than two identical neighbors.
fn many_siblings(img: &RgbaImage, x: u32, y: u32) -> bool {
    let (w, h) = img.dimensions();
    let (x0, y0, x1, y1) = (
        x.saturating_sub(1),
        y.saturating_sub(1),
        (x + 1).min(w - 1),
        (y + 1).min(h - 1),
    );
    let mut zeroes = u32::from(x == x0 || x == x1 || y == y0 || y == y1);
    let p = img.get_pixel(x, y);
    for nx in x0..=x1 {
        for ny in y0..=y1 {
            if (nx, ny) != (x, y) && img.get_pixel(nx, ny) == p {
                zeroes += 1;
                if zeroes > 2 {
                    return true;
                }
            }
        }
    }
    false
}

/// Pixelmatch `antialiased`: the pixel sits between a darker and a brighter neighbor, has few
/// identical neighbors, and one of those extremes is a flat region in both images.
fn antialiased(img: &RgbaImage, x: u32, y: u32, other: &RgbaImage) -> bool {
    let (w, h) = img.dimensions();
    let (x0, y0, x1, y1) = (
        x.saturating_sub(1),
        y.saturating_sub(1),
        (x + 1).min(w - 1),
        (y + 1).min(h - 1),
    );
    let mut zeroes = u32::from(x == x0 || x == x1 || y == y0 || y == y1);
    let base = luma(img.get_pixel(x, y));
    let (mut min, mut max) = (0f64, 0f64);
    let (mut min_at, mut max_at) = ((0, 0), (0, 0));
    for nx in x0..=x1 {
        for ny in y0..=y1 {
            if (nx, ny) == (x, y) {
                continue;
            }
            let delta = base - luma(img.get_pixel(nx, ny));
            if delta == 0. {
                zeroes += 1;
                if zeroes > 2 {
                    return false;
                }
            } else if delta < min {
                min = delta;
                min_at = (nx, ny);
            } else if delta > max {
                max = delta;
                max_at = (nx, ny);
            }
        }
    }
    if min == 0. || max == 0. {
        return false;
    }
    (many_siblings(img, min_at.0, min_at.1) && many_siblings(other, min_at.0, min_at.1))
        || (many_siblings(img, max_at.0, max_at.1) && many_siblings(other, max_at.0, max_at.1))
}

/// A partial-coverage pixel whose value moved by at most a quarter of the local contrast:
/// anti-aliasing along the same edge, not an edge that moved (that changes it by the full
/// contrast).
fn soft_fringe(a: &RgbaImage, x: u32, y: u32, delta: u8) -> bool {
    let range = local_range(a, x, y);
    range > 0 && (delta as u32) * 4 <= range as u32
}

/// Channel range over the 3×3 neighborhood.
fn local_range(a: &RgbaImage, x: u32, y: u32) -> u8 {
    let (w, h) = a.dimensions();
    let (mut lo, mut hi) = (u8::MAX, 0u8);
    for nx in x.saturating_sub(1)..=(x + 1).min(w - 1) {
        for ny in y.saturating_sub(1)..=(y + 1).min(h - 1) {
            let p = a.get_pixel(nx, ny);
            for c in &p.0[..3] {
                lo = lo.min(*c);
                hi = hi.max(*c);
            }
        }
    }
    hi - lo
}

/// The widest single-channel range over the 3×3 neighborhood (a tinted flat area is 0).
fn channel_range(a: &RgbaImage, x: u32, y: u32) -> u8 {
    let (w, h) = a.dimensions();
    (0..3)
        .map(|c| {
            let (mut lo, mut hi) = (u8::MAX, 0u8);
            for nx in x.saturating_sub(1)..=(x + 1).min(w - 1) {
                for ny in y.saturating_sub(1)..=(y + 1).min(h - 1) {
                    let v = a.get_pixel(nx, ny).0[c];
                    lo = lo.min(v);
                    hi = hi.max(v);
                }
            }
            hi - lo
        })
        .max()
        .unwrap_or(0)
}

/// Reference text boxes (CSS px) scaled to device pixels and grown by 2dp, so glyph
/// anti-aliasing and sub-pixel placement land inside.
fn text_mask(path: &Path, w: u32, h: u32) -> Vec<bool> {
    let mut mask = vec![false; (w * h) as usize];
    let Ok(raw) = std::fs::read_to_string(path) else {
        return mask;
    };
    let rects: Vec<[f32; 4]> = serde_json::from_str(&raw).unwrap_or_default();
    for [x, y, rw, rh] in rects {
        let x0 = ((x * 2.).floor() as i64 - 2).max(0) as u32;
        let y0 = ((y * 2.).floor() as i64 - 2).max(0) as u32;
        let x1 = (((x + rw) * 2.).ceil() as u32 + 2).min(w);
        let y1 = (((y + rh) * 2.).ceil() as u32 + 2).min(h);
        for yy in y0..y1 {
            for xx in x0..x1 {
                mask[(yy * w + xx) as usize] = true;
            }
        }
    }
    mask
}

fn diff_one(spec: &SceneSpec, reference: &Path, candidate: &Path, out: &Path) -> DiffResult {
    let mut r = DiffResult {
        name: spec.name.clone(),
        hard: 0,
        text: 0,
        aa: 0,
        max_delta: 0,
        tolerance: spec.tolerance,
        hard_tolerance: spec.hard_tolerance,
        error: None,
    };
    let load = |p: &Path| -> Result<RgbaImage> {
        Ok(image::open(p)
            .with_context(|| format!("open {}", p.display()))?
            .to_rgba8())
    };
    let (a, b) = match (load(reference), load(candidate)) {
        (Ok(a), Ok(b)) => (a, b),
        (Err(e), _) | (_, Err(e)) => {
            r.error = Some(e.to_string());
            return r;
        }
    };
    if a.dimensions() != b.dimensions() {
        r.error = Some(format!("size {:?} vs {:?}", a.dimensions(), b.dimensions()));
        return r;
    }
    let (w, h) = a.dimensions();
    let mask = text_mask(&reference.with_extension("text.json"), w, h);
    let mut heat = RgbaImage::new(w, h);
    for (x, y, pa) in a.enumerate_pixels() {
        let pb = b.get_pixel(x, y);
        let delta = (0..3).map(|i| pa.0[i].abs_diff(pb.0[i])).max().unwrap_or(0);
        let gray = ((pa.0[0] as u32 * 3 + pa.0[1] as u32 * 6 + pa.0[2] as u32) / 10 / 4) as u8;
        // A one-level channel delta is 8-bit compositing quantization: Skia/CoreGraphics
        // blend translucent fills in integer steps, GPUI's Metal blender in float.
        let color = if delta <= 1 {
            Rgba([gray, gray, gray, 255])
        } else if mask[(y * w + x) as usize] {
            r.text += 1;
            Rgba([gray / 2, gray / 2, 96 + (delta / 2), 255])
        } else if antialiased(&a, x, y, &b)
            || antialiased(&b, x, y, &a)
            || soft_fringe(&a, x, y, delta)
            // WebKit dithers gradients: up to two levels of noise, plus one of quantization,
            // in a smooth region.
            || (delta <= 3 && channel_range(&a, x, y) <= 6 && channel_range(&b, x, y) <= 6)
        {
            r.aa += 1;
            Rgba([200, 160, 0, 255])
        } else {
            r.hard += 1;
            Rgba([255, 0, 0, 255])
        };
        if delta > 1 {
            r.max_delta = r.max_delta.max(delta);
        }
        heat.put_pixel(x, y, color);
    }
    if let Err(e) = heat.save(out.join(file_name(&spec.name))) {
        r.error = Some(e.to_string());
    }
    r
}

fn diff(specs: &[SceneSpec]) -> Result<bool> {
    let out = root().join("out/diff");
    std::fs::create_dir_all(&out)?;
    let mut ok = true;
    let mut report = Vec::new();
    println!(
        "{:<36} {:>6} {:>7} {:>6} {:>5} {:>9} {:>6}",
        "scene", "hard", "text", "aa", "max", "tolerance", "hard ok"
    );
    for spec in specs {
        let f = file_name(&spec.name);
        let r = diff_one(
            spec,
            &root().join("out/reference").join(&f),
            &root().join("out/candidate").join(&f),
            &out,
        );
        let pass = r.pass();
        ok &= pass;
        match &r.error {
            Some(e) => println!("{:<36} ERROR {e}", r.name),
            None => println!(
                "{:<36} {:>6} {:>7} {:>6} {:>5} {:>9} {:>6} {}",
                r.name,
                r.hard,
                r.text,
                r.aa,
                r.max_delta,
                r.tolerance,
                r.hard_tolerance,
                if pass { "ok" } else { "FAIL" }
            ),
        }
        report.push(serde_json::json!({
            "scene": r.name, "hard": r.hard, "text": r.text, "aa": r.aa, "maxDelta": r.max_delta,
            "tolerance": r.tolerance, "hardTolerance": r.hard_tolerance, "pass": pass,
            "error": r.error,
        }));
    }
    std::fs::write(
        root().join("out/report.json"),
        serde_json::to_string_pretty(&report)?,
    )?;
    Ok(ok)
}

struct Measure {
    family: recur_ui::fonts::Family,
    size: f32,
    weight: f32,
    tracking: f32,
    spans: Vec<String>,
}

impl Render for Measure {
    fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        use recur_ui::text::{Text, TextStyled, span};
        let t = Text::from_spans(self.spans.iter().map(|s| span(s.clone())).collect())
            .family(self.family.clone())
            .size(self.size)
            .weight(self.weight)
            .tracking(self.tracking)
            .nowrap();
        let (w, h, m) = t.measure(None, window);
        println!(
            "width {w} height {h} baseline {} ascent {} descent {}",
            m.baseline, m.ascent, m.descent
        );
        gpui_kit::div()
    }
}

/// `ui-parity measure <ui|mono|disp|system> <size> <weight> <tracking-em> <span>…`: print the
/// layout recur-ui gives a nowrap line, to compare with the reference engine's numbers.
fn measure(args: &[String]) -> Result<()> {
    use recur_ui::fonts::Family;
    let [family, size, weight, tracking, spans @ ..] = args else {
        bail!("usage: measure <ui|mono|disp|system> <size> <weight> <tracking-em> <span>...");
    };
    let family = match family.as_str() {
        "mono" => Family::Mono,
        "disp" => Family::Display,
        "system" => Family::System,
        _ => Family::Ui,
    };
    let view = Measure {
        family,
        size: size.parse()?,
        weight: weight.parse()?,
        tracking: tracking.parse()?,
        spans: spans.to_vec(),
    };
    let mut cx = HeadlessAppContext::with_platform(
        gpui_kit::platform::current_platform(true).text_system(),
        Arc::new(recur_ui::Assets),
        || Ok(None),
    );
    cx.update(|cx| {
        gpui_kit::init(cx);
        recur_ui::init(cx);
    });
    let handle =
        cx.update(|cx| cx.open_window(WindowOptions::default(), |_, cx| cx.new(|_| view)))?;
    cx.update_window(handle.into(), |_, window, cx| window.render_frame(cx))?;
    Ok(())
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("measure") {
        return measure(&args[1..]);
    }
    let (cmd, names) = args
        .split_first()
        .map_or(("check", &[][..]), |(c, n)| (c.as_str(), n));
    let mut specs = all_specs();
    if !names.is_empty() {
        specs.retain(|s| {
            names
                .iter()
                .any(|n| s.name == *n || s.name.starts_with(n.as_str()))
        });
    }
    let unknown: Vec<&str> = specs
        .iter()
        .map(|s| s.name.as_str())
        .filter(|n| !exists(n))
        .collect();
    if !unknown.is_empty() {
        bail!("scenes without a Rust builder: {unknown:?}");
    }
    match cmd {
        "render" => render(&specs, &root().join("out/candidate")),
        "diff" => {
            if !diff(&specs)? {
                bail!("scenes over tolerance (heatmaps in out/diff)");
            }
            Ok(())
        }
        "check" => {
            render(&specs, &root().join("out/candidate"))?;
            if !diff(&specs)? {
                bail!("scenes over tolerance (heatmaps in out/diff)");
            }
            Ok(())
        }
        other => bail!("unknown command {other}; use render, diff or check"),
    }
}
