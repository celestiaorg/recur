//! `recur bundles lint`: schema, TypeScript compile, shebang `#!`,
//! keyEquivalent parse / reserved-app collisions, cross-bundle sharing.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use crate::go::{self, quote};
use crate::grammar::{grammar_convention_ok, terminal_theme_convention_ok, theme_convention_ok};
use crate::keyequiv::{RESERVED_APP_CHORDS, is_reserved_app_chord, parse_key_equivalent};
use crate::loops::{ROLE_MODE_LOOP, Role, VERB_VERIFY, allowed_role_modes, same_or_stricter};
use crate::manifest::{
    BundleMeta, Command, INTERPRETER_SHEBANG, INTERPRETER_TYPESCRIPT, OUTPUT_NEW_DOCUMENT,
    PROMPT_MODE_LOOP, PROMPT_SLASH_PATTERN, PROMPT_SLASH_RE, PROMPT_TIMEOUT_MAX_MS, PromptAction,
    parse_manifest_full, valid_bundle_id, valid_prompt_mode,
};
use crate::snippet::Snippet;
use crate::{Error, err};

/// `error` fails lint; `info` is advisory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LintSeverity {
    Error,
    Info,
}

impl LintSeverity {
    pub fn as_str(self) -> &'static str {
        match self {
            LintSeverity::Error => "error",
            LintSeverity::Info => "info",
        }
    }
}

/// One diagnostic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LintFinding {
    pub severity: LintSeverity,
    pub bundle: String,
    /// Command/prompt name or id; empty for bundle-level findings.
    pub command: String,
    /// Bundle dir or file.
    pub path: String,
    pub message: String,
}

/// Findings across one or more bundles.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LintReport {
    pub findings: Vec<LintFinding>,
    pub errors: usize,
    pub infos: usize,
}

impl LintReport {
    /// True when there are no error-severity findings.
    pub fn ok(&self) -> bool {
        self.errors == 0
    }

    fn add(&mut self, f: LintFinding) {
        match f.severity {
            LintSeverity::Error => self.errors += 1,
            LintSeverity::Info => self.infos += 1,
        }
        self.findings.push(f);
    }
}

struct KeyClaim {
    canonical: String,
    bundle: String,
    name: String,
    dir: String,
}

struct SlashClaim {
    slash: String,
    bundle: String,
    name: String,
    dir: String,
}

/// Lints each bundle directory and cross-checks shared keyEquivalents and
/// prompt slashes. `cache_dir` receives compiled TypeScript.
pub fn lint_bundles(dirs: &[String], cache_dir: &str) -> LintReport {
    let mut rep = LintReport::default();
    let mut claims: Vec<KeyClaim> = Vec::new();
    let mut slash_claims: Vec<SlashClaim> = Vec::new();

    for dir in dirs {
        let (meta, cmds, prompts, snippets, findings) = match lint_one_bundle(dir, cache_dir) {
            Ok(v) => v,
            Err(e) => {
                rep.add(LintFinding {
                    severity: LintSeverity::Error,
                    bundle: go::base(dir),
                    command: String::new(),
                    path: dir.clone(),
                    message: e.0,
                });
                continue;
            }
        };
        for f in findings {
            rep.add(f);
        }
        let mut add_key_claim = |raw: &str, name: &str| {
            let raw = raw.trim();
            if raw.is_empty() {
                return;
            }
            let Ok(canonical) = parse_key_equivalent(raw) else {
                return;
            };
            if is_reserved_app_chord(&canonical) {
                return;
            }
            claims.push(KeyClaim {
                canonical,
                bundle: meta.name.clone(),
                name: name.to_string(),
                dir: dir.clone(),
            });
        };
        for c in &cmds {
            add_key_claim(&c.key_equivalent, &c.name);
        }
        for s in &snippets {
            add_key_claim(&s.key_equivalent, &s.name);
        }
        for p in &prompts {
            slash_claims.push(SlashClaim {
                slash: p.slash.clone(),
                bundle: meta.name.clone(),
                name: p.name.clone(),
                dir: dir.clone(),
            });
        }
    }

    let mut by_chord: BTreeMap<&str, Vec<&KeyClaim>> = BTreeMap::new();
    for c in &claims {
        by_chord.entry(&c.canonical).or_default().push(c);
    }
    for (chord, list) in by_chord {
        if list.len() < 2 {
            continue;
        }
        let mut names: Vec<String> = list
            .iter()
            .map(|c| format!("{}/{}", c.bundle, c.name))
            .collect();
        names.sort();
        rep.add(LintFinding {
            severity: LintSeverity::Info,
            bundle: String::new(),
            command: String::new(),
            path: list[0].dir.clone(),
            message: format!(
                "keyEquivalent {} shared by {} (disambiguation menu at runtime)",
                quote(chord),
                names.join(", ")
            ),
        });
    }

    let mut by_slash: BTreeMap<&str, Vec<&SlashClaim>> = BTreeMap::new();
    for c in &slash_claims {
        by_slash.entry(&c.slash).or_default().push(c);
    }
    for (slash, list) in by_slash {
        if list.len() < 2 {
            continue;
        }
        // Same-bundle dupes are already reported per bundle.
        let dirs: BTreeSet<&str> = list.iter().map(|c| c.dir.as_str()).collect();
        if dirs.len() < 2 {
            continue;
        }
        let mut names: Vec<String> = list
            .iter()
            .map(|c| format!("{}/{}", c.bundle, c.name))
            .collect();
        names.sort();
        rep.add(LintFinding {
            severity: LintSeverity::Error,
            bundle: list[0].bundle.clone(),
            command: list[0].name.clone(),
            path: list[0].dir.clone(),
            message: format!("slash {} duplicated by {}", quote(slash), names.join(", ")),
        });
    }
    rep
}

type LintedBundle = (
    BundleMeta,
    Vec<Command>,
    Vec<PromptAction>,
    Vec<Snippet>,
    Vec<LintFinding>,
);

fn lint_one_bundle(dir: &str, cache_dir: &str) -> Result<LintedBundle, Error> {
    let data = go::read_file(&go::join(&[dir, "bundle.toml"]))
        .map_err(|e| err!("read bundle.toml: {e}"))?;
    let parsed = parse_manifest_full(dir, &data)?;
    let meta = parsed.meta.clone();
    let mut findings: Vec<LintFinding> = Vec::new();
    let mut add = |severity: LintSeverity, command: &str, path: &str, message: String| {
        findings.push(LintFinding {
            severity,
            bundle: meta.name.clone(),
            command: command.to_string(),
            path: path.to_string(),
            message,
        });
    };
    use LintSeverity::{Error as E, Info as I};

    for g in &parsed.grammars {
        if !grammar_convention_ok(&g.path) {
            add(
                I,
                &g.scope_name,
                &g.abs_path,
                format!(
                    "grammar path {} should end in .tmLanguage.json or .tmLanguage",
                    quote(&g.path)
                ),
            );
        }
        match go::stat(&g.abs_path) {
            Err(e) => add(E, &g.scope_name, dir, format!("grammar file missing: {e}")),
            Ok(st) if st.is_dir() => add(
                E,
                &g.scope_name,
                &g.abs_path,
                "grammar path is a directory".to_string(),
            ),
            Ok(_) => {
                if let Ok(body) = std::fs::read(&g.abs_path)
                    && !body.is_empty()
                    && go::to_lower(&g.path).ends_with(".json")
                {
                    // Lightweight JSON check (plist is XML, skipped).
                    let text = String::from_utf8_lossy(&body);
                    let trim = text.trim();
                    if !trim.starts_with('{') && !trim.starts_with('[') {
                        add(
                            E,
                            &g.scope_name,
                            &g.abs_path,
                            "grammar JSON does not look like an object/array".to_string(),
                        );
                    }
                }
            }
        }
    }
    for t in &parsed.themes {
        if !theme_convention_ok(&t.path) {
            add(
                I,
                &t.name,
                &t.abs_path,
                format!(
                    "theme path {} should end in .tmTheme.json (or .tmTheme)",
                    quote(&t.path)
                ),
            );
        }
        if let Ok(body) = std::fs::read(&t.abs_path) {
            let text = String::from_utf8_lossy(&body);
            if go::to_lower(&t.path).ends_with(".json") && !text.trim().starts_with('{') {
                add(
                    E,
                    &t.name,
                    &t.abs_path,
                    "theme JSON does not look like an object".to_string(),
                );
            }
        }
    }
    for t in &parsed.terminal_themes {
        if !terminal_theme_convention_ok(&t.path) {
            add(
                I,
                &t.name,
                &t.abs_path,
                format!("terminalTheme path {} should end in .json", quote(&t.path)),
            );
        }
    }
    for d in &parsed.language_data {
        if !d.has_payload() {
            add(
                E,
                &d.id,
                dir,
                "languageData has no comment tokens, indent patterns, or closeBrackets".to_string(),
            );
        }
        if d.block_comment_start.is_empty() != d.block_comment_end.is_empty() {
            add(
                E,
                &d.id,
                dir,
                "blockCommentStart and blockCommentEnd must both be set".to_string(),
            );
        }
    }
    for s in &parsed.snippets {
        if let Some((sev, msg)) = key_equivalent_finding(&s.key_equivalent) {
            add(sev, &s.name, dir, msg);
        }
    }
    for sy in &parsed.symbols {
        if symbol_transform_has_oniguruma(&sy.transform) {
            add(
                I,
                &sy.id,
                dir,
                "symbol transform uses Oniguruma syntax; Recur applies JavaScript regex s/// only"
                    .to_string(),
            );
        }
    }
    let role_by_id: HashMap<&str, &Role> =
        parsed.roles.iter().map(|r| (r.id.as_str(), r)).collect();
    for lp in &parsed.loops {
        let cond_id = lp
            .roles
            .get("conductor")
            .map(String::as_str)
            .unwrap_or_default();
        if let Some(r) = role_by_id.get(cond_id)
            && r.mode != ROLE_MODE_LOOP
        {
            add(
                E,
                &lp.id,
                dir,
                format!("conductor {} must have mode = \"loop\"", quote(cond_id)),
            );
        }
        for ph in &lp.phases {
            let at = format!("{}/{}", lp.id, ph.id);
            if ph.start == "review"
                && ph
                    .prompts
                    .get("implement")
                    .is_some_and(|p| !p.trim().is_empty())
            {
                add(
                    I,
                    &at,
                    dir,
                    "start = \"review\" ignores prompts.implement".to_string(),
                );
            }
            if ph.uses == VERB_VERIFY {
                let vid = ph
                    .roles
                    .get("verifier")
                    .map(String::as_str)
                    .unwrap_or_default();
                if let Some(r) = role_by_id.get(vid)
                    && !r.tools.iter().any(|t| t == "report_result")
                {
                    add(
                        E,
                        &at,
                        dir,
                        "verifier role must list report_result".to_string(),
                    );
                }
            }
            for (slot, rid) in &ph.roles {
                let Some(r) = role_by_id.get(rid.as_str()) else {
                    continue;
                };
                let allowed = allowed_role_modes(&ph.uses, slot);
                if !allowed.is_empty() && !allowed.contains(&r.mode.as_str()) {
                    add(
                        E,
                        &at,
                        dir,
                        format!(
                            "roles.{slot} {} has mode {} (want {})",
                            quote(rid),
                            quote(&r.mode),
                            allowed.join("|")
                        ),
                    );
                }
            }
            if let Some(th) = &ph.findings {
                for pol in &parsed.policies {
                    if let Err(e) = same_or_stricter(th, &pol.findings_threshold) {
                        add(
                            E,
                            &at,
                            dir,
                            format!("findings_threshold vs policy {}: {e}", pol.id),
                        );
                    }
                }
            }
        }
    }
    for c in &parsed.commands {
        if let Some((sev, msg)) = key_equivalent_finding(&c.key_equivalent) {
            add(sev, &c.name, dir, msg);
        }
        if c.open_in_preview {
            if c.output != OUTPUT_NEW_DOCUMENT {
                add(
                    E,
                    &c.name,
                    dir,
                    "openInPreview requires output = \"newDocument\"".to_string(),
                );
            }
            if !previewable_language(&c.language) {
                let lang = c.language.trim();
                let msg = if lang.is_empty() {
                    "openInPreview requires language (html, htm, md, mdx, markdown, svg, mmd, or mermaid)".to_string()
                } else {
                    format!(
                        "openInPreview: language {} has no Preview chip (use html, htm, md, mdx, markdown, svg, mmd, or mermaid)",
                        quote(lang)
                    )
                };
                add(E, &c.name, dir, msg);
            }
        }
        match c.interpreter.as_str() {
            INTERPRETER_TYPESCRIPT => {
                if let Err(e) = crate::compile_macro(cache_dir, c) {
                    add(
                        E,
                        &c.name,
                        &script_path_or_dir(&c.script_path, &c.script, dir),
                        e.0,
                    );
                }
            }
            INTERPRETER_SHEBANG => {
                if let Err(msg) = lint_shebang(&c.body, &c.script) {
                    add(
                        E,
                        &c.name,
                        &script_path_or_dir(&c.script_path, &c.script, dir),
                        msg,
                    );
                }
            }
            _ => {}
        }
    }

    let mut seen_slash: HashMap<String, String> = HashMap::new();
    for p in &parsed.prompts {
        if !PROMPT_SLASH_RE.is_match(&p.slash) {
            add(
                E,
                &p.name,
                dir,
                format!(
                    "slash {} must match {PROMPT_SLASH_PATTERN}",
                    quote(&p.slash)
                ),
            );
        }
        if !p.loop_id.is_empty() {
            if !valid_bundle_id(&p.loop_id) {
                add(
                    E,
                    &p.name,
                    dir,
                    format!("loop {} is not a safe id", quote(&p.loop_id)),
                );
            }
            if !p.mode.is_empty() && p.mode != PROMPT_MODE_LOOP {
                add(E, &p.name, dir, "loop requires mode \"loop\"".to_string());
            }
        }
        if !p.mode.is_empty() && !valid_prompt_mode(&p.mode) {
            add(
                E,
                &p.name,
                dir,
                format!(
                    "unknown mode {} (use ask|plan|agent|review|loop, or omit)",
                    quote(&p.mode)
                ),
            );
        }
        if p.fresh_session && p.mode == PROMPT_MODE_LOOP {
            add(
                E,
                &p.name,
                dir,
                "fresh_session is not valid with mode \"loop\"".to_string(),
            );
        }
        if !p.tab_group.is_empty() && p.mode.is_empty() {
            add(E, &p.name, dir, "tab_group requires mode".to_string());
        }
        if !p.tab_group.is_empty() && !PROMPT_SLASH_RE.is_match(&p.tab_group) {
            add(
                E,
                &p.name,
                dir,
                format!(
                    "tab_group {} must match {PROMPT_SLASH_PATTERN}",
                    quote(&p.tab_group)
                ),
            );
        }
        if p.timeout_ms < 0 || p.timeout_ms > PROMPT_TIMEOUT_MAX_MS {
            add(
                E,
                &p.name,
                dir,
                format!("timeoutMs must be 0 or 1..{PROMPT_TIMEOUT_MAX_MS}"),
            );
        }
        match seen_slash.get(&p.slash) {
            Some(prev) => add(
                E,
                &p.name,
                dir,
                format!(
                    "slash {} duplicated with prompt {}",
                    quote(&p.slash),
                    quote(prev)
                ),
            ),
            None => {
                seen_slash.insert(p.slash.clone(), p.name.clone());
            }
        }
        match p.interpreter.as_str() {
            INTERPRETER_TYPESCRIPT => {
                if let Err(e) = crate::compile_prompt_macro(cache_dir, p) {
                    add(
                        E,
                        &p.name,
                        &script_path_or_dir(&p.script_path, &p.script, dir),
                        e.0,
                    );
                }
            }
            INTERPRETER_SHEBANG => {
                if let Err(msg) = lint_shebang(&p.body, &p.script) {
                    add(
                        E,
                        &p.name,
                        &script_path_or_dir(&p.script_path, &p.script, dir),
                        msg,
                    );
                }
            }
            _ => {}
        }
        if p.complete_interpreter == INTERPRETER_TYPESCRIPT {
            if let Err(e) = crate::compile_complete_macro(cache_dir, p) {
                add(E, &p.name, &p.complete_path, e.0);
            }
        } else if p.complete_interpreter == INTERPRETER_SHEBANG
            && !starts_with_shebang(&p.complete_body)
        {
            add(
                E,
                &p.name,
                &p.complete_path,
                "complete shebang script must start with #!".to_string(),
            );
        }
    }
    // simplify: no Settings → Bundles toggles for [[search]]/[[expand]] until a
    // pack ships several; disable with the parent prompt pack by removing it.
    for se in &parsed.searches {
        if !starts_with_shebang(&se.body) {
            add(
                E,
                &se.name,
                &se.script_path,
                "search shebang script must start with #!".to_string(),
            );
        }
    }
    for ex in &parsed.expands {
        if !starts_with_shebang(&ex.body) {
            add(
                E,
                &ex.name,
                &ex.script_path,
                "expand shebang script must start with #!".to_string(),
            );
        }
    }
    Ok((
        meta,
        parsed.commands,
        parsed.prompts,
        parsed.snippets,
        findings,
    ))
}

fn key_equivalent_finding(raw: &str) -> Option<(LintSeverity, String)> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    match parse_key_equivalent(raw) {
        Err(e) => Some((LintSeverity::Error, format!("keyEquivalent: {e}"))),
        Ok(can) => RESERVED_APP_CHORDS.get(can.as_str()).map(|label| {
            (
                LintSeverity::Error,
                format!(
                    "keyEquivalent {} collides with app shortcut {} (binding dropped in-app)",
                    quote(&can),
                    quote(label)
                ),
            )
        }),
    }
}

/// Whether a newDocument language gets an Observatory Code/Preview chip
/// (mirrors `frontend/src/lib/preview.ts`).
pub fn previewable_language(lang: &str) -> bool {
    let s = go::to_lower(lang.trim());
    matches!(
        s.strip_prefix('.').unwrap_or(&s),
        "html" | "htm" | "md" | "mdx" | "markdown" | "svg" | "mmd" | "mermaid"
    )
}

fn script_path_or_dir(script_path: &str, script: &str, dir: &str) -> String {
    if !script_path.is_empty() {
        return script_path.to_string();
    }
    if !script.is_empty() {
        return go::join(&[dir, script]);
    }
    go::join(&[dir, "bundle.toml"])
}

fn starts_with_shebang(body: &str) -> bool {
    body.trim_start_matches('\u{FEFF}').starts_with("#!")
}

fn lint_shebang(body: &str, script: &str) -> Result<(), String> {
    if starts_with_shebang(body) {
        return Ok(());
    }
    if !script.is_empty() {
        return Err(format!(
            "shebang script {} must start with #!",
            quote(script)
        ));
    }
    Err("shebang body must start with #!".to_string())
}

fn symbol_transform_has_oniguruma(src: &str) -> bool {
    ["(?x)", "(?i)", "(?m)", "\\h", "(?("]
        .iter()
        .any(|p| src.contains(p))
}

/// Prints findings for the CLI.
pub fn format_lint_report(r: &LintReport, color: bool) -> String {
    const RED: &str = "\x1b[31m";
    const GREEN: &str = "\x1b[32m";
    const YELLOW: &str = "\x1b[33m";
    const DIM: &str = "\x1b[2m";
    const BOLD: &str = "\x1b[1m";
    let paint = |code: &str, s: &str| {
        if color {
            format!("{code}{s}\x1b[0m")
        } else {
            s.to_string()
        }
    };
    let bold_red = format!("{BOLD}{RED}");
    let mut b = String::new();
    for f in &r.findings {
        let loc = if f.command.is_empty() {
            f.bundle.clone()
        } else {
            format!("{}/{}", f.bundle, f.command)
        };
        let tag = match f.severity {
            LintSeverity::Error => paint(&bold_red, "error"),
            LintSeverity::Info => paint(YELLOW, "info"),
        };
        b.push_str(&format!("{tag}\t{loc}\t{}\n", f.message));
        if !f.path.is_empty() {
            b.push_str(&format!("  {}\n", paint(DIM, &f.path)));
        }
    }
    if r.errors == 0 && r.infos == 0 {
        b.push_str(&format!("{}\n", paint(GREEN, "ok\tno issues")));
        return b;
    }
    let info_part = format!("{} info", r.infos);
    if r.errors == 0 {
        b.push_str(&format!("{}, {info_part}\n", paint(GREEN, "0 errors")));
    } else {
        b.push_str(&format!(
            "{}, {info_part}\n",
            paint(&bold_red, &format!("{} errors", r.errors))
        ));
    }
    b
}
