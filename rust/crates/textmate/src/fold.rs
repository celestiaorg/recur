//! Folding for TextMate languages (`cm-tm-fold.ts`): grammar
//! `foldingStartMarker` / `foldingStopMarker` when present, else indentation.
//! Ranges are 1-based lines: the fold hides everything after the end of
//! `start_line` through the end of `end_line`.

use crate::jsregex::JsRegex;
use crate::lang_data::{has_non_space, leading_indent};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TmFoldMarkers {
    pub start_source: String,
    pub stop_source: String,
}

pub struct CompiledFoldMarkers {
    pub start: JsRegex,
    pub stop: JsRegex,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FoldRange {
    pub start_line: usize,
    pub end_line: usize,
}

pub fn compile_fold_markers(src: Option<&TmFoldMarkers>) -> Option<CompiledFoldMarkers> {
    let src = src?;
    Some(CompiledFoldMarkers {
        start: JsRegex::new(&src.start_source, "")?,
        stop: JsRegex::new(&src.stop_source, "")?,
    })
}

/// Fold the deeper-indented block after `line`.
pub fn fold_by_indent(
    line_text: impl Fn(usize) -> String,
    line_count: usize,
    line: usize,
    tab_size: usize,
) -> Option<FoldRange> {
    let text = line_text(line);
    if !has_non_space(&text) {
        return None;
    }
    let start_indent = leading_indent(&text, tab_size);
    let mut fold_to = None;
    for n in line + 1..=line_count {
        let next = line_text(n);
        if !has_non_space(&next) {
            continue;
        }
        if leading_indent(&next, tab_size) > start_indent {
            fold_to = Some(n);
        } else {
            break;
        }
    }
    fold_to.map(|end_line| FoldRange {
        start_line: line,
        end_line,
    })
}

/// Marker fold: from a start-marker line to the line before its matching
/// stop marker (depth-counted).
pub fn fold_by_markers(
    line_text: impl Fn(usize) -> String,
    line_count: usize,
    line: usize,
    start: &JsRegex,
    stop: &JsRegex,
) -> Option<FoldRange> {
    if !start.test(&line_text(line)) {
        return None;
    }
    let mut depth = 1;
    for n in line + 1..=line_count {
        let text = line_text(n);
        if start.test(&text) {
            depth += 1;
        }
        if stop.test(&text) {
            depth -= 1;
            if depth == 0 {
                if n <= line + 1 {
                    return None;
                }
                return Some(FoldRange {
                    start_line: line,
                    end_line: n - 1,
                });
            }
        }
    }
    None
}

/// The fold service for a scope: markers first, then indentation.
pub fn textmate_fold(
    markers: Option<&CompiledFoldMarkers>,
    line_text: impl Fn(usize) -> String,
    line_count: usize,
    line: usize,
    tab_size: usize,
) -> Option<FoldRange> {
    if let Some(m) = markers
        && let Some(r) = fold_by_markers(&line_text, line_count, line, &m.start, &m.stop)
    {
        return Some(r);
    }
    fold_by_indent(line_text, line_count, line, tab_size)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(doc: &str) -> (Vec<String>, usize) {
        let v: Vec<String> = doc.split('\n').map(str::to_string).collect();
        let n = v.len();
        (v, n)
    }

    #[test]
    fn folds_a_deeper_indented_block() {
        let (v, n) = lines("func main() {\n\tx := 1\n\ty := 2\n}\n");
        assert_eq!(
            fold_by_indent(|i| v[i - 1].clone(), n, 1, 4),
            Some(FoldRange {
                start_line: 1,
                end_line: 3
            })
        );
        let (v, n) = lines("a\nb\n");
        assert_eq!(fold_by_indent(|i| v[i - 1].clone(), n, 1, 4), None);
    }

    #[test]
    fn folds_by_markers_with_nesting() {
        let m = compile_fold_markers(Some(&TmFoldMarkers {
            start_source: "/\\*\\*|\\{\\s*$".into(),
            stop_source: "\\*\\*/|^\\s*}".into(),
        }))
        .unwrap();
        let (v, n) = lines("object X {\n  val a = 1\n  val b = 2\n}\n");
        assert_eq!(
            fold_by_markers(|i| v[i - 1].clone(), n, 1, &m.start, &m.stop),
            Some(FoldRange {
                start_line: 1,
                end_line: 3
            })
        );
        let (v, n) = lines("outer {\n  inner {\n    x\n  }\n  y\n}\n");
        assert_eq!(
            fold_by_markers(|i| v[i - 1].clone(), n, 1, &m.start, &m.stop),
            Some(FoldRange {
                start_line: 1,
                end_line: 5
            })
        );
        assert_eq!(
            fold_by_markers(|i| v[i - 1].clone(), n, 2, &m.start, &m.stop),
            Some(FoldRange {
                start_line: 2,
                end_line: 3
            })
        );
    }

    #[test]
    fn falls_back_to_indent_without_markers() {
        let (v, n) = lines("func main() {\n\tx := 1\n\ty := 2\n}\n");
        assert_eq!(
            textmate_fold(None, |i| v[i - 1].clone(), n, 1, 4),
            Some(FoldRange {
                start_line: 1,
                end_line: 3
            })
        );
    }

    #[test]
    fn markers_js_rejects_are_not_compiled() {
        let crystal = TmFoldMarkers {
            start_source: "(?:^(\\s*+(module|class)))".into(),
            stop_source: "end".into(),
        };
        assert!(compile_fold_markers(Some(&crystal)).is_none());
    }
}
