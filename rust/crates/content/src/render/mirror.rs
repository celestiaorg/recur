//! The line layout `recur_ui::Text` computes, recomputed from the same inputs for what the
//! element does not expose: where a link, a find match or a selection falls. Shaping goes
//! through the same GPUI text system calls per run, inline boxes add the same extents, lines
//! break at the same UAX #14 opportunities with the same tolerance, and line boxes are the
//! same unions of strut and runs, so the geometry matches what paints.

use std::ops::Range;

use gpui_kit::{FontFeatures, TextRun, Window, px};
use recur_ui::font_metrics::line_gap;
use recur_ui::fonts::Family;
use recur_ui::text::{InlineBox, LineHeight, line_metrics};
use unicode_linebreak::{BreakOpportunity, linebreaks};

/// One span as the text element resolves it.
#[derive(Clone, Debug, PartialEq)]
pub struct RunSpec {
    pub text: String,
    pub family: Family,
    pub size: f32,
    pub weight: f32,
    pub italic: bool,
    pub inline_box: Option<InlineBox>,
}

/// A text block's inputs.
#[derive(Clone, Debug, PartialEq)]
pub struct TextSpec {
    pub runs: Vec<RunSpec>,
    pub family: Family,
    pub size: f32,
    pub weight: f32,
    pub leading: f32,
    pub anywhere: bool,
}

#[derive(Clone, Debug)]
pub struct Line {
    /// Content byte range.
    pub range: Range<usize>,
    pub top: f32,
    pub height: f32,
    para: usize,
}

struct Para {
    range: Range<usize>,
    edges: Vec<(usize, f32)>,
    /// Where each inline box opens (byte, x before its left margin), as `recur_ui::text`.
    box_starts: Vec<(usize, f32)>,
}

impl Para {
    fn x_at(&self, byte: usize) -> f32 {
        match self.edges.binary_search_by_key(&byte, |e| e.0) {
            Ok(i) => self.edges[i].1,
            Err(i) => self.edges.get(i).map_or(0., |e| e.1),
        }
    }

    fn line_start(&self, byte: usize) -> f32 {
        if byte == 0 {
            return 0.;
        }
        self.box_starts
            .iter()
            .find(|(b, _)| *b == byte)
            .map_or_else(|| self.x_at(byte), |(_, x)| *x)
    }

    fn width(&self, text: &str, r: Range<usize>) -> f32 {
        let trimmed_end = r.start + text[r.clone()].trim_end_matches([' ', '\t']).len();
        let end = if trimmed_end == text.len() {
            self.edges.last().map_or(0., |e| e.1)
        } else {
            self.x_at(trimmed_end)
        };
        end - self.line_start(r.start)
    }
}

/// The laid-out lines of one text block.
pub struct Mirror {
    content: String,
    /// Each run's absolute byte range.
    runs: Vec<Range<usize>>,
    paras: Vec<Para>,
    pub lines: Vec<Line>,
}

fn font_extent(
    window: &Window,
    family: &Family,
    weight: f32,
    italic: bool,
    size: f32,
) -> (f32, f32, f32) {
    let ts = window.text_system();
    let id = ts.resolve_font(&family.font(weight, italic, FontFeatures::default()));
    (
        ts.ascent(id, px(size)).into(),
        -f32::from(ts.descent(id, px(size))),
        line_gap(family, size),
    )
}

impl Mirror {
    pub fn layout(spec: &TextSpec, width: f32, window: &Window) -> Mirror {
        let mut content = String::new();
        let mut runs = Vec::new();
        for r in &spec.runs {
            let start = content.len();
            content.push_str(&r.text);
            runs.push(start..content.len());
        }
        let mut paras = Vec::new();
        let mut start = 0;
        for piece in content.split('\n') {
            paras.push(shape_para(
                spec,
                &runs,
                &content,
                start..start + piece.len(),
                window,
            ));
            start += piece.len() + 1;
        }
        let eps = 0.5 / window.scale_factor();
        let lh = LineHeight::Scale(spec.leading);
        let (a, d, g) = font_extent(window, &spec.family, spec.weight, false, spec.size);
        let strut = line_metrics(a, d, g, spec.size, lh);
        let extents: Vec<(Range<usize>, f32, f32)> = spec
            .runs
            .iter()
            .zip(&runs)
            .map(|(r, range)| {
                let (a, d, g) = font_extent(window, &r.family, r.weight, r.italic, r.size);
                let m = line_metrics(a, d, g, r.size, lh);
                (range.clone(), m.baseline, m.height - m.baseline)
            })
            .collect();
        let mut lines = Vec::new();
        let mut top = 0.;
        for (pi, p) in paras.iter().enumerate() {
            let text = &content[p.range.clone()];
            for range in break_lines(p, text, width, spec.anywhere, eps) {
                let abs = p.range.start + range.start..p.range.start + range.end;
                let (mut above, mut below) = (strut.baseline, strut.height - strut.baseline);
                for (r, ra, rb) in &extents {
                    if r.start < abs.end && abs.start < r.end {
                        above = above.max(*ra);
                        below = below.max(*rb);
                    }
                }
                lines.push(Line {
                    range: abs,
                    top,
                    height: above + below,
                    para: pi,
                });
                top += above + below;
            }
        }
        Mirror {
            content,
            runs,
            paras,
            lines,
        }
    }

    /// The run (span) at a content byte.
    pub fn run_at(&self, byte: usize) -> Option<usize> {
        self.runs.iter().position(|r| r.contains(&byte))
    }

    /// The content byte under `(x, y)` (relative to the text box), if a glyph is there.
    pub fn hit(&self, x: f32, y: f32) -> Option<usize> {
        let line = self
            .lines
            .iter()
            .find(|l| y >= l.top && y < l.top + l.height)?;
        let p = &self.paras[line.para];
        let left = p.line_start(line.range.start - p.range.start);
        let mut prev: Option<(usize, f32)> = None;
        for (b, ex) in &p.edges {
            let abs = p.range.start + b;
            if abs < line.range.start || abs > line.range.end {
                continue;
            }
            let ex = ex - left;
            if let Some((pb, px0)) = prev
                && x >= px0
                && x < ex
            {
                return Some(pb);
            }
            prev = Some((abs, ex));
        }
        None
    }

    /// Rectangles (relative to the text box) covering a content byte range.
    pub fn rects(&self, range: Range<usize>) -> Vec<(f32, f32, f32, f32)> {
        let mut out = Vec::new();
        for l in &self.lines {
            let s = range.start.max(l.range.start);
            let e = range.end.min(l.range.end);
            if s >= e {
                continue;
            }
            let p = &self.paras[l.para];
            let left = p.line_start(l.range.start - p.range.start);
            let x0 = p.x_at(s - p.range.start) - left;
            let x1 = p.x_at(e - p.range.start) - left;
            out.push((x0, l.top, x1 - x0, l.height));
        }
        out
    }

    pub fn content(&self) -> &str {
        &self.content
    }
}

fn shape_para(
    spec: &TextSpec,
    runs: &[Range<usize>],
    content: &str,
    range: Range<usize>,
    window: &Window,
) -> Para {
    let mut pen = 0.;
    let mut edges = Vec::new();
    let mut box_starts = Vec::new();
    for (ri, run_range) in runs.iter().enumerate() {
        let s = run_range.start.max(range.start);
        let e = run_range.end.min(range.end);
        if s >= e {
            continue;
        }
        let r = &spec.runs[ri];
        let ib = r.inline_box;
        if let Some(b) = ib
            && s == run_range.start.max(range.start)
        {
            box_starts.push((s - range.start, pen));
            pen += b.margin_left + b.border + b.pad_left;
        }
        let text = &content[s..e];
        let font = r.family.font(r.weight, r.italic, FontFeatures::default());
        let shaped = window.text_system().shape_line(
            text.to_string().into(),
            px(r.size),
            &[TextRun {
                len: text.len(),
                font,
                color: Default::default(),
                background_color: None,
                underline: None,
                strikethrough: None,
            }],
            None,
        );
        let total = f32::from(shaped.width);
        let mut glyph_x: Vec<(usize, f32)> = shaped
            .runs
            .iter()
            .flat_map(|run| {
                run.glyphs
                    .iter()
                    .map(|g| (g.index, f32::from(g.position.x)))
            })
            .collect();
        glyph_x.sort_by_key(|(i, _)| *i);
        let mut gi = 0;
        for (bi, _) in text.char_indices() {
            while gi < glyph_x.len() && glyph_x[gi].0 < bi {
                gi += 1;
            }
            let x = glyph_x.get(gi).map_or(total, |g| g.1);
            edges.push((s - range.start + bi, pen + x));
        }
        pen += total;
        if let Some(b) = ib
            && e == run_range.end.min(range.end)
        {
            pen += b.margin_right + b.border + b.pad_right;
        }
    }
    edges.push((range.len(), pen));
    Para {
        range,
        edges,
        box_starts,
    }
}

fn break_lines(para: &Para, text: &str, max: f32, anywhere: bool, eps: f32) -> Vec<Range<usize>> {
    let ops: Vec<(usize, BreakOpportunity)> = linebreaks(text).collect();
    let mut lines = Vec::new();
    let mut start = 0;
    let mut last_fit: Option<usize> = None;
    let mut i = 0;
    while i < ops.len() {
        let (pos, kind) = ops[i];
        let w = para.width(text, start..pos);
        if w <= max + eps {
            if kind == BreakOpportunity::Mandatory {
                lines.push(start..pos);
                start = pos;
                last_fit = None;
            } else {
                last_fit = Some(pos);
            }
            i += 1;
            continue;
        }
        if let Some(b) = last_fit.take() {
            lines.push(start..b);
            start = b;
            continue;
        }
        if anywhere {
            let mut cut = start;
            for (bi, ch) in text[start..pos].char_indices() {
                let end = start + bi + ch.len_utf8();
                if para.width(text, start..end) > max + eps {
                    break;
                }
                cut = end;
            }
            if cut == start {
                cut = start + text[start..].chars().next().map_or(1, char::len_utf8);
            }
            lines.push(start..cut);
            start = cut;
            continue;
        }
        if kind == BreakOpportunity::Mandatory {
            lines.push(start..pos);
            start = pos;
        } else {
            last_fit = Some(pos);
        }
        i += 1;
    }
    if start < text.len() || lines.is_empty() {
        lines.push(start..text.len());
    }
    lines
}
