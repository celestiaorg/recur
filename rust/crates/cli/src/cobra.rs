//! The parts of spf13/cobra v1.10.2 and spf13/pflag v1.0.10 the Go CLI
//! exercised, ported so help, usage, version, flag errors, the `help` and
//! `completion` commands and the `__complete` protocol are byte-identical.
//!
//! Commands and flags live in arenas; ids are indices. Like cobra, the
//! `help` / `version` flags are defined lazily (at execute, help, or
//! completion time), which changes how `Find` strips flags before them.

use std::io::Write;

use crate::Io;

pub type CmdId = usize;
pub type FlagId = usize;

/// How a command validates positional args.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArgsRule {
    /// `Args == nil`: cobra's `legacyArgs`.
    Legacy,
    Arbitrary,
    NoArgs,
    Exact(usize),
    Max(usize),
    Min(usize),
}

/// What a runnable command does; dispatched by [`crate::app`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Root,
    Open,
    BundlesTest,
    BundlesLint,
    BundlesCreate,
    Help,
    CompletionBash,
    CompletionZsh,
    CompletionFish,
    CompletionPowershell,
    Complete,
}

/// A completion function attached to a command (`ValidArgsFunction`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValidArgsFn {
    None,
    NoFile,
    HelpTopics,
}

#[derive(Debug, Clone)]
pub struct Cmd {
    pub use_line: String,
    pub short: String,
    pub long: String,
    pub example: String,
    pub aliases: Vec<String>,
    pub hidden: bool,
    pub action: Option<Action>,
    pub args: ArgsRule,
    pub version: String,
    pub disable_flag_parsing: bool,
    pub disable_flags_in_use_line: bool,
    pub valid_args_fn: ValidArgsFn,
    /// The root's `PersistentPreRunE` (Viper config init).
    pub persistent_pre_run: bool,
    parent: Option<CmdId>,
    children: Vec<CmdId>,
    max_name_len: usize,
    /// Flags defined on `Flags()` (plus lazily added help/version).
    local: Vec<FlagId>,
    persistent: Vec<FlagId>,
    /// Positional args from the last parse, and `--` position.
    parsed_args: Vec<String>,
    called_as: String,
    help_cmd: Option<CmdId>,
}

impl Cmd {
    pub fn new(use_line: &str, short: &str) -> Self {
        Cmd {
            use_line: use_line.to_string(),
            short: short.to_string(),
            long: String::new(),
            example: String::new(),
            aliases: Vec::new(),
            hidden: false,
            action: None,
            args: ArgsRule::Legacy,
            version: String::new(),
            disable_flag_parsing: false,
            disable_flags_in_use_line: false,
            valid_args_fn: ValidArgsFn::None,
            persistent_pre_run: false,
            parent: None,
            children: Vec::new(),
            max_name_len: 0,
            local: Vec::new(),
            persistent: Vec::new(),
            parsed_args: Vec::new(),
            called_as: String::new(),
            help_cmd: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlagKind {
    Bool,
    Str,
}

#[derive(Debug, Clone)]
pub struct Flag {
    pub name: String,
    pub shorthand: Option<u8>,
    pub usage: String,
    pub kind: FlagKind,
    def_value: String,
    pub value: String,
    pub changed: bool,
    /// Defined by cobra itself (help / version).
    by_cobra: bool,
}

impl Flag {
    fn no_opt_def_val(&self) -> &str {
        match self.kind {
            FlagKind::Bool => "true",
            FlagKind::Str => "",
        }
    }

    fn type_name(&self) -> &'static str {
        match self.kind {
            FlagKind::Bool => "bool",
            FlagKind::Str => "string",
        }
    }
}

/// A command failure as `Execute` reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CliError {
    /// `flag.ErrHelp`: print help, exit 0.
    Help,
    /// A plain error: printed red, exit 1.
    Msg(String),
    /// The CLI's `exitError`: optional message, explicit code.
    Exit(i32, Option<String>),
}

pub struct Cobra {
    pub cmds: Vec<Cmd>,
    pub flags: Vec<Flag>,
    pub root: CmdId,
}

impl Cobra {
    pub fn new(root: Cmd) -> Self {
        Cobra {
            cmds: vec![root],
            flags: Vec::new(),
            root: 0,
        }
    }

    pub fn add(&mut self, parent: CmdId, cmd: Cmd) -> CmdId {
        let id = self.cmds.len();
        self.cmds.push(cmd);
        self.add_command(parent, id);
        id
    }

    fn add_command(&mut self, parent: CmdId, child: CmdId) {
        self.cmds[child].parent = Some(parent);
        let name_len = self.name(child).len();
        let p = &mut self.cmds[parent];
        p.max_name_len = p.max_name_len.max(name_len);
        p.children.push(child);
    }

    fn remove_command(&mut self, parent: CmdId, child: CmdId) {
        self.cmds[parent].children.retain(|c| *c != child);
        self.cmds[child].parent = None;
        let max = self.cmds[parent]
            .children
            .iter()
            .map(|c| self.name(*c).len())
            .max()
            .unwrap_or(0);
        self.cmds[parent].max_name_len = max;
    }

    pub fn flag(
        &mut self,
        cmd: CmdId,
        persistent: bool,
        flag: (&str, Option<u8>, FlagKind, &str),
    ) -> FlagId {
        let (name, shorthand, kind, usage) = flag;
        let def = match kind {
            FlagKind::Bool => "false",
            FlagKind::Str => "",
        };
        let id = self.flags.len();
        self.flags.push(Flag {
            name: name.to_string(),
            shorthand,
            usage: usage.to_string(),
            kind,
            def_value: def.to_string(),
            value: def.to_string(),
            changed: false,
            by_cobra: false,
        });
        if persistent {
            self.cmds[cmd].persistent.push(id);
        } else {
            self.cmds[cmd].local.push(id);
        }
        id
    }

    pub fn parent(&self, c: CmdId) -> Option<CmdId> {
        self.cmds[c].parent
    }

    pub fn name(&self, c: CmdId) -> &str {
        let u = &self.cmds[c].use_line;
        u.split(' ').next().unwrap_or(u)
    }

    pub fn command_path(&self, c: CmdId) -> String {
        match self.cmds[c].parent {
            Some(p) => format!("{} {}", self.command_path(p), self.name(c)),
            None => self.name(c).to_string(),
        }
    }

    pub fn called_as(&self, c: CmdId) -> &str {
        &self.cmds[c].called_as
    }

    pub fn args(&self, c: CmdId) -> &[String] {
        &self.cmds[c].parsed_args
    }

    fn runnable(&self, c: CmdId) -> bool {
        self.cmds[c].action.is_some()
    }

    fn has_subcommands(&self, c: CmdId) -> bool {
        !self.cmds[c].children.is_empty()
    }

    /// Children sorted by name (cobra's `Commands()`).
    pub fn commands(&self, c: CmdId) -> Vec<CmdId> {
        let mut out = self.cmds[c].children.clone();
        out.sort_by(|a, b| self.name(*a).cmp(self.name(*b)));
        out
    }

    fn is_available(&self, c: CmdId) -> bool {
        if self.cmds[c].hidden {
            return false;
        }
        if let Some(p) = self.cmds[c].parent
            && self.cmds[p].help_cmd == Some(c)
        {
            return false;
        }
        self.runnable(c) || self.has_available_subcommands(c)
    }

    fn has_available_subcommands(&self, c: CmdId) -> bool {
        self.cmds[c].children.iter().any(|s| self.is_available(*s))
    }

    fn is_additional_help_topic(&self, c: CmdId) -> bool {
        if self.runnable(c) || self.cmds[c].hidden {
            return false;
        }
        self.cmds[c]
            .children
            .iter()
            .all(|s| self.is_additional_help_topic(*s))
    }

    fn name_padding(&self, c: CmdId) -> usize {
        match self.cmds[c].parent {
            Some(p) if self.cmds[p].max_name_len > 11 => self.cmds[p].max_name_len,
            _ => 11,
        }
    }

    fn command_path_padding(&self, c: CmdId) -> usize {
        match self.cmds[c].parent {
            Some(p) => {
                let max = self.cmds[p]
                    .children
                    .iter()
                    .map(|s| self.command_path(*s).len())
                    .max()
                    .unwrap_or(0);
                max.max(11)
            }
            None => 11,
        }
    }

    /// Parents' persistent flags, nearest parent first.
    fn parents_pflags(&self, c: CmdId) -> Vec<FlagId> {
        let mut out: Vec<FlagId> = Vec::new();
        let mut p = self.cmds[c].parent;
        while let Some(id) = p {
            for f in &self.cmds[id].persistent {
                if !out
                    .iter()
                    .any(|o| self.flags[*o].name == self.flags[*f].name)
                {
                    out.push(*f);
                }
            }
            p = self.cmds[id].parent;
        }
        out
    }

    /// `c.Flags()` after `mergePersistentFlags`.
    fn flag_set(&self, c: CmdId) -> Vec<FlagId> {
        let mut out: Vec<FlagId> = Vec::new();
        let candidates = self.cmds[c]
            .local
            .iter()
            .chain(self.cmds[c].persistent.iter())
            .copied()
            .chain(self.parents_pflags(c));
        for f in candidates {
            if !out
                .iter()
                .any(|o| self.flags[*o].name == self.flags[f].name)
            {
                out.push(f);
            }
        }
        out
    }

    fn local_flags(&self, c: CmdId) -> Vec<FlagId> {
        let parents = self.parents_pflags(c);
        self.flag_set(c)
            .into_iter()
            .filter(|f| !parents.contains(f))
            .collect()
    }

    fn inherited_flags(&self, c: CmdId) -> Vec<FlagId> {
        let local = self.local_flags(c);
        self.parents_pflags(c)
            .into_iter()
            .filter(|f| {
                !local
                    .iter()
                    .any(|l| self.flags[*l].name == self.flags[*f].name)
            })
            .collect()
    }

    fn lookup(&self, set: &[FlagId], name: &str) -> Option<FlagId> {
        set.iter().copied().find(|f| self.flags[*f].name == name)
    }

    fn lookup_short(&self, set: &[FlagId], short: u8) -> Option<FlagId> {
        set.iter()
            .copied()
            .find(|f| self.flags[*f].shorthand == Some(short))
    }

    /// The flag named `name` on `c` or a parent (cobra's `Flag`).
    pub fn flag_named(&self, c: CmdId, name: &str) -> Option<FlagId> {
        self.lookup(&self.flag_set(c), name)
    }

    pub fn flag_value(&self, id: FlagId) -> &str {
        &self.flags[id].value
    }

    pub fn flag_bool(&self, id: FlagId) -> bool {
        self.flags[id].value == "true"
    }

    fn init_default_help_flag(&mut self, c: CmdId) {
        if self.lookup(&self.flag_set(c), "help").is_some() {
            return;
        }
        let name = self.name(c).to_string();
        let usage = if name.is_empty() {
            "help for this command".to_string()
        } else {
            format!("help for {name}")
        };
        let id = self.flag(c, false, ("help", Some(b'h'), FlagKind::Bool, &usage));
        self.flags[id].by_cobra = true;
    }

    fn init_default_version_flag(&mut self, c: CmdId) {
        if self.cmds[c].version.is_empty() {
            return;
        }
        let set = self.flag_set(c);
        if self.lookup(&set, "version").is_some() {
            return;
        }
        let name = self.name(c).to_string();
        let usage = if name.is_empty() {
            "version for this command".to_string()
        } else {
            format!("version for {name}")
        };
        let short = if self.lookup_short(&set, b'v').is_none() {
            Some(b'v')
        } else {
            None
        };
        let id = self.flag(c, false, ("version", short, FlagKind::Bool, &usage));
        self.flags[id].by_cobra = true;
    }

    fn has_no_opt_def_val(&self, set: &[FlagId], name: &str) -> bool {
        self.lookup(set, name)
            .is_some_and(|f| !self.flags[f].no_opt_def_val().is_empty())
    }

    fn short_has_no_opt_def_val(&self, set: &[FlagId], name: &str) -> bool {
        let Some(first) = name.bytes().next() else {
            return false;
        };
        self.lookup_short(set, first)
            .is_some_and(|f| !self.flags[f].no_opt_def_val().is_empty())
    }

    fn strip_flags(&self, args: &[String], c: CmdId) -> Vec<String> {
        let set = self.flag_set(c);
        let mut commands = Vec::new();
        let mut args = args;
        while let Some((s, rest)) = args.split_first() {
            args = rest;
            if s == "--" {
                break;
            }
            let takes_value = (s.starts_with("--")
                && !s.contains('=')
                && !self.has_no_opt_def_val(&set, &s[2..]))
                || (s.starts_with('-')
                    && !s.contains('=')
                    && s.len() == 2
                    && !self.short_has_no_opt_def_val(&set, &s[1..]));
            if takes_value {
                if args.len() <= 1 {
                    break;
                }
                args = &args[1..];
                continue;
            }
            if !s.is_empty() && !s.starts_with('-') {
                commands.push(s.clone());
            }
        }
        commands
    }

    fn args_minus_first_x(&self, args: &[String], x: &str, c: CmdId) -> Vec<String> {
        let set = self.flag_set(c);
        let mut pos = 0;
        while pos < args.len() {
            let s = &args[pos];
            if s == "--" {
                break;
            }
            let takes_value = (s.starts_with("--")
                && !s.contains('=')
                && !self.has_no_opt_def_val(&set, &s[2..]))
                || (s.starts_with('-')
                    && !s.contains('=')
                    && s.len() == 2
                    && !self.short_has_no_opt_def_val(&set, &s[1..]));
            if takes_value {
                pos += 2;
                continue;
            }
            if !s.starts_with('-') && s == x {
                let mut out = args[..pos].to_vec();
                out.extend_from_slice(&args[pos + 1..]);
                return out;
            }
            pos += 1;
        }
        args.to_vec()
    }

    fn find_next(&mut self, c: CmdId, next: &str) -> Option<CmdId> {
        let hit =
            self.cmds[c].children.iter().copied().find(|s| {
                self.name(*s) == next || self.cmds[*s].aliases.iter().any(|a| a == next)
            })?;
        self.cmds[hit].called_as = next.to_string();
        Some(hit)
    }

    /// cobra's `Find`: the target command and its unconsumed args.
    pub fn find(&mut self, args: &[String]) -> (CmdId, Vec<String>, Option<CliError>) {
        let mut c = self.root;
        let mut a = args.to_vec();
        loop {
            let stripped = self.strip_flags(&a, c);
            let Some(next) = stripped.first() else { break };
            match self.find_next(c, next) {
                Some(cmd) => {
                    a = self.args_minus_first_x(&a, next, c);
                    c = cmd;
                }
                None => break,
            }
        }
        let err = if self.cmds[c].args == ArgsRule::Legacy {
            let stripped = self.strip_flags(&a, c);
            self.legacy_args(c, &stripped).err()
        } else {
            None
        };
        (c, a, err)
    }

    fn legacy_args(&self, c: CmdId, args: &[String]) -> Result<(), CliError> {
        if !self.has_subcommands(c) {
            return Ok(());
        }
        if self.cmds[c].parent.is_none()
            && let Some(first) = args.first()
        {
            return Err(CliError::Msg(format!(
                "unknown command {} for {}{}",
                go_quote(first),
                go_quote(&self.command_path(c)),
                self.find_suggestions(c, first)
            )));
        }
        Ok(())
    }

    fn find_suggestions(&self, c: CmdId, arg: &str) -> String {
        let mut suggestions = Vec::new();
        for s in &self.cmds[c].children {
            if !self.is_available(*s) {
                continue;
            }
            let name = self.name(*s);
            let by_distance = levenshtein_fold(arg, name) <= 2;
            let by_prefix = name.to_lowercase().starts_with(&arg.to_lowercase());
            if by_distance || by_prefix {
                suggestions.push(name.to_string());
            }
        }
        if suggestions.is_empty() {
            return String::new();
        }
        let mut out = String::from("\n\nDid you mean this?\n");
        for s in suggestions {
            out.push_str(&format!("\t{s}\n"));
        }
        out
    }

    fn validate_args(&self, c: CmdId, args: &[String]) -> Result<(), CliError> {
        let n = args.len();
        let msg = match self.cmds[c].args {
            ArgsRule::Legacy | ArgsRule::Arbitrary => return Ok(()),
            ArgsRule::NoArgs if n > 0 => format!(
                "unknown command {} for {}",
                go_quote(&args[0]),
                go_quote(&self.command_path(c))
            ),
            ArgsRule::Exact(want) if n != want => format!("accepts {want} arg(s), received {n}"),
            ArgsRule::Max(max) if n > max => format!("accepts at most {max} arg(s), received {n}"),
            ArgsRule::Min(min) if n < min => {
                format!("requires at least {min} arg(s), only received {n}")
            }
            _ => return Ok(()),
        };
        Err(CliError::Msg(msg))
    }

    /// pflag `Parse` over `c.Flags()`. Positional args land in
    /// `parsed_args`.
    pub fn parse_flags(&mut self, c: CmdId, args: &[String]) -> Result<(), CliError> {
        if self.cmds[c].disable_flag_parsing {
            return Ok(());
        }
        let set = self.flag_set(c);
        let mut positional = Vec::new();
        let mut args = args.to_vec();
        let res = self.parse_args(&set, &mut args, &mut positional);
        self.cmds[c].parsed_args = positional;
        res
    }

    fn parse_args(
        &mut self,
        set: &[FlagId],
        args: &mut Vec<String>,
        positional: &mut Vec<String>,
    ) -> Result<(), CliError> {
        while !args.is_empty() {
            let s = args.remove(0);
            if s.len() < 2 || !s.starts_with('-') {
                positional.push(s);
                continue;
            }
            if s.as_bytes()[1] == b'-' {
                if s.len() == 2 {
                    positional.append(args);
                    break;
                }
                self.parse_long_arg(set, &s, args)?;
            } else {
                let mut shorthands = s[1..].to_string();
                while !shorthands.is_empty() {
                    shorthands = self.parse_single_short_arg(set, &shorthands, args)?;
                }
            }
        }
        Ok(())
    }

    fn parse_long_arg(
        &mut self,
        set: &[FlagId],
        s: &str,
        args: &mut Vec<String>,
    ) -> Result<(), CliError> {
        let name = &s[2..];
        if name.is_empty() || name.starts_with('-') || name.starts_with('=') {
            return Err(CliError::Msg(format!("bad flag syntax: {s}")));
        }
        let (name, inline) = match name.split_once('=') {
            Some((n, v)) => (n, Some(v.to_string())),
            None => (name, None),
        };
        let Some(flag) = self.lookup(set, name) else {
            if name == "help" {
                return Err(CliError::Help);
            }
            return Err(CliError::Msg(format!("unknown flag: --{name}")));
        };
        let value = match inline {
            Some(v) => v,
            None if !self.flags[flag].no_opt_def_val().is_empty() => {
                self.flags[flag].no_opt_def_val().to_string()
            }
            None if !args.is_empty() => args.remove(0),
            None => return Err(CliError::Msg(format!("flag needs an argument: --{name}"))),
        };
        self.set_flag(flag, &value)
    }

    fn parse_single_short_arg(
        &mut self,
        set: &[FlagId],
        shorthands: &str,
        args: &mut Vec<String>,
    ) -> Result<String, CliError> {
        if shorthands.starts_with("test.") {
            return Ok(String::new());
        }
        let c = shorthands.as_bytes()[0];
        let Some(flag) = self.lookup_short(set, c) else {
            if c == b'h' {
                return Err(CliError::Help);
            }
            // pflag formats `rune(name[0])`: the byte, not a decoded char.
            return Err(CliError::Msg(format!(
                "unknown shorthand flag: {} in -{shorthands}",
                go_quote_rune(c as char)
            )));
        };
        let rest = shorthands[1..].to_string();
        let (value, out) = if shorthands.len() > 2 && shorthands.as_bytes()[1] == b'=' {
            (shorthands[2..].to_string(), String::new())
        } else if !self.flags[flag].no_opt_def_val().is_empty() {
            (self.flags[flag].no_opt_def_val().to_string(), rest)
        } else if shorthands.len() > 1 {
            (rest, String::new())
        } else if !args.is_empty() {
            (args.remove(0), String::new())
        } else {
            return Err(CliError::Msg(format!(
                "flag needs an argument: {} in -{shorthands}",
                go_quote_rune(c as char)
            )));
        };
        self.set_flag(flag, &value)?;
        Ok(out)
    }

    fn set_flag(&mut self, id: FlagId, value: &str) -> Result<(), CliError> {
        let f = &mut self.flags[id];
        match f.kind {
            FlagKind::Str => f.value = value.to_string(),
            FlagKind::Bool => match parse_go_bool(value) {
                Some(b) => f.value = b.to_string(),
                None => {
                    let name = match f.shorthand {
                        Some(s) => format!("-{}, --{}", s as char, f.name),
                        None => format!("--{}", f.name),
                    };
                    return Err(CliError::Msg(format!(
                        "invalid argument {} for {} flag: strconv.ParseBool: parsing {}: invalid syntax",
                        go_quote(value),
                        go_quote(&name),
                        go_quote(value)
                    )));
                }
            },
        }
        f.changed = true;
        Ok(())
    }

    fn use_line(&self, c: CmdId) -> String {
        let mut line = match self.cmds[c].parent {
            Some(p) => format!("{} {}", self.command_path(p), self.cmds[c].use_line),
            None => self.cmds[c].use_line.clone(),
        };
        if self.cmds[c].disable_flags_in_use_line {
            return line;
        }
        if !self.flag_set(c).is_empty() && !line.contains("[flags]") {
            line.push_str(" [flags]");
        }
        line
    }

    /// pflag `FlagUsages` over `ids` (sorted by name).
    fn flag_usages(&self, ids: &[FlagId]) -> String {
        let mut ids = ids.to_vec();
        ids.sort_by(|a, b| self.flags[*a].name.cmp(&self.flags[*b].name));
        let mut lines = Vec::new();
        let mut maxlen = 0;
        for id in ids {
            let f = &self.flags[id];
            let mut line = match f.shorthand {
                Some(s) => format!("  -{}, --{}", s as char, f.name),
                None => format!("      --{}", f.name),
            };
            let (varname, usage) = unquote_usage(f);
            if !varname.is_empty() {
                line.push(' ');
                line.push_str(&varname);
            }
            line.push('\0');
            maxlen = maxlen.max(line.len());
            line.push_str(&usage);
            if f.value_is_nonzero_default() {
                match f.kind {
                    FlagKind::Str => {
                        line.push_str(&format!(" (default {})", go_quote(&f.def_value)))
                    }
                    FlagKind::Bool => line.push_str(&format!(" (default {})", f.def_value)),
                }
            }
            lines.push(line);
        }
        let mut buf = String::new();
        for line in lines {
            let sidx = line.find('\0').unwrap_or(line.len());
            let spacing = " ".repeat(maxlen - sidx);
            let usage = line[sidx + 1..].replace('\n', &format!("\n{}", " ".repeat(maxlen + 2)));
            buf.push_str(&format!("{} {} {}\n", &line[..sidx], spacing, usage));
        }
        buf
    }

    /// cobra's `defaultUsageFunc`.
    pub fn usage_string(&self, c: CmdId) -> String {
        let mut w = String::from("Usage:");
        if self.runnable(c) {
            w.push_str(&format!("\n  {}", self.use_line(c)));
        }
        if self.has_available_subcommands(c) {
            w.push_str(&format!("\n  {} [command]", self.command_path(c)));
        }
        if !self.cmds[c].aliases.is_empty() {
            let mut names = vec![self.name(c).to_string()];
            names.extend(self.cmds[c].aliases.iter().cloned());
            w.push_str(&format!("\n\nAliases:\n  {}", names.join(", ")));
        }
        if !self.cmds[c].example.is_empty() {
            w.push_str(&format!("\n\nExamples:\n{}", self.cmds[c].example));
        }
        if self.has_available_subcommands(c) {
            w.push_str("\n\nAvailable Commands:");
            for s in self.commands(c) {
                if self.is_available(s) || self.name(s) == "help" {
                    w.push_str(&format!(
                        "\n  {} {}",
                        rpad(self.name(s), self.name_padding(s)),
                        self.cmds[s].short
                    ));
                }
            }
        }
        let local = self.local_flags(c);
        if !local.is_empty() {
            w.push_str(&format!(
                "\n\nFlags:\n{}",
                trim_right_space(&self.flag_usages(&local))
            ));
        }
        let inherited = self.inherited_flags(c);
        if !inherited.is_empty() {
            w.push_str(&format!(
                "\n\nGlobal Flags:\n{}",
                trim_right_space(&self.flag_usages(&inherited))
            ));
        }
        if self.cmds[c]
            .children
            .iter()
            .any(|s| self.is_additional_help_topic(*s))
        {
            w.push_str("\n\nAdditional help topics:");
            for s in self.commands(c) {
                if self.is_additional_help_topic(s) {
                    w.push_str(&format!(
                        "\n  {} {}",
                        rpad(&self.command_path(s), self.command_path_padding(s)),
                        self.cmds[s].short
                    ));
                }
            }
        }
        if self.has_available_subcommands(c) {
            w.push_str(&format!(
                "\n\nUse \"{} [command] --help\" for more information about a command.",
                self.command_path(c)
            ));
        }
        w.push('\n');
        w
    }

    /// cobra's `defaultHelpFunc`.
    pub fn help_string(&self, c: CmdId) -> String {
        let cmd = &self.cmds[c];
        let usage = if cmd.long.is_empty() {
            &cmd.short
        } else {
            &cmd.long
        };
        let usage = trim_right_space(usage);
        let mut w = String::new();
        if !usage.is_empty() {
            w.push_str(usage);
            w.push_str("\n\n");
        }
        if self.runnable(c) || self.has_subcommands(c) {
            w.push_str(&self.usage_string(c));
        }
        w
    }

    /// Adds the `help` command (cobra's `InitDefaultHelpCmd`).
    pub fn init_default_help_cmd(&mut self) {
        let root = self.root;
        if !self.has_subcommands(root) {
            return;
        }
        let name = self.name(root).to_string();
        let mut help = Cmd::new("help [command]", "Help about any command");
        help.long = format!(
            "Help provides help for any command in the application.\nSimply type {name} help [path to command] for full details."
        );
        help.action = Some(Action::Help);
        help.valid_args_fn = ValidArgsFn::HelpTopics;
        let id = self.add(root, help);
        self.cmds[root].help_cmd = Some(id);
    }

    /// Adds `__complete` only when the args invoke it (`initCompleteCmd`).
    pub fn init_complete_cmd(&mut self, args: &[String]) {
        let root = self.root;
        let mut cmd = Cmd::new(
            "__complete [command-line]",
            "Request shell completion choices for the specified command-line",
        );
        cmd.aliases = vec!["__completeNoDesc".to_string()];
        cmd.disable_flags_in_use_line = true;
        cmd.hidden = true;
        cmd.disable_flag_parsing = true;
        cmd.args = ArgsRule::Min(1);
        cmd.long = "__complete is a special command that is used by the shell completion logic\nto request completion choices for the specified command-line.".to_string();
        cmd.action = Some(Action::Complete);
        let id = self.add(root, cmd);
        let (found, _, err) = self.find(args);
        if err.is_some() || self.name(found) != "__complete" {
            self.remove_command(root, id);
        }
    }

    /// Adds the `completion` command tree (`InitDefaultCompletionCmd`).
    pub fn init_default_completion_cmd(&mut self) {
        let root = self.root;
        let children = self.cmds[root].children.clone();
        if children.iter().any(|c| self.name(*c) == "completion") {
            return;
        }
        let name = self.name(root).to_string();
        let mut completion = Cmd::new(
            "completion",
            "Generate the autocompletion script for the specified shell",
        );
        completion.long = format!(
            "Generate the autocompletion script for {name} for the specified shell.\nSee each sub-command's help for details on how to use the generated script.\n"
        );
        completion.args = ArgsRule::NoArgs;
        completion.valid_args_fn = ValidArgsFn::NoFile;
        let comp = self.add(root, completion);

        let short = |shell: &str| format!("Generate the autocompletion script for {shell}");
        let shells = [
            (
                "bash",
                Action::CompletionBash,
                format!(
                    "Generate the autocompletion script for the bash shell.\n\nThis script depends on the 'bash-completion' package.\nIf it is not installed already, you can install it via your OS's package manager.\n\nTo load completions in your current shell session:\n\n\tsource <({name} completion bash)\n\nTo load completions for every new session, execute once:\n\n#### Linux:\n\n\t{name} completion bash > /etc/bash_completion.d/{name}\n\n#### macOS:\n\n\t{name} completion bash > $(brew --prefix)/etc/bash_completion.d/{name}\n\nYou will need to start a new shell for this setup to take effect.\n"
                ),
                true,
            ),
            (
                "zsh",
                Action::CompletionZsh,
                format!(
                    "Generate the autocompletion script for the zsh shell.\n\nIf shell completion is not already enabled in your environment you will need\nto enable it.  You can execute the following once:\n\n\techo \"autoload -U compinit; compinit\" >> ~/.zshrc\n\nTo load completions in your current shell session:\n\n\tsource <({name} completion zsh)\n\nTo load completions for every new session, execute once:\n\n#### Linux:\n\n\t{name} completion zsh > \"${{fpath[1]}}/_{name}\"\n\n#### macOS:\n\n\t{name} completion zsh > $(brew --prefix)/share/zsh/site-functions/_{name}\n\nYou will need to start a new shell for this setup to take effect.\n"
                ),
                false,
            ),
            (
                "fish",
                Action::CompletionFish,
                format!(
                    "Generate the autocompletion script for the fish shell.\n\nTo load completions in your current shell session:\n\n\t{name} completion fish | source\n\nTo load completions for every new session, execute once:\n\n\t{name} completion fish > ~/.config/fish/completions/{name}.fish\n\nYou will need to start a new shell for this setup to take effect.\n"
                ),
                false,
            ),
            (
                "powershell",
                Action::CompletionPowershell,
                format!(
                    "Generate the autocompletion script for powershell.\n\nTo load completions in your current shell session:\n\n\t{name} completion powershell | Out-String | Invoke-Expression\n\nTo load completions for every new session, add the output of the above command\nto your powershell profile.\n"
                ),
                false,
            ),
        ];
        for (shell, action, long, no_flags_in_use) in shells {
            let mut cmd = Cmd::new(shell, &short(shell));
            cmd.long = long;
            cmd.args = ArgsRule::NoArgs;
            cmd.disable_flags_in_use_line = no_flags_in_use;
            cmd.valid_args_fn = ValidArgsFn::NoFile;
            cmd.action = Some(action);
            let id = self.add(comp, cmd);
            self.flag(
                id,
                false,
                (
                    "no-descriptions",
                    None,
                    FlagKind::Bool,
                    "disable completion descriptions",
                ),
            );
        }
    }

    /// cobra's `ExecuteC` minus the final error print (the root silences
    /// errors and usage; the CLI prints its own).
    pub fn execute_c(
        &mut self,
        args: &[String],
        io: &mut Io,
        run: &mut dyn FnMut(&mut Cobra, CmdId, &mut Io) -> Result<(), CliError>,
    ) -> Result<(), CliError> {
        self.init_default_help_cmd();
        self.init_complete_cmd(args);
        self.init_default_completion_cmd();
        let (cmd, flags, err) = self.find(args);
        if let Some(err) = err {
            return Err(err);
        }
        if self.cmds[cmd].called_as.is_empty() {
            self.cmds[cmd].called_as = self.name(cmd).to_string();
        }
        match self.execute(cmd, &flags, io, run) {
            Err(CliError::Help) => {
                let help = self.help_string(cmd);
                let _ = io.out.write_all(help.as_bytes());
                Ok(())
            }
            other => other,
        }
    }

    fn execute(
        &mut self,
        c: CmdId,
        args: &[String],
        io: &mut Io,
        run: &mut dyn FnMut(&mut Cobra, CmdId, &mut Io) -> Result<(), CliError>,
    ) -> Result<(), CliError> {
        self.init_default_help_flag(c);
        self.init_default_version_flag(c);
        self.parse_flags(c, args)?;
        if let Some(h) = self.flag_named(c, "help")
            && self.flag_bool(h)
        {
            return Err(CliError::Help);
        }
        if !self.cmds[c].version.is_empty()
            && let Some(v) = self.flag_named(c, "version")
            && self.flag_bool(v)
        {
            let line = format!("{} version {}\n", self.name(c), self.cmds[c].version);
            let _ = io.out.write_all(line.as_bytes());
            return Ok(());
        }
        if !self.runnable(c) {
            return Err(CliError::Help);
        }
        if self.cmds[c].disable_flag_parsing {
            self.cmds[c].parsed_args = args.to_vec();
        }
        let positional = self.cmds[c].parsed_args.clone();
        self.validate_args(c, &positional)?;
        run(self, c, io)
    }

    /// Whether `c` or a parent declares the persistent pre-run hook.
    pub fn has_persistent_pre_run(&self, c: CmdId) -> bool {
        let mut p = Some(c);
        while let Some(id) = p {
            if self.cmds[id].persistent_pre_run {
                return true;
            }
            p = self.cmds[id].parent;
        }
        false
    }

    /// The `help [command]` action.
    pub fn run_help(&mut self, c: CmdId, io: &mut Io) -> Result<(), CliError> {
        let args = self.cmds[c].parsed_args.clone();
        let (target, _, err) = self.find(&args);
        if err.is_some() {
            let topics: Vec<String> = args.iter().map(|a| go_quote_backquote(a)).collect();
            let msg = format!("Unknown help topic [{}]\n", topics.join(" "));
            let _ = io.err.write_all(msg.as_bytes());
            let usage = self.usage_string(self.root);
            let _ = io.err.write_all(usage.as_bytes());
            return Ok(());
        }
        self.init_default_help_flag(target);
        self.init_default_version_flag(target);
        let help = self.help_string(target);
        let _ = io.out.write_all(help.as_bytes());
        Ok(())
    }

    /// The `__complete` action: completion choices, then `:<directive>`.
    pub fn run_complete(&mut self, c: CmdId, io: &mut Io) -> Result<(), CliError> {
        let args = self.cmds[c].parsed_args.clone();
        let (_final, completions, directive, err) = self.get_completions(&args);
        if let Some(err) = err {
            comp_debug(&format!("[Error] {err}\n"), io);
        }
        let mut no_desc = self.called_as(c) == "__completeNoDesc";
        if !no_desc && let Some(v) = parse_go_bool(&self.env_config("COMPLETION_DESCRIPTIONS")) {
            no_desc = !v;
        }
        let no_active_help = self.active_help_config() == "0";
        let mut out = String::new();
        for comp in completions {
            if no_active_help && comp.starts_with("_activeHelp_ ") {
                continue;
            }
            let mut comp = comp.as_str();
            if no_desc {
                comp = comp.split('\t').next().unwrap_or(comp);
            }
            comp = comp.split('\n').next().unwrap_or(comp);
            out.push_str(comp.trim());
            out.push('\n');
        }
        out.push_str(&format!(":{directive}\n"));
        let _ = io.out.write_all(out.as_bytes());
        let _ = io.err.write_all(
            format!(
                "Completion ended with directive: {}\n",
                directive_string(directive)
            )
            .as_bytes(),
        );
        Ok(())
    }

    fn env_config(&self, suffix: &str) -> String {
        let v = std::env::var(config_env_var(self.name(self.root), suffix)).unwrap_or_default();
        if !v.is_empty() {
            return v;
        }
        std::env::var(config_env_var("COBRA", suffix)).unwrap_or_default()
    }

    fn active_help_config(&self) -> String {
        let global = std::env::var("COBRA_ACTIVE_HELP").unwrap_or_default();
        if global == "0" {
            return global;
        }
        std::env::var(config_env_var(self.name(self.root), "ACTIVE_HELP")).unwrap_or_default()
    }

    fn get_completions(&mut self, args: &[String]) -> (CmdId, Vec<String>, u32, Option<String>) {
        let (to_complete, trimmed) = match args.split_last() {
            Some((last, rest)) => (last.clone(), rest.to_vec()),
            None => (String::new(), Vec::new()),
        };
        let (final_cmd, final_args, err) = self.find(&trimmed);
        if err.is_some() {
            return (
                final_cmd,
                Vec::new(),
                DIRECTIVE_DEFAULT,
                Some(format!(
                    "unable to find a command for arguments: [{}]",
                    trimmed.join(" ")
                )),
            );
        }
        if !self.cmds[final_cmd].disable_flag_parsing {
            self.init_default_help_flag(final_cmd);
            self.init_default_version_flag(final_cmd);
        }
        let (flag, mut final_args, to_complete, flag_err) =
            self.check_if_flag_completion(final_cmd, final_args, to_complete);

        let mut with_dash = final_args.clone();
        with_dash.push("--".to_string());
        let _ = self.parse_flags(final_cmd, &with_dash);
        let new_arg_count = self.cmds[final_cmd].parsed_args.len();
        if let Err(e) = self.parse_flags(final_cmd, &final_args) {
            let msg = match e {
                CliError::Msg(m) => m,
                CliError::Help => "pflag: help requested".to_string(),
                CliError::Exit(_, m) => m.unwrap_or_default(),
            };
            return (
                final_cmd,
                Vec::new(),
                DIRECTIVE_DEFAULT,
                Some(format!(
                    "Error while parsing flags from args [{}]: {msg}",
                    final_args.join(" ")
                )),
            );
        }
        let real_arg_count = self.cmds[final_cmd].parsed_args.len();
        let flag_completion = new_arg_count <= real_arg_count;
        if let Some((is_comp_error, msg)) = flag_err
            && (!is_comp_error || flag_completion)
        {
            return (final_cmd, Vec::new(), DIRECTIVE_DEFAULT, Some(msg));
        }
        let set = self.flag_set(final_cmd);
        let present = |name: &str| {
            self.lookup(&set, name)
                .is_some_and(|f| self.flags[f].by_cobra && self.flags[f].changed)
        };
        if present("version") || present("help") {
            return (final_cmd, Vec::new(), DIRECTIVE_NO_FILE_COMP, None);
        }
        if !self.cmds[final_cmd].disable_flag_parsing {
            final_args = self.cmds[final_cmd].parsed_args.clone();
        }

        let mut completions: Vec<String> = Vec::new();
        let mut directive;
        if flag.is_none()
            && to_complete.starts_with('-')
            && !to_complete.contains('=')
            && flag_completion
        {
            let mut ids = self.inherited_flags(final_cmd);
            ids.sort_by(|a, b| self.flags[*a].name.cmp(&self.flags[*b].name));
            let mut local = self.local_flags(final_cmd);
            local.sort_by(|a, b| self.flags[*a].name.cmp(&self.flags[*b].name));
            ids.extend(local);
            for id in ids {
                let f = &self.flags[id];
                if f.changed {
                    continue;
                }
                let long = format!("--{}", f.name);
                if long.starts_with(&to_complete) {
                    completions.push(format!("{long}\t{}", f.usage));
                }
                if let Some(s) = f.shorthand {
                    let short = format!("-{}", s as char);
                    if short.starts_with(&to_complete) {
                        completions.push(format!("{short}\t{}", f.usage));
                    }
                }
            }
            directive = DIRECTIVE_NO_FILE_COMP;
            if completions.len() == 1 && completions[0].ends_with('=') {
                directive = DIRECTIVE_NO_SPACE;
            }
            if !self.cmds[final_cmd].disable_flag_parsing {
                return (final_cmd, completions, directive, None);
            }
        } else {
            directive = DIRECTIVE_DEFAULT;
            if flag.is_none() {
                let local_non_persistent: Vec<FlagId> = self
                    .local_flags(final_cmd)
                    .into_iter()
                    .filter(|f| !self.cmds[final_cmd].persistent.contains(f))
                    .collect();
                let found_local = local_non_persistent.iter().any(|f| self.flags[*f].changed);
                if final_args.is_empty() && !found_local {
                    let help_cmd = self.cmds[final_cmd].help_cmd;
                    for s in self.commands(final_cmd) {
                        if self.is_available(s) || Some(s) == help_cmd {
                            if self.name(s).starts_with(&to_complete) {
                                completions.push(format!(
                                    "{}\t{}",
                                    self.name(s),
                                    self.cmds[s].short
                                ));
                            }
                            directive = DIRECTIVE_NO_FILE_COMP;
                        }
                    }
                }
            }
        }
        if flag.is_none() || !flag_completion {
            match self.cmds[final_cmd].valid_args_fn {
                ValidArgsFn::None => {}
                ValidArgsFn::NoFile => directive = DIRECTIVE_NO_FILE_COMP,
                ValidArgsFn::HelpTopics => {
                    let (comps, d) = self.help_topic_completions(&final_args, &to_complete);
                    completions.extend(comps);
                    directive = d;
                }
            }
        }
        (final_cmd, completions, directive, None)
    }

    fn help_topic_completions(&mut self, args: &[String], to_complete: &str) -> (Vec<String>, u32) {
        let (cmd, _, err) = self.find(args);
        if err.is_some() {
            return (Vec::new(), DIRECTIVE_NO_FILE_COMP);
        }
        let help_cmd = self.cmds[cmd].help_cmd;
        let mut out = Vec::new();
        for s in self.commands(cmd) {
            if (self.is_available(s) || Some(s) == help_cmd)
                && self.name(s).starts_with(to_complete)
            {
                out.push(format!("{}\t{}", self.name(s), self.cmds[s].short));
            }
        }
        (out, DIRECTIVE_NO_FILE_COMP)
    }

    /// Returns the flag being completed, trimmed args, the value prefix, and
    /// an error `(is_flag_comp_error, message)`.
    #[allow(clippy::type_complexity)]
    fn check_if_flag_completion(
        &self,
        c: CmdId,
        args: Vec<String>,
        last_arg: String,
    ) -> (Option<FlagId>, Vec<String>, String, Option<(bool, String)>) {
        if self.cmds[c].disable_flag_parsing {
            return (None, args, last_arg, None);
        }
        let mut flag_name = String::new();
        let mut trimmed = args.clone();
        let mut with_equal = false;
        let original = last_arg.clone();
        let mut last = last_arg;
        if last.starts_with('-') {
            match last.find('=') {
                Some(index) => {
                    flag_name = if last[..index].starts_with("--") {
                        last[2..index].to_string()
                    } else {
                        last[index - 1..index].to_string()
                    };
                    last = last[index + 1..].to_string();
                    with_equal = true;
                }
                None => return (None, args, last, None),
            }
        }
        if flag_name.is_empty()
            && let Some(prev) = args.last()
            && is_flag_arg(prev)
            && !prev.contains('=')
        {
            flag_name = if let Some(long) = prev.strip_prefix("--") {
                long.to_string()
            } else {
                prev[prev.len() - 1..].to_string()
            };
            trimmed = args[..args.len() - 1].to_vec();
        }
        if flag_name.is_empty() {
            return (None, trimmed, last, None);
        }
        let Some(flag) = self.find_flag(c, &flag_name) else {
            return (
                None,
                args,
                original,
                Some((
                    true,
                    format!(
                        "Subcommand '{}' does not support flag '{flag_name}'",
                        self.name(c)
                    ),
                )),
            );
        };
        if !with_equal && !self.flags[flag].no_opt_def_val().is_empty() {
            return (None, args, last, None);
        }
        (Some(flag), trimmed, last, None)
    }

    fn find_flag(&self, c: CmdId, name: &str) -> Option<FlagId> {
        let set = self.flag_set(c);
        let mut name = name.to_string();
        if name.len() == 1 {
            let b = name.as_bytes()[0];
            match self
                .lookup_short(&set, b)
                .or_else(|| self.lookup_short(&self.inherited_flags(c), b))
            {
                Some(f) => name = self.flags[f].name.clone(),
                None => return None,
            }
        }
        self.lookup(&set, &name)
    }
}

impl Flag {
    fn value_is_nonzero_default(&self) -> bool {
        match self.kind {
            FlagKind::Bool => !(self.def_value == "false" || self.def_value.is_empty()),
            FlagKind::Str => !self.def_value.is_empty(),
        }
    }
}

pub const DIRECTIVE_DEFAULT: u32 = 0;
pub const DIRECTIVE_NO_SPACE: u32 = 2;
pub const DIRECTIVE_NO_FILE_COMP: u32 = 4;

fn directive_string(d: u32) -> String {
    let names = [
        (1, "ShellCompDirectiveError"),
        (2, "ShellCompDirectiveNoSpace"),
        (4, "ShellCompDirectiveNoFileComp"),
        (8, "ShellCompDirectiveFilterFileExt"),
        (16, "ShellCompDirectiveFilterDirs"),
        (32, "ShellCompDirectiveKeepOrder"),
    ];
    let mut out: Vec<&str> = names
        .iter()
        .filter(|(bit, _)| d & bit != 0)
        .map(|(_, n)| *n)
        .collect();
    if out.is_empty() {
        out.push("ShellCompDirectiveDefault");
    }
    if d >= 64 {
        return format!("ERROR: unexpected ShellCompDirective value: {d}");
    }
    out.join(", ")
}

/// cobra's `CompDebug(msg, true)`: append to `BASH_COMP_DEBUG_FILE` when
/// set, and print to stderr.
fn comp_debug(msg: &str, io: &mut Io) {
    let msg = format!("[Debug] {msg}");
    if let Ok(path) = std::env::var("BASH_COMP_DEBUG_FILE")
        && !path.is_empty()
        && let Ok(mut f) = std::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(&path)
    {
        let _ = f.write_all(msg.as_bytes());
    }
    let _ = io.err.write_all(msg.as_bytes());
}

fn config_env_var(name: &str, suffix: &str) -> String {
    format!("{name}_{suffix}")
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

fn is_flag_arg(arg: &str) -> bool {
    let b = arg.as_bytes();
    (b.len() >= 3 && &b[..2] == b"--") || (b.len() >= 2 && b[0] == b'-' && b[1] != b'-')
}

/// pflag `UnquoteUsage`: a back-quoted name in the usage, else the type.
fn unquote_usage(f: &Flag) -> (String, String) {
    let usage = &f.usage;
    if let Some(i) = usage.find('`')
        && let Some(j) = usage[i + 1..].find('`')
    {
        let name = &usage[i + 1..i + 1 + j];
        let unquoted = format!("{}{}{}", &usage[..i], name, &usage[i + 2 + j..]);
        return (name.to_string(), unquoted);
    }
    let name = match f.type_name() {
        "bool" => "",
        other => other,
    };
    (name.to_string(), usage.clone())
}

/// Go `strconv.ParseBool`.
pub fn parse_go_bool(s: &str) -> Option<bool> {
    match s {
        "1" | "t" | "T" | "TRUE" | "true" | "True" => Some(true),
        "0" | "f" | "F" | "FALSE" | "false" | "False" => Some(false),
        _ => None,
    }
}

fn rpad(s: &str, n: usize) -> String {
    format!("{s:<n$}")
}

/// Go `strings.TrimRightFunc(s, unicode.IsSpace)`.
fn trim_right_space(s: &str) -> &str {
    s.trim_end()
}

pub fn go_quote(s: &str) -> String {
    recur_bundles::go::quote(s)
}

/// Go `%q` of a rune.
fn go_quote_rune(c: char) -> String {
    match c {
        '\'' => "'\\''".to_string(),
        '"' => "'\"'".to_string(),
        c => {
            let q = recur_bundles::go::quote(&c.to_string());
            format!("'{}'", &q[1..q.len() - 1])
        }
    }
}

/// Go `%#q`: back-quoted when `strconv.CanBackquote`, else `%q`.
fn go_quote_backquote(s: &str) -> String {
    let can = s
        .chars()
        .all(|c| c != '`' && c != '\u{feff}' && c != '\u{7f}' && (c >= ' ' || c == '\t'));
    if can { format!("`{s}`") } else { go_quote(s) }
}

/// Damerau-free Levenshtein distance with case folding (cobra's `ld`).
fn levenshtein_fold(s: &str, t: &str) -> usize {
    let s: Vec<char> = s.to_lowercase().chars().collect();
    let t: Vec<char> = t.to_lowercase().chars().collect();
    let mut d: Vec<Vec<usize>> = (0..=s.len())
        .map(|i| {
            let mut row = vec![0; t.len() + 1];
            row[0] = i;
            row
        })
        .collect();
    for (j, cell) in d[0].iter_mut().enumerate() {
        *cell = j;
    }
    for j in 1..=t.len() {
        for i in 1..=s.len() {
            d[i][j] = if s[i - 1] == t[j - 1] {
                d[i - 1][j - 1]
            } else {
                (d[i - 1][j] + 1)
                    .min(d[i][j - 1] + 1)
                    .min(d[i - 1][j - 1] + 1)
            };
        }
    }
    d[s.len()][t.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levenshtein() {
        assert_eq!(levenshtein_fold("bundlez", "bundles"), 1);
        assert_eq!(levenshtein_fold("OPEN", "open"), 0);
        assert_eq!(levenshtein_fold("", "abc"), 3);
    }

    #[test]
    fn quoting() {
        assert_eq!(go_quote_rune('x'), "'x'");
        assert_eq!(go_quote_rune('\''), "'\\''");
        assert_eq!(go_quote_backquote("a b"), "`a b`");
        assert_eq!(go_quote_backquote("a`b"), "\"a`b\"");
    }

    #[test]
    fn directives() {
        assert_eq!(directive_string(0), "ShellCompDirectiveDefault");
        assert_eq!(
            directive_string(6),
            "ShellCompDirectiveNoSpace, ShellCompDirectiveNoFileComp"
        );
    }
}
