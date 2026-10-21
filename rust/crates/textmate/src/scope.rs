//! Command applicability by grammar scope and file glob
//! (`internal/bundles/scope.go`), with Go's `path.Match` glob semantics.

/// Whether a command scoped to `want_scope` / `file_glob` applies to a file.
/// Unscoped commands match everything; scopes match by TextMate prefix
/// (comma-separated alternatives); globs match the base name or slash path.
pub fn matches(want_scope: &str, file_glob: &str, file_scope: &str, file_path: &str) -> bool {
    if want_scope.is_empty() && file_glob.is_empty() {
        return true;
    }
    if !want_scope.is_empty() && !scope_prefix_match(want_scope, file_scope) {
        return false;
    }
    if !file_glob.is_empty() && !glob_match(file_glob, file_path) {
        return false;
    }
    true
}

pub fn scope_prefix_match(want: &str, have: &str) -> bool {
    let want = want.trim();
    let have = have.trim();
    if want.is_empty() {
        return true;
    }
    if have.is_empty() {
        return false;
    }
    want.split(',')
        .map(str::trim)
        .filter(|a| !a.is_empty())
        .any(|alt| {
            have == alt || (have.starts_with(alt) && have.as_bytes().get(alt.len()) == Some(&b'.'))
        })
}

/// Any pattern matches (empty list matches every path).
pub fn match_file_glob(patterns: &[String], file_path: &str) -> bool {
    patterns.is_empty() || patterns.iter().any(|p| glob_match(p, file_path))
}

pub fn glob_match(pattern: &str, file_path: &str) -> bool {
    let pattern = pattern.trim();
    if pattern.is_empty() {
        return true;
    }
    let alts = expand_braces(pattern);
    if alts.len() > 1 {
        return alts.iter().any(|p| glob_match(p, file_path));
    }
    let file_path = file_path.replace('\\', "/");
    let base = go_base(&file_path);
    if path_match(pattern, &base) || path_match(pattern, &file_path) {
        return true;
    }
    if let Some(suf) = pattern.strip_prefix("**/") {
        if path_match(suf, &base) {
            return true;
        }
        for (i, c) in file_path.char_indices() {
            if c == '/' && path_match(suf, &file_path[i + 1..]) {
                return true;
            }
        }
    }
    false
}

/// Go `filepath.Base` for slash paths.
fn go_base(p: &str) -> String {
    if p.is_empty() {
        return ".".into();
    }
    let trimmed = p.trim_end_matches('/');
    if trimmed.is_empty() {
        return "/".into();
    }
    trimmed.rsplit('/').next().unwrap_or(trimmed).to_string()
}

fn expand_braces(pattern: &str) -> Vec<String> {
    let (Some(start), Some(end)) = (pattern.find('{'), pattern.rfind('}')) else {
        return vec![pattern.to_string()];
    };
    if end <= start {
        return vec![pattern.to_string()];
    }
    let (prefix, suffix) = (&pattern[..start], &pattern[end + 1..]);
    let out: Vec<String> = pattern[start + 1..end]
        .split(',')
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(|p| format!("{prefix}{p}{suffix}"))
        .collect();
    if out.is_empty() {
        vec![pattern.to_string()]
    } else {
        out
    }
}

/// Go `path.Match`; a malformed pattern never matches.
pub fn path_match(pattern: &str, name: &str) -> bool {
    let mut pattern = pattern;
    let mut name = name;
    'pattern: while !pattern.is_empty() {
        let (star, chunk, rest) = scan_chunk(pattern);
        pattern = rest;
        if star && chunk.is_empty() {
            return !name.contains('/');
        }
        match match_chunk(chunk, name) {
            Ok(Some(t)) if t.is_empty() || !pattern.is_empty() => {
                name = t;
                continue;
            }
            Err(()) => return false,
            _ => {}
        }
        if star {
            let mut i = 0;
            while i < name.len() && name.as_bytes()[i] != b'/' {
                let c = name[i..].chars().next().map_or(1, char::len_utf8);
                i += c;
                match match_chunk(chunk, &name[i..]) {
                    Ok(Some(t)) => {
                        if pattern.is_empty() && !t.is_empty() {
                            continue;
                        }
                        name = t;
                        continue 'pattern;
                    }
                    Err(()) => return false,
                    Ok(None) => {}
                }
            }
        }
        return false;
    }
    name.is_empty()
}

fn scan_chunk(pattern: &str) -> (bool, &str, &str) {
    let mut p = pattern;
    let mut star = false;
    while p.starts_with('*') {
        p = &p[1..];
        star = true;
    }
    let b = p.as_bytes();
    let mut in_range = false;
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'\\' if i + 1 < b.len() => i += 1,
            b'\\' => {}
            b'[' => in_range = true,
            b']' => in_range = false,
            b'*' if !in_range => break,
            _ => {}
        }
        i += 1;
    }
    (star, &p[..i], &p[i..])
}

fn get_esc(chunk: &str) -> Result<(char, &str), ()> {
    let mut c = chunk;
    if c.is_empty() || c.starts_with('-') || c.starts_with(']') {
        return Err(());
    }
    if let Some(rest) = c.strip_prefix('\\') {
        c = rest;
        if c.is_empty() {
            return Err(());
        }
    }
    let ch = c.chars().next().ok_or(())?;
    let rest = &c[ch.len_utf8()..];
    if rest.is_empty() {
        return Err(());
    }
    Ok((ch, rest))
}

fn match_chunk<'a>(chunk: &str, s: &'a str) -> Result<Option<&'a str>, ()> {
    let mut chunk = chunk;
    let mut s = s;
    let mut failed = false;
    while !chunk.is_empty() {
        if !failed && s.is_empty() {
            failed = true;
        }
        match chunk.as_bytes()[0] {
            b'[' => {
                let mut r = '\0';
                if !failed {
                    r = s.chars().next().unwrap_or('\0');
                    s = &s[r.len_utf8()..];
                }
                chunk = &chunk[1..];
                let negated = chunk.starts_with('^');
                if negated {
                    chunk = &chunk[1..];
                }
                let mut matched = false;
                let mut nrange = 0;
                loop {
                    if !chunk.is_empty() && chunk.starts_with(']') && nrange > 0 {
                        chunk = &chunk[1..];
                        break;
                    }
                    let (lo, rest) = get_esc(chunk)?;
                    chunk = rest;
                    let mut hi = lo;
                    if let Some(rest) = chunk.strip_prefix('-') {
                        let (h, rest) = get_esc(rest)?;
                        hi = h;
                        chunk = rest;
                    }
                    if lo <= r && r <= hi {
                        matched = true;
                    }
                    nrange += 1;
                }
                if matched == negated {
                    failed = true;
                }
            }
            b'?' => {
                if !failed {
                    if s.starts_with('/') {
                        failed = true;
                    }
                    let c = s.chars().next().map_or(0, char::len_utf8);
                    s = &s[c..];
                }
                chunk = &chunk[1..];
            }
            b'\\' => {
                chunk = &chunk[1..];
                if chunk.is_empty() {
                    return Err(());
                }
                let c = chunk.chars().next().ok_or(())?;
                if !failed {
                    if !s.starts_with(c) {
                        failed = true;
                    }
                    s = s.get(c.len_utf8()..).unwrap_or("");
                }
                chunk = &chunk[c.len_utf8()..];
            }
            _ => {
                let c = chunk.chars().next().ok_or(())?;
                if !failed {
                    if !s.starts_with(c) {
                        failed = true;
                    }
                    s = s.get(c.len_utf8()..).unwrap_or("");
                }
                chunk = &chunk[c.len_utf8()..];
            }
        }
    }
    if failed { Ok(None) } else { Ok(Some(s)) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unscoped_commands_match_everything() {
        assert!(matches("", "", "", ""));
        assert!(matches("", "", "source.go", "main.go"));
    }

    #[test]
    fn grammar_prefix() {
        assert!(matches("source.go", "", "source.go", "a.go"));
        assert!(matches("source.go", "", "source.go.string", "a.go"));
        assert!(!matches("source.go", "", "source.python", "a.py"));
        assert!(!matches("source.go", "", "", "a.go"));
    }

    #[test]
    fn file_globs() {
        assert!(matches("", "*.go", "", "pkg/main.go"));
        assert!(!matches("", "*.go", "", "readme.md"));
        assert!(matches("", "**/internal/*.go", "", "foo/internal/x.go"));
        assert!(matches("", "*.{js,ts,tsx}", "", "src/app.tsx"));
        assert!(!matches("", "*.{js,ts,tsx}", "", "src/app.go"));
        assert!(match_file_glob(&[], "anything.go"));
        assert!(match_file_glob(&["*.go".into(), "*.mod".into()], "go.mod"));
        assert!(!match_file_glob(&["*.go".into()], "readme.md"));
    }

    #[test]
    fn scope_and_glob_together() {
        assert!(matches("source.go", "*.go", "source.go", "main.go"));
        assert!(!matches("source.go", "*.go", "source.go", "main.py"));
        assert!(!matches("source.go", "*.go", "source.python", "main.go"));
    }

    #[test]
    fn comma_scope_or() {
        let scope = "text.html, source.svelte, source.vue, source.tsx, source.js.jsx, source.astro";
        let glob = "*.{html,htm,svelte,vue,tsx,jsx,astro,mdx,hbs,handlebars}";
        for (s, p, want) in [
            ("source.svelte", "App.svelte", true),
            ("text.html.basic", "index.html", true),
            ("text.html.vue", "App.vue", true),
            ("source.tsx", "App.tsx", true),
            ("source.js.jsx", "App.jsx", true),
            ("source.astro", "index.astro", true),
            ("text.html.markdown", "page.mdx", true),
            ("text.html.handlebars", "page.hbs", true),
            ("source.go", "main.go", false),
            ("source.ts", "app.ts", false),
            ("source.js", "app.js", false),
            ("text.html.markdown", "readme.md", false),
            ("source.tsx", "App.go", false),
        ] {
            assert_eq!(matches(scope, glob, s, p), want, "{s} {p}");
        }
    }

    #[test]
    fn go_path_match_semantics() {
        assert!(path_match("a*b", "axxb"));
        assert!(!path_match("a*b", "a/b"));
        assert!(path_match("a?c", "abc"));
        assert!(path_match("[a-c]x", "bx"));
        assert!(!path_match("[^a-c]x", "bx"));
        assert!(path_match("\\*", "*"));
        assert!(!path_match("[", "a"));
        assert!(path_match("*.go", "main.go"));
        assert!(!path_match("*.go", "dir/main.go"));
    }
}
