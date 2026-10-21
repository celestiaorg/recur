//! XML plist parser, a port of vscode-textmate's `plist.ts` (`parsePLIST`).

use crate::node::Node;

#[derive(Debug, thiserror::Error)]
#[error("Near offset {offset}: {message} ~~~{context}~~~")]
pub struct PlistError {
    pub offset: usize,
    pub message: String,
    pub context: String,
}

#[derive(Clone, Copy, PartialEq)]
enum State {
    Root,
    Dict,
    Arr,
}

/// A container being filled. Dicts remember the pending `<key>`.
enum Frame {
    Dict(Vec<(String, Node)>),
    Arr(Vec<Node>),
}

struct Parser<'a> {
    content: &'a str,
    pos: usize,
    state: State,
    cur: Option<Frame>,
    root: Option<Node>,
    state_stack: Vec<State>,
    obj_stack: Vec<(Option<Frame>, Option<String>)>,
    cur_key: Option<String>,
}

impl Parser<'_> {
    fn fail<T>(&self, msg: &str) -> Result<T, PlistError> {
        let end = (self.pos + 50).min(self.content.len());
        let start = floor_char_boundary(self.content, self.pos);
        Err(PlistError {
            offset: self.pos,
            message: msg.to_string(),
            context: self.content[start..floor_char_boundary(self.content, end)].to_string(),
        })
    }

    fn skip_whitespace(&mut self) {
        let b = self.content.as_bytes();
        while self.pos < b.len() && matches!(b[self.pos], b' ' | b'\t' | b'\r' | b'\n') {
            self.pos += 1;
        }
    }

    fn advance_if_starts_with(&mut self, s: &str) -> bool {
        if self.content[self.pos..].starts_with(s) {
            self.pos += s.len();
            return true;
        }
        false
    }

    fn advance_until(&mut self, s: &str) {
        match self.content[self.pos..].find(s) {
            Some(i) => self.pos += i + s.len(),
            None => self.pos = self.content.len(),
        }
    }

    fn capture_until(&mut self, s: &str) -> String {
        match self.content[self.pos..].find(s) {
            Some(i) => {
                let r = self.content[self.pos..self.pos + i].to_string();
                self.pos += i + s.len();
                r
            }
            None => {
                let r = self.content[self.pos..].to_string();
                self.pos = self.content.len();
                r
            }
        }
    }

    fn push_state(&mut self, new_state: State, new_cur: Frame) {
        self.state_stack.push(self.state);
        self.obj_stack.push((self.cur.take(), self.cur_key.take()));
        self.state = new_state;
        self.cur = Some(new_cur);
    }

    /// Close the current container and store it into its parent.
    fn pop_state(&mut self) -> Result<(), PlistError> {
        let Some(state) = self.state_stack.pop() else {
            return self.fail("illegal state stack");
        };
        let (parent, parent_key) = self.obj_stack.pop().unwrap_or((None, None));
        let done = match self.cur.take() {
            Some(Frame::Dict(entries)) => Node::Map(entries),
            Some(Frame::Arr(items)) => Node::Array(items),
            None => Node::Null,
        };
        self.state = state;
        self.cur = parent;
        self.cur_key = None;
        match (&mut self.cur, parent_key) {
            (Some(Frame::Dict(entries)), Some(k)) => set_entry(entries, k, done),
            (Some(Frame::Arr(items)), _) => items.push(done),
            (None, _) => self.root = Some(done),
            (Some(Frame::Dict(_)), None) => {}
        }
        Ok(())
    }

    fn enter_container(&mut self, frame: Frame) -> Result<(), PlistError> {
        let new_state = match frame {
            Frame::Dict(_) => State::Dict,
            Frame::Arr(_) => State::Arr,
        };
        match self.state {
            State::Dict => {
                if self.cur_key.is_none() {
                    return self.fail("missing <key>");
                }
                // The key is restored with the parent frame and used on pop.
                let key = self.cur_key.take();
                self.state_stack.push(self.state);
                self.obj_stack.push((self.cur.take(), key));
                self.state = new_state;
                self.cur = Some(frame);
            }
            State::Arr | State::Root => {
                if self.state == State::Root {
                    self.root = None;
                }
                self.push_state(new_state, frame);
            }
        }
        Ok(())
    }

    fn leave(&mut self, want: State, tag: &str) -> Result<(), PlistError> {
        if self.state == want {
            self.pop_state()
        } else {
            self.fail(&format!("unexpected </{tag}>"))
        }
    }

    fn accept_key(&mut self, val: String) -> Result<(), PlistError> {
        match self.state {
            State::Dict => {
                if self.cur_key.is_some() {
                    return self.fail("too many <key>");
                }
                self.cur_key = Some(val);
                Ok(())
            }
            _ => self.fail("unexpected <key>"),
        }
    }

    fn accept_value(&mut self, val: Node) -> Result<(), PlistError> {
        match self.state {
            State::Dict => {
                let Some(k) = self.cur_key.take() else {
                    return self.fail("missing <key>");
                };
                if let Some(Frame::Dict(entries)) = &mut self.cur {
                    set_entry(entries, k, val);
                }
            }
            State::Arr => {
                if let Some(Frame::Arr(items)) = &mut self.cur {
                    items.push(val);
                }
            }
            State::Root => self.root = Some(val),
        }
        Ok(())
    }

    fn parse_open_tag(&mut self) -> (String, bool) {
        let mut r = self.capture_until(">");
        let mut closed = false;
        if r.ends_with('/') {
            closed = true;
            r.pop();
        }
        (r.trim().to_string(), closed)
    }

    fn parse_tag_value(&mut self, closed: bool) -> String {
        if closed {
            return String::new();
        }
        let val = self.capture_until("</");
        self.advance_until(">");
        escape_val(&val)
    }

    fn run(mut self) -> Result<Node, PlistError> {
        if self.content.starts_with('\u{feff}') {
            self.pos = '\u{feff}'.len_utf8();
        }
        let len = self.content.len();
        while self.pos < len {
            self.skip_whitespace();
            if self.pos >= len {
                break;
            }
            let ch = self.content.as_bytes()[self.pos];
            self.pos += 1;
            if ch != b'<' {
                return self.fail("expected <");
            }
            if self.pos >= len {
                return self.fail("unexpected end of input");
            }
            let peek = self.content.as_bytes()[self.pos];
            if peek == b'?' {
                self.pos += 1;
                self.advance_until("?>");
                continue;
            }
            if peek == b'!' {
                self.pos += 1;
                if self.advance_if_starts_with("--") {
                    self.advance_until("-->");
                    continue;
                }
                self.advance_until(">");
                continue;
            }
            if peek == b'/' {
                self.pos += 1;
                self.skip_whitespace();
                if self.advance_if_starts_with("plist") {
                    self.advance_until(">");
                    continue;
                }
                if self.advance_if_starts_with("dict") {
                    self.advance_until(">");
                    self.leave(State::Dict, "dict")?;
                    continue;
                }
                if self.advance_if_starts_with("array") {
                    self.advance_until(">");
                    self.leave(State::Arr, "array")?;
                    continue;
                }
                return self.fail("unexpected closed tag");
            }

            let (name, closed) = self.parse_open_tag();
            match name.as_str() {
                "dict" => {
                    self.enter_container(Frame::Dict(Vec::new()))?;
                    if closed {
                        self.leave(State::Dict, "dict")?;
                    }
                }
                "array" => {
                    self.enter_container(Frame::Arr(Vec::new()))?;
                    if closed {
                        self.leave(State::Arr, "array")?;
                    }
                }
                "key" => {
                    let v = self.parse_tag_value(closed);
                    self.accept_key(v)?;
                }
                "string" | "data" => {
                    let v = self.parse_tag_value(closed);
                    self.accept_value(Node::String(v))?;
                }
                "date" => {
                    let v = self.parse_tag_value(closed);
                    self.accept_value(Node::String(v))?;
                }
                "real" => {
                    let v = self.parse_tag_value(closed);
                    match parse_js_float(&v) {
                        Some(n) => self.accept_value(Node::Number(n))?,
                        None => return self.fail("cannot parse float"),
                    }
                }
                "integer" => {
                    let v = self.parse_tag_value(closed);
                    match crate::utils::js_parse_int(&v) {
                        Some(n) => self.accept_value(Node::Number(n as f64))?,
                        None => return self.fail("cannot parse integer"),
                    }
                }
                "true" => {
                    self.parse_tag_value(closed);
                    self.accept_value(Node::Bool(true))?;
                }
                "false" => {
                    self.parse_tag_value(closed);
                    self.accept_value(Node::Bool(false))?;
                }
                n if n.starts_with("plist") => {}
                n => return self.fail(&format!("unexpected opened tag {n}")),
            }
        }
        Ok(self.root.unwrap_or(Node::Null))
    }
}

fn set_entry(entries: &mut Vec<(String, Node)>, k: String, v: Node) {
    match entries.iter_mut().find(|(key, _)| *key == k) {
        Some(slot) => slot.1 = v,
        None => entries.push((k, v)),
    }
}

fn floor_char_boundary(s: &str, mut i: usize) -> usize {
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// `parseFloat`: the longest leading decimal literal.
fn parse_js_float(s: &str) -> Option<f64> {
    let t = s.trim_start();
    let b = t.as_bytes();
    let mut end = 0;
    let mut best = None;
    while end < b.len() {
        end += 1;
        if let Ok(v) = t[..end].parse::<f64>()
            && !t[..end].ends_with(['e', 'E', '+', '-'])
        {
            best = Some(v);
        }
    }
    best
}

fn escape_val(s: &str) -> String {
    let numeric = replace_numeric_entities(&replace_numeric_entities(s, "&#", 10), "&#x", 16);
    let mut out = String::with_capacity(numeric.len());
    let mut rest = numeric.as_str();
    'scan: while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        for (entity, ch) in [
            ("&amp;", '&'),
            ("&lt;", '<'),
            ("&gt;", '>'),
            ("&quot;", '"'),
            ("&apos;", '\''),
        ] {
            if let Some(after) = rest.strip_prefix(entity) {
                out.push(ch);
                rest = after;
                continue 'scan;
            }
        }
        out.push('&');
        rest = &rest[1..];
    }
    out.push_str(rest);
    out
}

fn replace_numeric_entities(s: &str, prefix: &str, radix: u32) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find(prefix) {
        out.push_str(&rest[..i]);
        let after = &rest[i + prefix.len()..];
        let digits: usize = after
            .char_indices()
            .take_while(|(_, c)| {
                if radix == 16 {
                    c.is_ascii_digit() || ('a'..='f').contains(c)
                } else {
                    c.is_ascii_digit()
                }
            })
            .map(|(_, c)| c.len_utf8())
            .sum();
        let decoded = (digits > 0 && after[digits..].starts_with(';'))
            .then(|| u32::from_str_radix(&after[..digits], radix).ok())
            .flatten()
            .and_then(char::from_u32);
        match decoded {
            Some(c) => {
                out.push(c);
                rest = &after[digits + 1..];
            }
            None => {
                out.push_str(prefix);
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

pub fn parse_plist(content: &str) -> Result<Node, PlistError> {
    Parser {
        content,
        pos: 0,
        state: State::Root,
        cur: None,
        root: None,
        state_stack: Vec::new(),
        obj_stack: Vec::new(),
        cur_key: None,
    }
    .run()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_grammar_plist() {
        let src = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>scopeName</key>
	<string>source.demo</string>
	<key>patterns</key>
	<array>
		<dict>
			<key>match</key>
			<string>a&amp;b&#65;&#x42;</string>
			<key>name</key>
			<string>x</string>
		</dict>
		<dict/>
	</array>
	<key>n</key><integer>12</integer>
	<key>t</key><true/>
</dict>
</plist>"#;
        let n = parse_plist(src).unwrap();
        assert_eq!(n.str_field("scopeName"), Some("source.demo"));
        let Some(Node::Array(pats)) = n.get("patterns") else {
            panic!("patterns")
        };
        assert_eq!(pats[0].str_field("match"), Some("a&bAB"));
        assert_eq!(pats[1], Node::Map(vec![]));
        assert_eq!(n.get("n"), Some(&Node::Number(12.0)));
        assert_eq!(n.get("t"), Some(&Node::Bool(true)));
    }

    #[test]
    fn reports_structure_errors() {
        assert!(parse_plist("<dict><string>x</string></dict>").is_err());
        assert!(parse_plist("x").is_err());
    }
}
