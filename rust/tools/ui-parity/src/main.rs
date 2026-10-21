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
    AnyView, AppContext, Bounds, Context, HeadlessAppContext, IntoElement, Keystroke, MouseButton,
    Render, Window, WindowBounds, WindowOptions, point, px, size,
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

struct SceneRoot(AnyView);

impl Render for SceneRoot {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.0.clone()
    }
}

/// Playwright key names → GPUI keystrokes.
fn keystroke(key: &str) -> String {
    match key {
        "Tab" => "tab".into(),
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
                let view = gallery::build(&name, window, cx)
                    .unwrap_or_else(|| cx.new(|_| recur_ui::gallery::Missing).into());
                cx.new(|_| SceneRoot(view))
            })
        })?;
        let any = handle.into();
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

struct DiffResult {
    name: String,
    differing: u64,
    max_delta: u8,
    tolerance: u64,
    error: Option<String>,
}

fn diff_one(spec: &SceneSpec, reference: &Path, candidate: &Path, out: &Path) -> DiffResult {
    let mut r = DiffResult {
        name: spec.name.clone(),
        differing: 0,
        max_delta: 0,
        tolerance: spec.tolerance,
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
    let mut heat = RgbaImage::new(a.width(), a.height());
    for (x, y, pa) in a.enumerate_pixels() {
        let pb = b.get_pixel(x, y);
        let delta = (0..3).map(|i| pa.0[i].abs_diff(pb.0[i])).max().unwrap_or(0);
        let gray = ((pa.0[0] as u32 * 3 + pa.0[1] as u32 * 6 + pa.0[2] as u32) / 10 / 4) as u8;
        // A one-level channel delta is 8-bit compositing quantization: Skia blends translucent
        // fills in integer steps, GPUI's Metal blender in float (ui-parity.md).
        if delta > 1 {
            r.differing += 1;
            r.max_delta = r.max_delta.max(delta);
            let v = 96u8.saturating_add(delta.saturating_mul(2).min(159));
            heat.put_pixel(x, y, Rgba([v, 0, (delta < 8) as u8 * 160, 255]));
        } else if delta == 1 {
            heat.put_pixel(x, y, Rgba([gray, gray, gray.saturating_add(40), 255]));
        } else {
            heat.put_pixel(x, y, Rgba([gray, gray, gray, 255]));
        }
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
        "{:<44} {:>10} {:>6} {:>10}",
        "scene", "differing", "max", "tolerance"
    );
    for spec in specs {
        let f = file_name(&spec.name);
        let r = diff_one(
            spec,
            &root().join("out/reference").join(&f),
            &root().join("out/candidate").join(&f),
            &out,
        );
        let pass = r.error.is_none() && r.differing <= r.tolerance;
        ok &= pass;
        match &r.error {
            Some(e) => println!("{:<44} ERROR {e}", r.name),
            None => println!(
                "{:<44} {:>10} {:>6} {:>10} {}",
                r.name,
                r.differing,
                r.max_delta,
                r.tolerance,
                if pass { "ok" } else { "FAIL" }
            ),
        }
        report.push(serde_json::json!({
            "scene": r.name, "differing": r.differing, "maxDelta": r.max_delta,
            "tolerance": r.tolerance, "pass": pass, "error": r.error,
        }));
    }
    std::fs::write(
        root().join("out/report.json"),
        serde_json::to_string_pretty(&report)?,
    )?;
    Ok(ok)
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (cmd, names) = args
        .split_first()
        .map_or(("check", &[][..]), |(c, n)| (c.as_str(), n));
    let mut specs = gallery::specs();
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
        .filter(|n| !gallery::exists(n))
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
