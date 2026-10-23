//! The parts of `lib/project-pic.ts` the project form needs beyond `recur_ui`'s avatar:
//! hex parsing, resolving a palette color to hex for the color panel, and turning a picked
//! image into the stored data URL.

use base64::Engine as _;
use recur_ui::Tokens;

/// `parseHexColor`: `#rgb` / `#rrggbb` as lowercase `#rrggbb`, else empty.
pub fn parse_hex_color(color: &str) -> String {
    let t = color.trim();
    let Some(h) = t.strip_prefix('#') else {
        return String::new();
    };
    if !h.chars().all(|c| c.is_ascii_hexdigit()) {
        return String::new();
    }
    match h.len() {
        6 => format!("#{}", h.to_ascii_lowercase()),
        3 => {
            let mut out = String::from("#");
            for c in h.chars() {
                out.push(c);
                out.push(c);
            }
            out.to_ascii_lowercase()
        }
        _ => String::new(),
    }
}

/// `colorToHex`: a hex color as is, a palette `var(--…)` resolved against the app tokens.
pub fn color_to_hex(color: &str, t: &Tokens) -> String {
    let hex = parse_hex_color(color);
    if !hex.is_empty() {
        return hex;
    }
    let c = match color.trim() {
        "var(--cyan)" => t.cyan,
        "var(--amber)" => t.amber,
        "var(--indigo)" => t.indigo,
        "var(--emerald)" => t.emerald,
        "var(--coral)" => t.coral,
        _ => return String::new(),
    };
    let [r, g, b, _] = c.to_rgba8();
    format!("#{r:02x}{g:02x}{b:02x}")
}

const PIC_SIZE: u32 = 128;

/// `picFromImageFile`: cover-fit a picked image into a 128px PNG data URL.
pub fn pic_from_image_bytes(bytes: &[u8]) -> Result<String, String> {
    let img = image::load_from_memory(bytes).map_err(|_| "Could not read image".to_string())?;
    let (w, h) = (img.width().max(1) as f32, img.height().max(1) as f32);
    let scale = (PIC_SIZE as f32 / w).max(PIC_SIZE as f32 / h);
    let (sw, sh) = (
        (w * scale).round().max(1.) as u32,
        (h * scale).round().max(1.) as u32,
    );
    let scaled = img.resize_exact(sw, sh, image::imageops::FilterType::Triangle);
    let mut canvas = image::RgbaImage::new(PIC_SIZE, PIC_SIZE);
    let (ox, oy) = (
        (PIC_SIZE as i64 - sw as i64) / 2,
        (PIC_SIZE as i64 - sh as i64) / 2,
    );
    image::imageops::overlay(&mut canvas, &scaled.to_rgba8(), ox, oy);
    let mut png = Vec::new();
    image::DynamicImage::ImageRgba8(canvas)
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .map_err(|_| "Could not draw image".to_string())?;
    Ok(format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(png)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_3_and_6_digit_hex() {
        assert_eq!(parse_hex_color("#4C6"), "#44cc66");
        assert_eq!(parse_hex_color("#4cc6d6"), "#4cc6d6");
        assert_eq!(parse_hex_color("var(--cyan)"), "");
        assert_eq!(parse_hex_color("#12345"), "");
    }

    #[test]
    fn resolves_palette_colors() {
        let t = recur_ui::ThemeId::from_key("default").tokens();
        assert_eq!(color_to_hex("#ABC", &t), "#aabbcc");
        assert!(color_to_hex("var(--cyan)", &t).starts_with('#'));
        assert_eq!(color_to_hex("rebeccapurple", &t), "");
    }

    #[test]
    fn cover_fits_into_a_square_png() {
        let mut src = image::RgbaImage::new(40, 20);
        for p in src.pixels_mut() {
            *p = image::Rgba([10, 200, 30, 255]);
        }
        let mut bytes = Vec::new();
        image::DynamicImage::ImageRgba8(src)
            .write_to(&mut std::io::Cursor::new(&mut bytes), image::ImageFormat::Png)
            .unwrap();
        let url = pic_from_image_bytes(&bytes).unwrap();
        let b64 = url.strip_prefix("data:image/png;base64,").unwrap();
        let png = base64::engine::general_purpose::STANDARD.decode(b64).unwrap();
        let out = image::load_from_memory(&png).unwrap();
        assert_eq!((out.width(), out.height()), (128, 128));
        assert!(pic_from_image_bytes(b"not an image").is_err());
    }
}
