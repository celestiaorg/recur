//! Diff code highlighting with the bundle grammars and theme the editor uses. Each side of a
//! hunk keeps its own grammar state: the old side runs through deletions, the new side through
//! additions, and context lines carry both.

use std::collections::HashMap;

use gpui_kit::Rgba;
use recur_domain::DiffLine;
use recur_editor::color::parse_css_color;
use recur_textmate::{Grammar, StateStack, paint_for_meta};

use crate::html::text_content;

/// Highlighted lines per card: the diff view's initial line cap, so the first screen is
/// uniformly styled. At most `REVIEW_HYDRATE_MAX` cards mount, so a patch highlights at most
/// 4,000 lines.
pub const MAX_HIGHLIGHT_LINES: usize = 500;

/// Longer lines (minified code) stay plain.
const MAX_LINE_BYTES: usize = 20_000;

/// A run of code text in its theme color (`None`: the editor foreground) and `tm_font` bits.
#[derive(Clone, Debug, PartialEq)]
pub struct Run {
    pub text: String,
    pub color: Option<Rgba>,
    pub font_style: u32,
}

/// A line's runs and the markup they were made from (`None`: not highlighted).
pub type Highlighted = Vec<Option<(String, Vec<Run>)>>;

/// The lines [`highlight_lines`] reads: through the [`MAX_HIGHLIGHT_LINES`]th code line.
pub fn budgeted(lines: &[DiffLine]) -> &[DiffLine] {
    let end = lines
        .iter()
        .enumerate()
        .filter(|(_, l)| l.r#type != "hunk")
        .nth(MAX_HIGHLIGHT_LINES)
        .map_or(lines.len(), |(i, _)| i);
    &lines[..end]
}

/// Each line's runs, index for index, painted as the editor paints them from the active theme's
/// `color_map` on `pane_bg`. Hunks restart both sides at the grammar's top level.
pub fn highlight_lines(
    grammar: &Grammar,
    lines: &[DiffLine],
    color_map: &[String],
    pane_bg: &str,
) -> Highlighted {
    let mut paints: HashMap<u32, (Option<Rgba>, u32)> = HashMap::new();
    let mut paint = |meta: u32| {
        *paints.entry(meta).or_insert_with(|| {
            let p = paint_for_meta(meta, false, color_map, &[], pane_bg);
            let color = p
                .foreground
                .and_then(|i| color_map.get(i as usize))
                .and_then(|c| parse_css_color(c));
            (color, p.font_style)
        })
    };
    let (mut old, mut new): (Option<StateStack>, Option<StateStack>) = (None, None);
    lines
        .iter()
        .map(|l| {
            let ctx = l.r#type == "ctx";
            let stack = match l.r#type.as_str() {
                "hunk" => {
                    (old, new) = (None, None);
                    return None;
                }
                "del" => &mut old,
                "add" | "ctx" => &mut new,
                _ => return None,
            };
            let text = text_content(&l.code_html);
            if text.is_empty() || text.len() > MAX_LINE_BYTES {
                return None;
            }
            let r = grammar.tokenize_line2(&text, stack.as_ref()).ok()?;
            *stack = Some(r.rule_stack);
            if ctx {
                old = new.clone();
            }
            let mut runs: Vec<Run> = Vec::new();
            for (k, pair) in r.tokens.chunks_exact(2).enumerate() {
                let start = (pair[0] as usize).min(text.len());
                let end = r.tokens.get(2 * k + 2).map_or(text.len(), |&e| e as usize);
                let piece = text.get(start..end.clamp(start, text.len()))?;
                let (color, font_style) = paint(pair[1]);
                match runs.last_mut() {
                    Some(last) if (last.color, last.font_style) == (color, font_style) => {
                        last.text.push_str(piece)
                    }
                    _ if piece.is_empty() => {}
                    _ => runs.push(Run {
                        text: piece.to_string(),
                        color,
                        font_style,
                    }),
                }
            }
            Some((l.code_html.clone(), runs))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use recur_textmate::{TmEngine, parse_theme_json};
    use std::sync::Arc;

    const FG: &str = "#d4d4d4";
    const KW: &str = "#c586c0";
    const STR: &str = "#ce9178";
    const NUM: &str = "#b5cea8";
    const FUNC: &str = "#dcdcaa";
    const CM: &str = "#6a9955";

    fn engine() -> (Arc<Grammar>, Vec<String>) {
        let mut e = TmEngine::new();
        let theme = parse_theme_json(
            &format!(
                r##"{{"name":"t","tokenColors":[
                    {{"settings":{{"foreground":"{FG}","background":"#1e1e1e"}}}},
                    {{"scope":"keyword","settings":{{"foreground":"{KW}"}}}},
                    {{"scope":"string","settings":{{"foreground":"{STR}"}}}},
                    {{"scope":"constant.numeric","settings":{{"foreground":"{NUM}"}}}},
                    {{"scope":"entity.name.function","settings":{{"foreground":"{FUNC}"}}}},
                    {{"scope":"comment","settings":{{"foreground":"{CM}","fontStyle":"italic"}}}}
                ]}}"##
            ),
            "t",
        )
        .expect("theme");
        let map = e.set_theme(theme.raw_theme);
        e.register_grammar(
            "source.t",
            r#"{"scopeName":"source.t","patterns":[
                {"begin":"/\\*","end":"\\*/","name":"comment.block.t"},
                {"match":"\"[^\"]*\"","name":"string.quoted.double.t"},
                {"match":"\\b(fn|let)\\b","name":"keyword.control.t"},
                {"match":"\\b[0-9]+\\b","name":"constant.numeric.t"},
                {"match":"==","name":"keyword.operator.t"},
                {"match":"\\b([a-z]+)(?=\\()","captures":{"1":{"name":"entity.name.function.t"}}}
            ]}"#,
            "t.json",
            &[],
        );
        assert!(matches!(e.ensure_grammar("source.t"), Ok(true)));
        (e.grammar("source.t").expect("compiled"), map)
    }

    fn highlight(lines: &[DiffLine]) -> Highlighted {
        let (g, map) = engine();
        highlight_lines(&g, lines, &map, "#1e1e1e")
    }

    fn line(kind: &str, html: &str) -> DiffLine {
        DiffLine {
            r#type: kind.into(),
            code_html: html.into(),
            ..Default::default()
        }
    }

    fn runs(h: &Option<(String, Vec<Run>)>) -> Vec<(String, Option<Rgba>)> {
        h.as_ref()
            .map(|(_, r)| r.iter().map(|r| (r.text.clone(), r.color)).collect())
            .unwrap_or_default()
    }

    fn c(s: &str) -> Option<Rgba> {
        parse_css_color(s)
    }

    #[test]
    fn highlights_in_theme_colors_per_side() {
        let lines = [
            line("hunk", "Lines 1–4"),
            line("ctx", "let a = f(1) == &#34;x&lt;&#34;"),
            line("del", "/* old"),
            line("add", "fn b"),
            line("del", "still */ 2"),
            line("hunk", "Lines 9–9"),
            line("add", "<span class=\"cm\">*/</span> 3"),
            line("ctx", ""),
        ];
        let h = highlight(&lines);
        assert_eq!(h.len(), lines.len());
        assert!(h[0].is_none() && h[5].is_none() && h[7].is_none());
        assert_eq!(
            runs(&h[1]),
            vec![
                ("let".into(), c(KW)),
                (" a = ".into(), c(FG)),
                ("f".into(), c(FUNC)),
                ("(".into(), c(FG)),
                ("1".into(), c(NUM)),
                (") ".into(), c(FG)),
                ("==".into(), c(KW)),
                (" ".into(), c(FG)),
                ("\"x<\"".into(), c(STR)),
            ]
        );
        assert_eq!(runs(&h[3])[0], ("fn".into(), c(KW)));
        assert_eq!(
            runs(&h[4]),
            vec![
                ("still */".into(), c(CM)),
                (" ".into(), c(FG)),
                ("2".into(), c(NUM)),
            ]
        );
        let cm = &h[4].as_ref().expect("highlighted").1[0];
        assert_ne!(cm.font_style & recur_textmate::tm_font::ITALIC, 0);
        assert_eq!(
            runs(&h[6]),
            vec![("*/ ".into(), c(FG)), ("3".into(), c(NUM))]
        );
        assert_eq!(
            h[6].as_ref().map(|(m, _)| m.as_str()),
            Some(lines[6].code_html.as_str())
        );
    }

    #[test]
    fn budget_stops_at_the_cap() {
        let mut lines = vec![line("hunk", "Lines 1–9")];
        lines.extend((0..MAX_HIGHLIGHT_LINES + 5).map(|_| line("add", "let")));
        let b = budgeted(&lines);
        assert_eq!(b.len(), MAX_HIGHLIGHT_LINES + 1);
        assert!(
            highlight(b)
                .iter()
                .skip(1)
                .all(|h| runs(h) == vec![("let".into(), c(KW))])
        );
        assert_eq!(budgeted(&lines[..3]).len(), 3);
    }
}
