//! Engine behaviour against the shipped grammars and themes in `bundles/`
//! (ports `tm-engine.test.ts`, `tm-dotenv-theme.test.ts` and the bundle
//! theme checks in `tm-theme.test.ts`).

use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use recur_textmate::{
    GrammarSourceText, LineReplace, MAX_IDLE_DOCS, MAX_IDLE_LINES, RawTheme, RawThemeSetting,
    ThemeScope, TmEngine, ViewportMode, decode_foreground, parse_theme_json, tm_doc_fingerprint,
};

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn read(rel: &str) -> String {
    std::fs::read_to_string(repo().join(rel)).unwrap()
}

const LONG: Duration = Duration::from_secs(1);

fn ts_engine() -> TmEngine {
    let mut e = TmEngine::new();
    e.register_grammar(
        "source.ts",
        &read("bundles/Languages/syntaxes/typescript.tmLanguage.json"),
        "typescript.tmLanguage.json",
        &[],
    );
    e
}

fn synth_ts(lines: usize) -> String {
    let mut out = Vec::new();
    let mut i = 0;
    while out.len() < lines {
        out.push(format!("export function f{i}(x: number): number {{"));
        out.push(format!("  const y = x + {i}"));
        out.push("  if (y > 10) return y * 2".to_string());
        out.push("  return y".to_string());
        out.push("}".to_string());
        i += 1;
    }
    out.truncate(lines);
    out.join("\n")
}

fn flat_theme() -> RawTheme {
    RawTheme {
        name: Some("test".into()),
        settings: vec![RawThemeSetting {
            foreground: Some("#ccc".into()),
            background: Some("#111".into()),
            ..RawThemeSetting::default()
        }],
    }
}

fn replace(from_line: usize, old: usize, new: &[&str]) -> LineReplace {
    LineReplace {
        from_line,
        old_line_count: old,
        new_lines: new.iter().map(|s| s.to_string()).collect(),
    }
}

#[test]
fn retokenizes_one_line_on_a_same_stack_keystroke() {
    let mut e = ts_engine();
    e.set_theme(flat_theme());
    assert!(e.ensure_grammar("source.ts").unwrap());
    e.open_doc("d1", "source.ts", &synth_ts(200));
    assert_eq!(
        e.tokenize_viewport("d1", 1, 200, LONG).mode,
        ViewportMode::Accurate
    );
    assert!(e.has_fresh_tokens("d1", 1, 80));
    e.edit_doc("d1", &replace(37, 1, &["  const y = x + 99"]));
    // The stale stack for line 37 is kept for early-stop but its tokens are
    // not fresh.
    assert!(!e.has_fresh_tokens("d1", 1, 80));
    let after = e.tokenize_viewport("d1", 1, 80, LONG);
    assert_eq!(after.mode, ViewportMode::Accurate);
    assert!(after.tokenized <= 3);
    assert!(e.has_fresh_tokens("d1", 1, 80));
}

#[test]
fn applies_a_whole_document_replace() {
    let mut e = TmEngine::new();
    e.open_doc("big", "source.ts", "const a = 1");
    let lines = vec!["const x = 1"; 200_000];
    e.edit_doc("big", &replace(1, 1, &lines));
    assert_eq!(e.line_count("big"), 200_000);
}

#[test]
fn reuses_the_cache_when_reopening_the_same_text() {
    let mut e = ts_engine();
    assert!(e.ensure_grammar("source.ts").unwrap());
    let text = synth_ts(200);
    assert!(!e.open_doc("src/a.ts", "source.ts", &text));
    e.tokenize_viewport("src/a.ts", 1, 200, LONG);
    assert_eq!(e.filled_through("src/a.ts"), 200);
    assert!(e.open_doc("src/a.ts", "source.ts", &text));
    assert_eq!(e.filled_through("src/a.ts"), 200);
    assert!(e.has_fresh_tokens("src/a.ts", 1, 40));
    assert!(!e.open_doc("src/a.ts", "source.ts", &format!("{text}\n")));
    assert_eq!(e.filled_through("src/a.ts"), 0);
    assert!(!e.has_fresh_tokens("src/a.ts", 1, 40));
}

#[test]
fn match_doc_uses_the_fingerprint() {
    let mut e = TmEngine::new();
    let text = synth_ts(200);
    e.open_doc("src/a.ts", "source.ts", &text);
    assert!(e.match_doc("src/a.ts", "source.ts", TmEngine::fingerprint_of(&text)));
    assert!(!e.match_doc("src/a.ts", "source.other", TmEngine::fingerprint_of(&text)));
    assert!(!e.match_doc("missing", "source.ts", TmEngine::fingerprint_of(&text)));
    assert!(!e.match_doc(
        "src/a.ts",
        "source.ts",
        TmEngine::fingerprint_of(&format!("{text}\n"))
    ));
    let changed = text.replacen("export", "exqort", 1);
    assert!(!e.match_doc("src/a.ts", "source.ts", TmEngine::fingerprint_of(&changed)));
    e.edit_doc("src/a.ts", &replace(1, 1, &["// changed"]));
    assert!(!e.match_doc("src/a.ts", "source.ts", TmEngine::fingerprint_of(&text)));
}

#[test]
fn viewport_is_accurate_when_the_chain_reaches_it() {
    let mut e = ts_engine();
    assert!(e.ensure_grammar("source.ts").unwrap());
    e.open_doc("d", "source.ts", &synth_ts(300));
    let r = e.tokenize_viewport("d", 1, 60, LONG);
    assert_eq!(r.mode, ViewportMode::Accurate);
    assert!(r.complete);
    assert!(e.has_fresh_tokens("d", 1, 60));
    assert_eq!(e.provisional_count("d"), 0);
    assert!(e.filled_through("d") >= 60);
}

#[test]
fn a_cold_deep_jump_guesses_and_stays_provisional() {
    let mut e = ts_engine();
    assert!(e.ensure_grammar("source.ts").unwrap());
    e.open_doc("d", "source.ts", &synth_ts(2000));
    let r = e.tokenize_viewport("d", 1500, 1560, LONG);
    assert_eq!(r.mode, ViewportMode::Guessed);
    assert!(r.complete);
    assert_eq!(e.filled_through("d"), 0);
    assert!(e.provisional_count("d") > 0);
    assert!(e.has_fresh_tokens("d", 1500, 1560));
    assert!(e.tokens_for_line("d", 1530).is_some_and(|t| !t.is_empty()));
}

#[test]
fn fill_corrects_wrong_guesses_and_reports_the_changed_range() {
    let mut e = ts_engine();
    assert!(e.ensure_grammar("source.ts").unwrap());
    let lines = [
        "const s = `",
        "function not_code() {",
        "const fake = 1",
        "`",
        "const real = 2",
    ];
    e.open_doc("d", "source.ts", &lines.join("\n"));
    assert_eq!(
        e.tokenize_viewport("d", 3, 3, LONG).mode,
        ViewportMode::Guessed
    );
    let guessed = e.tokens_for_line("d", 3).unwrap();
    let (mut done, mut from, mut to) = (false, None::<usize>, 0);
    for _ in 0..100 {
        let p = e.fill_slice("d", Duration::from_millis(50));
        if let Some(f) = p.changed_from {
            from = Some(from.map_or(f, |c| c.min(f)));
            to = to.max(p.changed_to);
        }
        done = p.done;
        if done {
            break;
        }
    }
    assert!(done);
    assert_eq!(e.filled_through("d"), lines.len());
    assert_eq!(e.provisional_count("d"), 0);
    assert!(from.is_some_and(|f| f <= 3));
    assert!(to >= 3);
    assert_ne!(*e.tokens_for_line("d", 3).unwrap(), *guessed);
}

#[test]
fn fill_advances_monotonically_under_a_tiny_budget() {
    let mut e = ts_engine();
    assert!(e.ensure_grammar("source.ts").unwrap());
    e.open_doc("d", "source.ts", &synth_ts(400));
    let mut prev = 0;
    let mut done = false;
    for _ in 0..10_000 {
        let p = e.fill_slice("d", Duration::from_micros(50));
        assert!(p.filled >= prev);
        prev = p.filled;
        done = p.done;
        if done {
            break;
        }
    }
    assert!(done);
    assert_eq!(e.filled_through("d"), 400);
}

#[test]
fn fill_jobs_on_another_thread_match_the_foreground_fill_and_drop_stale_results() {
    let text = synth_ts(1200);
    let mut fg = ts_engine();
    assert!(fg.ensure_grammar("source.ts").unwrap());
    fg.open_doc("d", "source.ts", &text);
    assert!(fg.fill_sync("d", LONG));
    let mut bg = ts_engine();
    assert!(bg.ensure_grammar("source.ts").unwrap());
    bg.open_doc("d", "source.ts", &text);

    let job = bg.fill_job("d", 300).unwrap();
    let stale = std::thread::spawn(move || job.run(LONG)).join().unwrap();
    bg.edit_doc(
        "d",
        &LineReplace {
            from_line: 1,
            old_line_count: 1,
            new_lines: vec!["export function f0(x: number): number {".into()],
        },
    );
    assert_eq!(bg.apply_fill("d", stale).filled, 0);

    while let Some(job) = bg.fill_job("d", 300) {
        let chunk = std::thread::spawn(move || job.run(LONG)).join().unwrap();
        bg.apply_fill("d", chunk);
    }
    assert_eq!(bg.filled_through("d"), 1200);
    for n in 1..=1200 {
        assert_eq!(bg.tokens_for_line("d", n), fg.tokens_for_line("d", n));
    }
}

#[test]
fn close_doc_drops_a_live_doc_close_if_idle_does_not() {
    let mut e = TmEngine::new();
    e.open_doc("live.ts", "source.ts", "const a = 1");
    assert!(!e.close_if_idle("live.ts"));
    assert!(e.has_doc("live.ts"));
    e.close_doc("live.ts");
    assert!(!e.has_doc("live.ts"));
}

#[test]
fn match_doc_samples_so_it_cannot_police_a_reload() {
    let mut e = TmEngine::new();
    let a: Vec<String> = (0..1000).map(|i| format!("const v{i} = 1")).collect();
    let mut b = a.clone();
    b[49] = "const v49 = 2".into();
    let fp = |lines: &[String]| {
        let text = lines.join("\n");
        tm_doc_fingerprint(|n| lines[n - 1].clone(), lines.len(), text.len())
    };
    e.open_doc("big.ts", "source.ts", &a.join("\n"));
    assert!(e.match_doc("big.ts", "source.ts", fp(&b)));
    let mut sampled = a.clone();
    sampled[32] = "const v32 = 2".into();
    assert!(!e.match_doc("big.ts", "source.ts", fp(&sampled)));
}

#[test]
fn retains_inactive_docs_and_frees_them_on_close() {
    let mut e = TmEngine::new();
    e.open_doc("a.ts", "source.ts", "const a = 1");
    e.open_doc("b.ts", "source.ts", "const b = 2");
    e.release_doc("a.ts");
    assert!(e.has_doc("a.ts"));
    assert!(!e.close_if_idle("b.ts"));
    assert!(e.has_doc("b.ts"));
    assert!(e.close_if_idle("a.ts"));
    assert!(!e.has_doc("a.ts"));
}

#[test]
fn evicts_idle_docs_by_total_lines() {
    let mut e = TmEngine::new();
    let big = vec!["x"; (MAX_IDLE_LINES as f64 * 0.6).ceil() as usize].join("\n");
    for id in ["b1", "b2"] {
        e.open_doc(id, "source.ts", &big);
        e.release_doc(id);
    }
    assert!(!e.has_doc("b1"));
    assert!(e.has_doc("b2"));
    for i in 0..MAX_IDLE_DOCS - 1 {
        let id = format!("s{i}");
        e.open_doc(&id, "source.ts", "const x = 1");
        e.release_doc(&id);
    }
    assert!(e.has_doc("s0"));
    assert!(e.has_doc(&format!("s{}", MAX_IDLE_DOCS - 2)));
}

#[test]
fn the_sweep_after_a_mid_file_edit_stops_when_stable() {
    let mut e = ts_engine();
    assert!(e.ensure_grammar("source.ts").unwrap());
    e.open_doc("d2", "source.ts", &synth_ts(200));
    e.tokenize_viewport("d2", 1, 200, LONG);
    assert_eq!(e.filled_through("d2"), 200);
    e.edit_doc("d2", &replace(37, 1, &["  const y = x + 99"]));
    assert_eq!(e.filled_through("d2"), 36);
    let after = e.tokenize_viewport("d2", 1, 80, LONG);
    assert!(after.tokenized <= 3);
    assert_eq!(e.filled_through("d2"), 200);
}

#[test]
fn scopes_for_line_names_a_typescript_function() {
    let mut e = ts_engine();
    assert!(e.ensure_grammar("source.ts").unwrap());
    e.open_doc("fn.ts", "source.ts", "export function hello() {}\n");
    assert!(e.fill_sync("fn.ts", LONG));
    let toks = e.scopes_for_line("fn.ts", 1).unwrap();
    let line = e.line_at("fn.ts", 1).unwrap().to_string();
    let names: Vec<&str> = toks
        .iter()
        .filter(|t| {
            t.scopes
                .iter()
                .any(|s| s.starts_with("entity.name.function"))
        })
        .map(|t| &line[t.start..t.end])
        .collect();
    assert_eq!(names, ["hello"]);
}

#[test]
fn re_noting_a_grammar_reloads_its_source() {
    let mut e = TmEngine::new();
    let version = Rc::new(Cell::new(1));
    let loads = Rc::new(Cell::new(0));
    let (v, l) = (version.clone(), loads.clone());
    e.set_source_loader(Some(Box::new(move |_| {
        l.set(l.get() + 1);
        Some(GrammarSourceText {
            content: format!(
                r#"{{"scopeName":"source.demo","patterns":[{{"match":"x","name":"v{}"}}]}}"#,
                v.get()
            ),
            path_hint: "demo.json".into(),
        })
    })));
    e.note_grammar("source.demo", "demo.json", &[]);
    assert!(e.ensure_grammar("source.demo").unwrap());
    version.set(2);
    e.note_grammar("source.demo", "demo.json", &[]);
    assert!(e.ensure_grammar("source.demo").unwrap());
    assert_eq!(loads.get(), 2);
}

#[test]
fn a_theme_change_drops_every_document() {
    let mut e = ts_engine();
    e.set_theme(flat_theme());
    assert!(e.ensure_grammar("source.ts").unwrap());
    e.open_doc("t.ts", "source.ts", "const themed = 1");
    let map = e.set_theme(RawTheme {
        name: None,
        settings: vec![
            RawThemeSetting {
                foreground: Some("#123456".into()),
                ..RawThemeSetting::default()
            },
            RawThemeSetting {
                scope: ThemeScope::One("keyword".into()),
                foreground: Some("#654321".into()),
                ..RawThemeSetting::default()
            },
        ],
    });
    assert!(!e.has_doc("t.ts"));
    let kw = map
        .iter()
        .position(|c| c.eq_ignore_ascii_case("#654321"))
        .unwrap() as u32;
    e.open_doc("t.ts", "source.ts", "const themed = 1");
    e.tokenize_viewport("t.ts", 1, 1, LONG);
    let toks = e.tokens_for_line("t.ts", 1).unwrap();
    assert!(toks.chunks(2).any(|p| decode_foreground(p[1]) == kw));
}

/// All shipped grammars, keyed by scope, registered on one engine.
fn all_grammars() -> (TmEngine, Vec<String>) {
    let mut paths = Vec::new();
    for bundle in std::fs::read_dir(repo().join("bundles")).unwrap().flatten() {
        if let Ok(entries) = std::fs::read_dir(bundle.path().join("syntaxes")) {
            paths.extend(entries.flatten().map(|e| e.path()));
        }
    }
    let mut e = TmEngine::new();
    let mut scopes = Vec::new();
    for p in &paths {
        let content = std::fs::read_to_string(p).unwrap();
        let raw = recur_textmate::parse_raw_grammar(&content, p.to_str()).unwrap();
        e.register_grammar(&raw.scope_name, &content, p.to_str().unwrap(), &[]);
        scopes.push(raw.scope_name);
    }
    (e, scopes)
}

fn scope_lists(
    e: &mut TmEngine,
    id: &str,
    scope: &str,
    text: &str,
) -> Vec<Vec<(String, Vec<String>)>> {
    assert!(e.ensure_grammar(scope).unwrap());
    e.open_doc(id, scope, text);
    assert!(e.fill_sync(id, LONG));
    let lines: Vec<&str> = text.split('\n').collect();
    (1..=lines.len())
        .map(|n| {
            e.scopes_for_line(id, n)
                .unwrap()
                .into_iter()
                .map(|t| {
                    (
                        lines[n - 1][t.start..t.end.min(lines[n - 1].len())].to_string(),
                        t.scopes,
                    )
                })
                .collect()
        })
        .collect()
}

#[test]
fn svelte_injections_embed_typescript_and_css() {
    let (mut e, _) = all_grammars();
    let doc = "<script lang=\"ts\">\nconst a: number = 1\n</script>\n<style>\n.x { color: red; }\n</style>";
    let lines = scope_lists(&mut e, "a.svelte", "source.svelte", doc);
    let ts_line = &lines[1];
    let konst = ts_line.iter().find(|(v, _)| v == "const").unwrap();
    assert!(konst.1.iter().any(|s| s == "source.ts"), "{konst:?}");
    assert!(
        konst.1.iter().any(|s| s.starts_with("storage.type")),
        "{konst:?}"
    );
    let css_line = &lines[4];
    let prop = css_line.iter().find(|(v, _)| v == "color").unwrap();
    assert!(prop.1.iter().any(|s| s == "source.css"), "{prop:?}");
    assert!(
        prop.1
            .iter()
            .any(|s| s.starts_with("support.type.property-name")),
        "{prop:?}"
    );
}

#[test]
fn markdown_fences_embed_other_grammars() {
    let (mut e, _) = all_grammars();
    let doc = "# Title\n```go\nfunc main() {}\n```";
    let lines = scope_lists(&mut e, "a.md", "text.html.markdown", doc);
    let func = lines[2]
        .iter()
        .find(|(v, _)| v == "func")
        .unwrap_or_else(|| panic!("{lines:?}"));
    // Like VS Code: the fence's contentName, then Go's own scopes (an
    // included grammar's root scope is not pushed).
    assert!(
        func.1.iter().any(|s| s == "meta.embedded.block.go"),
        "{func:?}"
    );
    assert!(
        func.1.iter().any(|s| s == "keyword.function.go"),
        "{func:?}"
    );
    assert!(
        lines[0]
            .iter()
            .any(|(_, sc)| sc.iter().any(|s| s.starts_with("markup.heading")))
    );
}

#[test]
fn inject_to_grammars_apply_inside_their_host() {
    let mut e = TmEngine::new();
    e.register_grammar(
        "source.host",
        r#"{"scopeName":"source.host","patterns":[{"begin":"\"","end":"\"","name":"string.quoted.host"}]}"#,
        "host.json",
        &[],
    );
    e.register_grammar(
        "source.todo-injection",
        r#"{"scopeName":"source.todo-injection","injectionSelector":"L:string","patterns":[{"match":"TODO","name":"keyword.todo"}]}"#,
        "todo.json",
        &["source.host".into()],
    );
    let lines = scope_lists(&mut e, "h", "source.host", "x \"a TODO b\" TODO");
    let todos: Vec<&Vec<String>> = lines[0]
        .iter()
        .filter(|(v, _)| v.contains("TODO"))
        .map(|(_, s)| s)
        .collect();
    assert_eq!(todos.len(), 2, "{lines:?}");
    assert!(todos[0].iter().any(|s| s == "keyword.todo"), "{lines:?}");
    assert!(!todos[1].iter().any(|s| s == "keyword.todo"), "{lines:?}");
}

fn dotenv_engine(theme_rel: &str, text: &str) -> TmEngine {
    let mut e = TmEngine::new();
    e.register_grammar(
        "source.dotenv",
        &read("bundles/Languages/syntaxes/dotenv.tmLanguage.json"),
        "dotenv.tmLanguage.json",
        &[],
    );
    let parsed = parse_theme_json(&read(theme_rel), "theme").unwrap();
    e.set_theme(parsed.raw_theme);
    assert!(e.ensure_grammar("source.dotenv").unwrap());
    e.open_doc("env", "source.dotenv", text);
    e.tokenize_viewport("env", 1, 1, LONG);
    e
}

fn color_at(e: &mut TmEngine, text: &str, needle: &str) -> String {
    let toks = e.tokens_for_line("env", 1).unwrap();
    let map = e.color_map();
    let at = text.find(needle).unwrap();
    for i in (0..toks.len()).step_by(2) {
        let start = toks[i] as usize;
        let end = if i + 2 < toks.len() {
            toks[i + 2] as usize
        } else {
            text.len()
        };
        if at >= start && at < end {
            return map[decode_foreground(toks[i + 1]) as usize].to_lowercase();
        }
    }
    panic!("no token covering {needle}");
}

const LINE: &str = "WAILS_VITE_PORT=$RECUR_PORT/extra";

#[test]
fn charcoal_paints_dotenv_roles() {
    let mut e = dotenv_engine("bundles/Recur/themes/charcoal.tmTheme.json", LINE);
    assert_eq!(color_at(&mut e, LINE, "WAILS_VITE_PORT"), "#6eb6ff");
    assert_eq!(color_at(&mut e, LINE, "="), "#969490");
    assert_eq!(color_at(&mut e, LINE, "$RECUR_PORT"), "#6eb6ff");
    assert_eq!(color_at(&mut e, LINE, "/extra"), "#9fe3b5");
}

#[test]
fn paper_paints_dotenv_roles() {
    let mut e = dotenv_engine("bundles/Recur/themes/paper.tmTheme.json", LINE);
    assert_eq!(color_at(&mut e, LINE, "WAILS_VITE_PORT"), "#2a7eb5");
    assert_eq!(color_at(&mut e, LINE, "="), "#5a616c");
    assert_eq!(color_at(&mut e, LINE, "$RECUR_PORT"), "#2a7eb5");
    assert_eq!(color_at(&mut e, LINE, "/extra"), "#2d9a5a");
}

#[test]
fn highlights_vars_inside_double_quoted_values() {
    let quoted = "PATH=\"$HOME/bin\"";
    let mut e = dotenv_engine("bundles/Recur/themes/charcoal.tmTheme.json", quoted);
    assert_eq!(color_at(&mut e, quoted, "$HOME"), "#6eb6ff");
}

#[test]
fn classic_theme_ports_are_baked_to_srgb() {
    let theme_dir = "bundles/Themes/themes";
    let filmgirl: Vec<serde_json::Value> =
        serde_json::from_str(&read("bundles/Themes/filmgirl-local.json")).unwrap();
    let mut classic: Vec<String> = filmgirl
        .iter()
        .map(|t| t["file"].as_str().unwrap().to_string())
        .collect();
    classic.push("made-of-code.tmTheme.json".into());
    classic.push("birds-of-paradise.tmTheme.json".into());
    for file in &classic {
        let json: serde_json::Value =
            serde_json::from_str(&read(&format!("{theme_dir}/{file}"))).unwrap();
        assert_eq!(json["colorSpaceName"].as_str(), Some("sRGB"), "{file}");
    }
    for file in ["catppuccin-frappe.tmTheme.json", "aura.tmTheme.json"] {
        let json: serde_json::Value =
            serde_json::from_str(&read(&format!("{theme_dir}/{file}"))).unwrap();
        assert!(json.get("colorSpaceName").is_none(), "{file}");
    }
    let charcoal_text = read("bundles/Recur/themes/charcoal.tmTheme.json");
    let charcoal: serde_json::Value = serde_json::from_str(&charcoal_text).unwrap();
    assert!(charcoal.get("colorSpaceName").is_none());
    assert_eq!(
        parse_theme_json(&charcoal_text, "t").unwrap().colors["editor.background"],
        charcoal["colors"]["editor.background"].as_str().unwrap()
    );
    let bg = |f: &str| {
        parse_theme_json(&read(&format!("{theme_dir}/{f}")), "t")
            .unwrap()
            .colors
    };
    let birds = bg("birds-of-paradise.tmTheme.json");
    assert_eq!(birds["editor.background"], "#473431");
    assert_eq!(birds["editor.selectionBackground"], "#B51C53");
    let monokai = bg("monokai.tmTheme.json");
    assert_eq!(monokai["editor.background"], "#34352D");
    assert_eq!(monokai["editor.selectionBackground"], "#5B5A4F");
    assert_eq!(
        bg("cool-glow.tmTheme.json")["editor.background"],
        "#040727FA"
    );
    assert_eq!(
        bg("made-of-code.tmTheme.json")["editor.background"],
        "#08091D"
    );
}

/// Every shipped grammar compiles and tokenizes without a regex error.
#[test]
fn every_shipped_grammar_loads() {
    let (mut e, scopes) = all_grammars();
    assert!(scopes.len() > 50);
    for scope in &scopes {
        assert!(e.ensure_grammar(scope).unwrap(), "{scope}");
        let id = format!("doc-{scope}");
        e.open_doc(&id, scope, "a = \"b\" // c\n<div> { x } </div>\n# heading");
        let r = e.tokenize_viewport(&id, 1, 3, LONG);
        assert_eq!(r.mode, ViewportMode::Accurate, "{scope}");
        assert!(r.complete, "{scope}");
    }
}

#[test]
fn a_deep_grammar_compiles_on_a_gcd_sized_stack() {
    let depth = 500;
    let repo = (0..depth)
        .map(|i| format!(r##""r{i}":{{"patterns":[{{"include":"#r{}"}}]}}"##, i + 1))
        .chain([format!(r#""r{depth}":{{"match":"x","name":"keyword.x"}}"#)])
        .collect::<Vec<_>>()
        .join(",");
    let json = format!(
        r##"{{"scopeName":"source.deep","patterns":[{{"include":"#r0"}}],"repository":{{{repo}}}}}"##
    );
    let mut e = TmEngine::new();
    e.register_grammar("source.deep", &json, "deep.json", &[]);
    assert!(matches!(e.ensure_grammar("source.deep"), Ok(true)));
    let g = e.grammar("source.deep").unwrap();
    std::thread::Builder::new()
        .stack_size(512 << 10)
        .spawn(move || g.tokenize_line("x", None).unwrap().tokens.len())
        .unwrap()
        .join()
        .unwrap();
}
