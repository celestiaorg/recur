//! Shell completion scripts, as cobra v1.10.2 generates them.
//!
//! The `*.tmpl` files are cobra's `fmt.Sprintf` format strings copied
//! verbatim (Copyright 2013-2023 The Cobra Authors, Apache License 2.0);
//! [`go_sprintf`] applies the subset of Go's fmt verbs they use.

const BASH: &str = include_str!("bash.tmpl");
const ZSH: &str = include_str!("zsh.tmpl");
const FISH: &str = include_str!("fish.tmpl");
const POWERSHELL: &str = include_str!("powershell.tmpl");

const ACTIVE_HELP_MARKER: &str = "_activeHelp_ ";

/// One `fmt.Sprintf` operand.
enum Arg<'a> {
    S(&'a str),
    D(u32),
}

/// The completion directive values the scripts branch on.
const DIRECTIVES: [u32; 6] = [1, 2, 4, 8, 16, 32];

fn comp_cmd(include_desc: bool) -> &'static str {
    if include_desc {
        "__complete"
    } else {
        "__completeNoDesc"
    }
}

fn name_for_var(name: &str) -> String {
    name.replace(['-', ':'], "_")
}

fn active_help_env_var(name: &str) -> String {
    format!("{name}_ACTIVE_HELP")
        .to_uppercase()
        .chars()
        .map(|c| {
            if c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

fn with_directives<'a>(head: Vec<Arg<'a>>, tail: Arg<'a>) -> Vec<Arg<'a>> {
    let mut args = head;
    args.extend(DIRECTIVES.iter().map(|d| Arg::D(*d)));
    args.push(tail);
    args
}

/// cobra `GenBashCompletionV2`.
pub fn bash(name: &str, include_desc: bool) -> String {
    let args = with_directives(
        vec![Arg::S(name), Arg::S(comp_cmd(include_desc))],
        Arg::S(ACTIVE_HELP_MARKER),
    );
    go_sprintf(BASH, &args)
}

/// cobra `GenZshCompletion` / `GenZshCompletionNoDesc`.
pub fn zsh(name: &str, include_desc: bool) -> String {
    let args = with_directives(
        vec![Arg::S(name), Arg::S(comp_cmd(include_desc))],
        Arg::S(ACTIVE_HELP_MARKER),
    );
    go_sprintf(ZSH, &args)
}

/// cobra `GenFishCompletion`.
pub fn fish(name: &str, include_desc: bool) -> String {
    let var = name_for_var(name);
    let help_var = active_help_env_var(name);
    let args = with_directives(
        vec![Arg::S(&var), Arg::S(name), Arg::S(comp_cmd(include_desc))],
        Arg::S(&help_var),
    );
    let mut out = go_sprintf(
        "# fish completion for %-36s -*- shell-script -*-\n",
        &[Arg::S(name)],
    );
    out.push_str(&go_sprintf(FISH, &args));
    out
}

/// cobra `GenPowerShellCompletion` / `GenPowerShellCompletionWithDesc`.
pub fn powershell(name: &str, include_desc: bool) -> String {
    let var = name_for_var(name);
    let help_var = active_help_env_var(name);
    let args = with_directives(
        vec![Arg::S(name), Arg::S(&var), Arg::S(comp_cmd(include_desc))],
        Arg::S(&help_var),
    );
    go_sprintf(POWERSHELL, &args)
}

/// Go `fmt.Sprintf` for `%%`, `%s` / `%d` with optional `[n]` argument
/// index and `-width` (padding counts runes, like Go).
fn go_sprintf(format: &str, args: &[Arg]) -> String {
    let mut out = String::with_capacity(format.len() + 256);
    let mut chars = format.chars().peekable();
    let mut next_arg = 0usize;
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        if chars.peek() == Some(&'%') {
            chars.next();
            out.push('%');
            continue;
        }
        let mut left = false;
        if chars.peek() == Some(&'-') {
            left = true;
            chars.next();
        }
        let mut width = 0usize;
        while let Some(d) = chars.peek().and_then(|c| c.to_digit(10)) {
            width = width * 10 + d as usize;
            chars.next();
        }
        if chars.peek() == Some(&'[') {
            chars.next();
            let mut n = 0usize;
            while let Some(d) = chars.peek().and_then(|c| c.to_digit(10)) {
                n = n * 10 + d as usize;
                chars.next();
            }
            chars.next();
            next_arg = n - 1;
        }
        let verb = chars.next().unwrap_or('%');
        let text = match (verb, args.get(next_arg)) {
            ('s', Some(Arg::S(s))) => (*s).to_string(),
            ('d', Some(Arg::D(d))) => d.to_string(),
            (v, _) => format!("%!{v}(BADARG)"),
        };
        next_arg += 1;
        let pad = width.saturating_sub(text.chars().count());
        if left {
            out.push_str(&text);
            out.push_str(&" ".repeat(pad));
        } else {
            out.push_str(&" ".repeat(pad));
            out.push_str(&text);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sprintf_subset() {
        assert_eq!(
            go_sprintf("%-6[1]s|%[2]d|%%|%[1]s", &[Arg::S("ab"), Arg::D(4)]),
            "ab    |4|%|ab"
        );
        assert_eq!(
            go_sprintf("# fish completion for %-36s -*-", &[Arg::S("recur")]).len(),
            61
        );
    }

    #[test]
    fn scripts_substitute_everything() {
        for (name, script) in [
            ("bash", bash("recur", true)),
            ("zsh", zsh("recur", false)),
            ("fish", fish("recur", true)),
            ("powershell", powershell("recur", true)),
        ] {
            assert!(!script.contains("%!"), "{name}: bad verb");
            assert!(!script.contains("%["), "{name}: unsubstituted index");
            assert!(script.contains("recur"), "{name}");
        }
        assert!(bash("recur", false).contains("__completeNoDesc"));
        assert!(fish("recur", true).contains("RECUR_ACTIVE_HELP"));
    }
}
