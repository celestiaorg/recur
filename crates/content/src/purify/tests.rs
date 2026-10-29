use super::*;

#[derive(serde::Deserialize)]
struct Fixture {
    input: String,
    plain: String,
    lsp: String,
    markdown: String,
    svg: String,
}

#[derive(serde::Deserialize)]
struct Markdown {
    input: String,
    out: String,
}

#[derive(serde::Deserialize)]
struct Golden {
    fixtures: Vec<Fixture>,
    markdown: Vec<Markdown>,
}

fn golden() -> Golden {
    serde_json::from_str(include_str!("../../tests/golden/purify.json")).unwrap()
}

fn check(label: &str, cases: impl Iterator<Item = (String, String, Config)>) {
    let mut failures = Vec::new();
    let mut n = 0;
    for (input, want, cfg) in cases {
        n += 1;
        let got = sanitize(&input, &cfg);
        if got != want {
            failures.push(format!("--- in\n{input}\n--- want\n{want}\n--- got\n{got}"));
        }
    }
    assert!(
        failures.is_empty(),
        "{label}: {} of {n} differ:\n{}",
        failures.len(),
        failures
            .iter()
            .take(5)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n\n")
    );
}

#[test]
fn matches_webkit_dompurify_on_its_fixtures() {
    let g = golden();
    check(
        "plain",
        g.fixtures.iter().map(|f| {
            (
                f.input.clone(),
                f.plain.clone(),
                Config::default_with(Hook::None),
            )
        }),
    );
    check(
        "lsp",
        g.fixtures.iter().map(|f| {
            (
                f.input.clone(),
                f.lsp.clone(),
                Config::default_with(Hook::Recur),
            )
        }),
    );
    check(
        "markdown",
        g.fixtures
            .iter()
            .map(|f| (f.input.clone(), f.markdown.clone(), Config::markdown())),
    );
    check(
        "svg",
        g.fixtures
            .iter()
            .map(|f| (f.input.clone(), f.svg.clone(), Config::mermaid_svg())),
    );
}

#[test]
fn matches_webkit_on_markdown_output() {
    check(
        "sanitizeMarkdown",
        golden()
            .markdown
            .into_iter()
            .map(|m| (m.input, m.out, Config::markdown())),
    );
}
