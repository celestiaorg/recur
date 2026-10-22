//! Font table values GPUI does not expose: `hhea` line gap and the `post` / `OS/2` decoration
//! metrics, read from the bundled faces. Every face of a bundled family shares its vertical
//! metrics, so the family's first face answers for all weights.

use crate::fonts::{FACES, Family};
use std::collections::HashMap;
use std::sync::OnceLock;

#[derive(Clone, Copy, Debug, PartialEq)]
struct Raw {
    upem: f32,
    line_gap: f32,
    underline_position: f32,
    underline_thickness: f32,
    strike_position: f32,
    strike_thickness: f32,
    zero_advance: f32,
}

/// Decoration placement in CSS px relative to the baseline.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Decorations {
    /// Distance below the baseline to the top of the underline.
    pub underline_offset: f32,
    pub underline_thickness: f32,
    /// Distance above the baseline to the top of the line-through.
    pub strike_offset: f32,
    pub strike_thickness: f32,
}

fn table() -> &'static HashMap<&'static str, Raw> {
    static TABLE: OnceLock<HashMap<&'static str, Raw>> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut out = HashMap::new();
        for (family, _, bytes) in FACES {
            if out.contains_key(family) {
                continue;
            }
            let Ok(face) = ttf_parser::Face::parse(bytes, 0) else {
                continue;
            };
            let underline = face.underline_metrics();
            let strike = face.strikeout_metrics();
            out.insert(
                *family,
                Raw {
                    upem: face.units_per_em() as f32,
                    line_gap: face.line_gap() as f32,
                    underline_position: underline.map_or(-100., |m| m.position as f32),
                    underline_thickness: underline.map_or(50., |m| m.thickness as f32),
                    strike_position: strike.map_or(500., |m| m.position as f32),
                    strike_thickness: strike.map_or(50., |m| m.thickness as f32),
                    zero_advance: face
                        .glyph_index('0')
                        .and_then(|g| face.glyph_hor_advance(g))
                        .map_or(0., f32::from),
                },
            );
        }
        out
    })
}

fn raw(family: &Family) -> Option<Raw> {
    table().get(family.primary().as_ref()).copied()
}

/// `hhea.lineGap` at `size` px; zero for faces outside the bundle.
pub fn line_gap(family: &Family, size: f32) -> f32 {
    raw(family).map_or(0., |r| r.line_gap / r.upem * size)
}

/// The `0` glyph's advance at `size` px: CSS `1ch`. Zero for faces outside the bundle.
pub fn zero_advance(family: &Family, size: f32) -> f32 {
    raw(family).map_or(0., |r| r.zero_advance / r.upem * size)
}

pub fn decorations(family: &Family, size: f32) -> Decorations {
    let r = raw(family).unwrap_or(Raw {
        upem: 1000.,
        line_gap: 0.,
        underline_position: -100.,
        underline_thickness: 50.,
        strike_position: 300.,
        strike_thickness: 50.,
        zero_advance: 0.,
    });
    let k = size / r.upem;
    Decorations {
        underline_offset: -r.underline_position * k - r.underline_thickness * k / 2.,
        underline_thickness: (r.underline_thickness * k).max(1.),
        strike_offset: r.strike_position * k + r.strike_thickness * k / 2.,
        strike_thickness: (r.strike_thickness * k).max(1.),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_families_have_no_line_gap() {
        for f in [Family::Ui, Family::Mono, Family::Display] {
            assert!(raw(&f).is_some(), "{f:?} bundled");
            assert_eq!(line_gap(&f, 13.), 0.);
        }
    }
}
