use super::*;

#[derive(serde::Deserialize)]
struct Case {
    md: String,
    html: String,
}

#[test]
fn matches_marked_on_the_spec_corpus() {
    let cases: Vec<Case> =
        serde_json::from_str(include_str!("../../tests/golden/marked.json")).unwrap();
    let mut failures = Vec::new();
    for (i, c) in cases.iter().enumerate() {
        let got = match parse(&c.md, &DefaultRenderer) {
            Ok(h) => h,
            Err(e) => format!("ERROR: {e}"),
        };
        if got != c.html && !(c.html.starts_with("ERROR") && got.starts_with("ERROR")) {
            failures.push(format!(
                "#{i}\n--- md\n{}\n--- want\n{}\n--- got\n{}",
                c.md, c.html, got
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} differ:\n{}",
        failures.len(),
        cases.len(),
        failures
            .iter()
            .take(8)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n\n")
    );
}

#[test]
fn renders_a_chat_sample() {
    let html = parse(
        "# Hi\n\n- [x] done\n- **b** `c`\n\n| a | b |\n|:-|-:|\n| 1 | 2 |\nline\nbreak",
        &DefaultRenderer,
    )
    .unwrap();
    assert_eq!(
        html,
        "<h1>Hi</h1>\n<ul>\n<li><input checked=\"\" disabled=\"\" type=\"checkbox\"> done</li>\n<li><strong>b</strong> <code>c</code></li>\n</ul>\n<table>\n<thead>\n<tr>\n<th align=\"left\">a</th>\n<th align=\"right\">b</th>\n</tr>\n</thead>\n<tbody><tr>\n<td align=\"left\">1</td>\n<td align=\"right\">2</td>\n</tr>\n<tr>\n<td align=\"left\">line</td>\n<td align=\"right\"></td>\n</tr>\n<tr>\n<td align=\"left\">break</td>\n<td align=\"right\"></td>\n</tr>\n</tbody></table>\n"
    );
}
