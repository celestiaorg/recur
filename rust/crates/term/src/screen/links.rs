//! Clickable links: OSC 8 hyperlinks first, then web URLs in the text.

use alacritty_terminal::Term;
use alacritty_terminal::index::{Boundary, Direction, Point};
use alacritty_terminal::term::search::{RegexIter, RegexSearch};

pub(crate) const URL: &str = r#"https?://[^\s"'!*(){}|\\\^<>`]*[^\s"':,.!?{}|\\\^~\[\]`()<>]"#;

/// A link under the pointer: its target and the grid cells it covers (inclusive).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    pub text: String,
    pub start: Point,
    pub end: Point,
}

pub(crate) fn link_at<T>(term: &Term<T>, urls: &mut RegexSearch, point: Point) -> Option<Link> {
    if let Some(link) = term.grid()[point].hyperlink() {
        let uri = link.uri().to_string();
        if url::Url::parse(&uri).is_ok_and(|u| matches!(u.scheme(), "http" | "https")) {
            let same = |p: Point| term.grid()[p].hyperlink().as_ref() == Some(&link);
            let walk = |mut p: Point, step: fn(Point, &Term<T>) -> Point| loop {
                let next = step(p, term);
                if next == p || !same(next) {
                    return p;
                }
                p = next;
            };
            return Some(Link {
                text: uri,
                start: walk(point, |p, t| p.sub(t, Boundary::Grid, 1)),
                end: walk(point, |p, t| p.add(t, Boundary::Grid, 1)),
            });
        }
    }
    let (start, end) = (term.line_search_left(point), term.line_search_right(point));
    RegexIter::new(start, end, Direction::Right, term, urls)
        .find(|m| m.contains(&point))
        .map(|m| Link {
            text: term.bounds_to_string(*m.start(), *m.end()),
            start: *m.start(),
            end: *m.end(),
        })
}
