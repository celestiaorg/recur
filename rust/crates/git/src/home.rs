// simplify: duplicate of recur_project::expand_home until both land; dedupe on merge

/// Expands a leading `~` (or `~/`) to the user's home directory. Other `~user`
/// forms are left untouched.
pub fn expand_home(p: &str) -> String {
    if (p == "~" || p.starts_with("~/"))
        && let Some(home) = std::env::var_os("HOME").filter(|h| !h.is_empty())
    {
        return crate::gopath::join(&[&home.to_string_lossy(), &p[1..]]);
    }
    p.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expand_home_joins_home() {
        let Some(home) = std::env::var_os("HOME").filter(|h| !h.is_empty()) else {
            return;
        };
        let home = home.to_string_lossy().into_owned();
        assert_eq!(
            expand_home("~/code/x"),
            crate::gopath::join(&[&home, "code/x"])
        );
        assert_eq!(expand_home("~"), crate::gopath::clean(&home));
        assert_eq!(expand_home("/abs/path"), "/abs/path");
        assert_eq!(expand_home("~other/x"), "~other/x");
    }
}
