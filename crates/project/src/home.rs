//! Home-relative paths: `~` expansion, the home and user config dirs, and the
//! lexical clean / abs helpers they rely on.

/// `$HOME`, an error when unset or empty.
pub fn user_home_dir() -> Result<String, String> {
    match std::env::var("HOME") {
        Ok(v) if !v.is_empty() => Ok(v),
        _ => Err("$HOME is not defined".to_owned()),
    }
}

/// `$HOME/Library/Application Support` on macOS, `$XDG_CONFIG_HOME` (absolute)
/// or `$HOME/.config` elsewhere.
pub fn user_config_dir() -> Result<String, String> {
    let env = |k: &str| std::env::var(k).unwrap_or_default();
    if cfg!(target_os = "macos") {
        let home = env("HOME");
        if home.is_empty() {
            return Err("$HOME is not defined".to_owned());
        }
        return Ok(home + "/Library/Application Support");
    }
    let xdg = env("XDG_CONFIG_HOME");
    if xdg.is_empty() {
        let home = env("HOME");
        if home.is_empty() {
            return Err("neither $XDG_CONFIG_HOME nor $HOME are defined".to_owned());
        }
        return Ok(home + "/.config");
    }
    if !xdg.starts_with('/') {
        return Err("path in $XDG_CONFIG_HOME is relative".to_owned());
    }
    Ok(xdg)
}

/// Expands a leading `~` (`~` or `~/…`) to the home directory, leaving other
/// paths (and `~user`) unchanged.
pub fn expand_home(p: &str) -> String {
    if (p == "~" || p.starts_with("~/"))
        && let Ok(home) = user_home_dir()
    {
        return join(&home, &p[1..]);
    }
    p.to_owned()
}

/// Joins two elements: empty elements are skipped and the result is cleaned.
pub fn join(a: &str, b: &str) -> String {
    match (a.is_empty(), b.is_empty()) {
        (true, true) => String::new(),
        (true, false) => clean(b),
        (false, true) => clean(a),
        (false, false) => clean(&format!("{a}/{b}")),
    }
}

/// Joins relative paths onto the working directory and cleans the result
/// lexically (no symlink resolution).
pub fn abs(p: &str) -> std::io::Result<String> {
    if p.starts_with('/') {
        return Ok(clean(p));
    }
    let wd = std::env::current_dir()?;
    Ok(join(&wd.to_string_lossy(), p))
}

/// The shortest lexically equivalent path.
pub fn clean(p: &str) -> String {
    if p.is_empty() {
        return ".".to_owned();
    }
    let rooted = p.starts_with('/');
    let mut out: Vec<&str> = Vec::new();
    for part in p.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                if out.last().is_some_and(|l| *l != "..") {
                    out.pop();
                } else if !rooted {
                    out.push("..");
                }
            }
            _ => out.push(part),
        }
    }
    let body = out.join("/");
    match (rooted, body.is_empty()) {
        (true, _) => format!("/{body}"),
        (false, true) => ".".to_owned(),
        (false, false) => body,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_matches_go() {
        for (input, want) in [
            ("", "."),
            ("/", "/"),
            ("a/b/../c", "a/c"),
            ("//a//b/./", "/a/b"),
            ("../../x", "../../x"),
            ("/../x", "/x"),
            ("a/..", "."),
            ("./", "."),
            ("a/../..", ".."),
        ] {
            assert_eq!(clean(input), want, "clean({input:?})");
        }
    }

    #[test]
    fn join_matches_go() {
        assert_eq!(join("/home/u", "/code/app"), "/home/u/code/app");
        assert_eq!(join("/home/u", ""), "/home/u");
        assert_eq!(join("/home/u/", "/"), "/home/u");
        assert_eq!(join("", "x/"), "x");
    }

    #[test]
    fn expand_home_leaves_other_paths() {
        assert_eq!(expand_home("/abs"), "/abs");
        assert_eq!(expand_home("~user/x"), "~user/x");
        assert_eq!(expand_home("rel/~"), "rel/~");
        assert_eq!(expand_home(""), "");
        if let Ok(home) = user_home_dir() {
            assert_eq!(expand_home("~"), clean(&home));
            assert_eq!(expand_home("~/code/app"), join(&home, "code/app"));
        }
    }
}
