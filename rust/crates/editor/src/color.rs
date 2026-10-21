//! CSS colors as the editor's stylesheets write them: hex (3/4/6/8
//! digits), `rgb()`/`rgba()`, `transparent`, the few named colors
//! CodeMirror's base theme uses, and `color-mix(in srgb, …)`.

use gpui_kit::{Hsla, Rgba};

pub fn rgba_f(r: f32, g: f32, b: f32, a: f32) -> Rgba {
    Rgba { r, g, b, a }
}

pub const TRANSPARENT: Rgba = Rgba {
    r: 0.0,
    g: 0.0,
    b: 0.0,
    a: 0.0,
};

fn hex_digit(c: u8) -> Option<f32> {
    (c as char).to_digit(16).map(|d| d as f32)
}

fn parse_hex(s: &str) -> Option<Rgba> {
    let h = s.strip_prefix('#')?.as_bytes();
    let short = |i: usize| hex_digit(h[i]).map(|d| d * 17.0 / 255.0);
    let long = |i: usize| Some((hex_digit(h[i])? * 16.0 + hex_digit(h[i + 1])?) / 255.0);
    match h.len() {
        3 => Some(rgba_f(short(0)?, short(1)?, short(2)?, 1.0)),
        4 => Some(rgba_f(short(0)?, short(1)?, short(2)?, short(3)?)),
        6 => Some(rgba_f(long(0)?, long(2)?, long(4)?, 1.0)),
        8 => Some(rgba_f(long(0)?, long(2)?, long(4)?, long(6)?)),
        _ => None,
    }
}

fn parse_fn_args(s: &str, name: &str) -> Option<Vec<String>> {
    let inner = s
        .strip_prefix(name)?
        .trim_start()
        .strip_prefix('(')?
        .strip_suffix(')')?;
    Some(
        inner
            .split([',', '/', ' '])
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .map(str::to_string)
            .collect(),
    )
}

fn channel(p: &str) -> Option<f32> {
    match p.strip_suffix('%') {
        Some(pct) => pct.parse::<f32>().ok().map(|v| v / 100.0),
        None => p.parse::<f32>().ok().map(|v| v / 255.0),
    }
}

fn alpha(p: &str) -> Option<f32> {
    match p.strip_suffix('%') {
        Some(pct) => pct.parse::<f32>().ok().map(|v| v / 100.0),
        None => p.parse::<f32>().ok(),
    }
}

fn named(s: &str) -> Option<Rgba> {
    Some(match s {
        "transparent" => TRANSPARENT,
        "white" => rgba_f(1.0, 1.0, 1.0, 1.0),
        "black" => rgba_f(0.0, 0.0, 0.0, 1.0),
        "red" => rgba_f(1.0, 0.0, 0.0, 1.0),
        "orange" => rgba_f(1.0, 165.0 / 255.0, 0.0, 1.0),
        "silver" => rgba_f(192.0 / 255.0, 192.0 / 255.0, 192.0 / 255.0, 1.0),
        _ => return None,
    })
}

/// Parse a CSS color, `color-mix(in srgb, …)` included.
pub fn parse_css_color(s: &str) -> Option<Rgba> {
    let s = s.trim().trim_end_matches("!important").trim();
    if let Some(c) = named(&s.to_ascii_lowercase()) {
        return Some(c);
    }
    if s.starts_with('#') {
        return parse_hex(s);
    }
    if s.starts_with("color-mix") {
        return parse_color_mix(s);
    }
    let args = parse_fn_args(s, "rgba").or_else(|| parse_fn_args(s, "rgb"))?;
    if args.len() < 3 {
        return None;
    }
    let a = if args.len() > 3 {
        alpha(&args[3])?
    } else {
        1.0
    };
    Some(rgba_f(
        channel(&args[0])?,
        channel(&args[1])?,
        channel(&args[2])?,
        a,
    ))
}

/// `color-mix(in srgb, A p%, B [q%])`.
fn parse_color_mix(s: &str) -> Option<Rgba> {
    let inner = s
        .strip_prefix("color-mix")?
        .trim_start()
        .strip_prefix('(')?
        .strip_suffix(')')?;
    let mut parts = split_top_level(inner);
    if parts.len() != 3 || parts.remove(0).trim() != "in srgb" {
        return None;
    }
    let side = |p: &str| -> Option<(Rgba, Option<f32>)> {
        let p = p.trim();
        match p.rfind(' ') {
            Some(i) if p[i + 1..].ends_with('%') && !p[..i].trim_end().ends_with(',') => {
                let pct = p[i + 1..].trim_end_matches('%').parse::<f32>().ok()? / 100.0;
                Some((parse_css_color(&p[..i])?, Some(pct)))
            }
            _ => Some((parse_css_color(p)?, None)),
        }
    };
    let (a, pa) = side(&parts[0])?;
    let (b, pb) = side(&parts[1])?;
    let (wa, wb) = match (pa, pb) {
        (Some(x), Some(y)) => (x, y),
        (Some(x), None) => (x, 1.0 - x),
        (None, Some(y)) => (1.0 - y, y),
        (None, None) => (0.5, 0.5),
    };
    Some(mix_srgb(a, wa, b, wb))
}

fn split_top_level(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut depth = 0;
    let mut cur = String::new();
    for c in s.chars() {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            ',' if depth == 0 => {
                out.push(std::mem::take(&mut cur));
                continue;
            }
            _ => {}
        }
        cur.push(c);
    }
    out.push(cur);
    out
}

/// CSS `color-mix` in sRGB: premultiplied interpolation, then scaled by
/// the weight sum when it is under 100%.
/// `color-mix(in srgb, a p%, b)` through `recur_ui::Color`.
pub fn mix(a: Rgba, p: f32, b: Rgba) -> Rgba {
    ui(a).mix(p, ui(b)).into()
}

/// `color-mix(in srgb, c p%, transparent)`.
pub fn wash(c: Rgba, p: f32) -> Rgba {
    ui(c).fade(p).into()
}

/// The `recur_ui` color for a GPUI one.
pub fn ui(c: Rgba) -> recur_ui::Color {
    recur_ui::Color {
        r: c.r,
        g: c.g,
        b: c.b,
        a: c.a,
    }
}

pub fn mix_srgb(a: Rgba, wa: f32, b: Rgba, wb: f32) -> Rgba {
    let total = wa + wb;
    if total <= 0.0 {
        return TRANSPARENT;
    }
    let (na, nb) = (wa / total, wb / total);
    let alpha = a.a * na + b.a * nb;
    let premul = |ca: f32, cb: f32| ca * a.a * na + cb * b.a * nb;
    let (r, g, bl) = if alpha > 0.0 {
        (
            premul(a.r, b.r) / alpha,
            premul(a.g, b.g) / alpha,
            premul(a.b, b.b) / alpha,
        )
    } else {
        (0.0, 0.0, 0.0)
    };
    rgba_f(r, g, bl, alpha * total.min(1.0))
}

/// `top` painted over `bottom` (source-over).
pub fn over(top: Rgba, bottom: Rgba) -> Rgba {
    let a = top.a + bottom.a * (1.0 - top.a);
    if a <= 0.0 {
        return TRANSPARENT;
    }
    let c = |t: f32, b: f32| (t * top.a + b * bottom.a * (1.0 - top.a)) / a;
    rgba_f(
        c(top.r, bottom.r),
        c(top.g, bottom.g),
        c(top.b, bottom.b),
        a,
    )
}

/// Parse or fall back.
pub fn css(s: &str, fallback: Rgba) -> Rgba {
    parse_css_color(s).unwrap_or(fallback)
}

pub fn hsla_of(c: Rgba) -> Hsla {
    c.into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: Rgba, b: Rgba) -> bool {
        (a.r - b.r).abs() < 0.01
            && (a.g - b.g).abs() < 0.01
            && (a.b - b.b).abs() < 0.01
            && (a.a - b.a).abs() < 0.01
    }

    #[test]
    fn parses_css_colors() {
        assert!(close(
            parse_css_color("#fff").unwrap(),
            rgba_f(1.0, 1.0, 1.0, 1.0)
        ));
        assert!(close(
            parse_css_color("#328c8252").unwrap(),
            rgba_f(0.196, 0.549, 0.51, 0.322)
        ));
        assert!(close(
            parse_css_color("rgba(255, 255, 255, .12)").unwrap(),
            rgba_f(1.0, 1.0, 1.0, 0.12)
        ));
        assert!(close(
            parse_css_color("rgb(10 20 30 / 50%)").unwrap(),
            rgba_f(10.0 / 255.0, 20.0 / 255.0, 30.0 / 255.0, 0.5)
        ));
        assert_eq!(parse_css_color("transparent").unwrap().a, 0.0);
        assert!(parse_css_color("nonsense").is_none());
    }

    #[test]
    fn mixes_like_css() {
        let m = parse_css_color("color-mix(in srgb, #5dbe7e 10%, transparent)").unwrap();
        assert!(close(
            m,
            rgba_f(
                0x5d as f32 / 255.0,
                0xbe as f32 / 255.0,
                0x7e as f32 / 255.0,
                0.1
            )
        ));
        let m = parse_css_color("color-mix(in srgb, #000000 50%, #ffffff)").unwrap();
        assert!(close(m, rgba_f(0.5, 0.5, 0.5, 1.0)));
        let o = over(rgba_f(1.0, 1.0, 1.0, 0.5), rgba_f(0.0, 0.0, 0.0, 1.0));
        assert!(close(o, rgba_f(0.5, 0.5, 0.5, 1.0)));
    }
}
