//! Inline formatting: the runs of a block's inline content with CSS whitespace collapsing
//! (`white-space: normal`) applied across element boundaries, as the browser lays them out.

use super::doc::{El, Node};
use super::style::sh_color;
use recur_ui::Color;

/// What paints a run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ink {
    Text,
    Muted,
    Cyan,
    Faint,
    /// A `sh-*` token color.
    Fixed(u32),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RunStyle {
    pub ink: Ink,
    pub weight: f32,
    pub italic: bool,
    pub strike: bool,
    pub underline: bool,
    /// Inside `<code>` (mono, sized, boxed).
    pub code: bool,
    /// The link this run belongs to, by index in the block.
    pub link: Option<usize>,
}

impl RunStyle {
    pub fn base(ink: Ink) -> RunStyle {
        RunStyle {
            ink,
            weight: 400.,
            italic: false,
            strike: false,
            underline: false,
            code: false,
            link: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Piece {
    Text(String, RunStyle),
    Break,
    Checkbox(bool),
    Image {
        src: String,
        alt: String,
        title: Option<String>,
        /// Inside a link (a badge): the outer `<a>` owns the click.
        link: Option<usize>,
    },
}

/// A link in a block: its href and title.
#[derive(Clone, Debug, PartialEq)]
pub struct Link {
    pub href: String,
    pub title: Option<String>,
}

/// One block's inline content, collapsed.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Inline {
    pub pieces: Vec<Piece>,
    pub links: Vec<Link>,
}

/// Options the enclosing block contributes.
#[derive(Clone, Copy)]
pub struct Context {
    pub ink: Ink,
    pub strong_weight: f32,
    pub em_muted: bool,
    pub weight: f32,
}

fn is_space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r' | '\u{c}')
}

struct Builder {
    out: Inline,
    /// The last emitted character was a collapsible space (or a line start).
    after_space: bool,
    cx: Context,
}

impl Builder {
    fn text(&mut self, s: &str, st: RunStyle) {
        let mut buf = String::new();
        for c in s.chars() {
            if is_space(c) {
                if !self.after_space {
                    buf.push(' ');
                    self.after_space = true;
                }
            } else {
                buf.push(c);
                self.after_space = false;
            }
        }
        if buf.is_empty() {
            return;
        }
        if let Some(Piece::Text(prev, ps)) = self.out.pieces.last_mut()
            && *ps == st
        {
            prev.push_str(&buf);
            return;
        }
        self.out.pieces.push(Piece::Text(buf, st));
    }

    fn walk(&mut self, nodes: &[Node], st: RunStyle) {
        for n in nodes {
            match n {
                Node::Text(t) => self.text(t, st),
                Node::El(e) => self.element(e, st),
            }
        }
    }

    fn element(&mut self, e: &El, st: RunStyle) {
        if e.svg {
            return;
        }
        let mut s = st;
        match e.tag.as_str() {
            "br" => {
                self.trim_trailing_space();
                self.out.pieces.push(Piece::Break);
                self.after_space = true;
                return;
            }
            "img" => {
                self.out.pieces.push(Piece::Image {
                    src: e.attr("src").unwrap_or("").to_string(),
                    alt: e.attr("alt").unwrap_or("").to_string(),
                    title: e.attr("title").map(str::to_string),
                    link: st.link,
                });
                self.after_space = false;
                return;
            }
            "input" => {
                if e.attr("type")
                    .is_some_and(|t| t.eq_ignore_ascii_case("checkbox"))
                {
                    self.out
                        .pieces
                        .push(Piece::Checkbox(e.attr("checked").is_some()));
                    self.after_space = false;
                }
                return;
            }
            "strong" | "b" => {
                s.weight = self.cx.strong_weight;
                if self.cx.em_muted {
                    s.ink = Ink::Text;
                }
            }
            "em" | "i" => {
                s.italic = true;
                if self.cx.em_muted {
                    s.ink = Ink::Muted;
                }
            }
            "del" | "s" => {
                s.strike = true;
                s.ink = Ink::Muted;
            }
            "code" => {
                s.code = true;
            }
            "a" => {
                if let Some(href) = e.attr("href") {
                    self.out.links.push(Link {
                        href: href.to_string(),
                        title: e.attr("title").map(str::to_string),
                    });
                    s.link = Some(self.out.links.len() - 1);
                    s.ink = Ink::Cyan;
                    s.underline = true;
                }
            }
            "span" => {
                if let Some(c) = e.attr("class") {
                    for cls in c.split_ascii_whitespace() {
                        if cls == "sh-prompt" {
                            s.ink = Ink::Faint;
                        } else if let Some(col) = sh_color(cls) {
                            s.ink = Ink::Fixed(rgb_u32(col));
                        }
                    }
                }
            }
            _ => {}
        }
        self.walk(&e.children, s);
    }

    fn trim_trailing_space(&mut self) {
        if let Some(Piece::Text(t, _)) = self.out.pieces.last_mut()
            && t.ends_with(' ')
        {
            t.pop();
            if t.is_empty() {
                self.out.pieces.pop();
            }
        }
    }
}

fn rgb_u32(c: Color) -> u32 {
    let [r, g, b, _] = c.to_rgba8();
    (r as u32) << 16 | (g as u32) << 8 | b as u32
}

/// Collapse `nodes` as one inline formatting context.
pub fn collect(nodes: &[Node], cx: Context) -> Inline {
    let mut b = Builder {
        out: Inline::default(),
        after_space: true,
        cx,
    };
    let mut st = RunStyle::base(cx.ink);
    st.weight = cx.weight;
    b.walk(nodes, st);
    b.trim_trailing_space();
    b.out
}

impl Inline {
    pub fn is_empty(&self) -> bool {
        self.pieces.is_empty()
    }

    /// Images only (with whitespace between them): laid out as a row of replaced boxes.
    pub fn images_only(&self) -> bool {
        !self.pieces.is_empty()
            && self.pieces.iter().all(|p| match p {
                Piece::Image { .. } => true,
                Piece::Text(t, _) => t.trim().is_empty(),
                Piece::Break => true,
                _ => false,
            })
            && self.pieces.iter().any(|p| matches!(p, Piece::Image { .. }))
    }

    /// The plain text (for find and copy).
    pub fn plain(&self) -> String {
        let mut s = String::new();
        for p in &self.pieces {
            match p {
                Piece::Text(t, _) => s.push_str(t),
                Piece::Break => s.push('\n'),
                _ => {}
            }
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::doc::Doc;

    fn cx() -> Context {
        Context {
            ink: Ink::Text,
            strong_weight: 600.,
            em_muted: true,
            weight: 400.,
        }
    }

    #[test]
    fn collapses_spaces_across_elements() {
        let d = Doc::parse("<p>  a  <strong> b</strong>\n c <br> d </p>");
        let Node::El(p) = &d.nodes[0] else { panic!() };
        let i = collect(&p.children, cx());
        assert_eq!(i.plain(), "a b c\nd");
        assert!(matches!(&i.pieces[1], Piece::Text(t, s) if t == "b" && s.weight == 600.));
    }
}
