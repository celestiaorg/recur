use std::collections::BTreeMap;

use recur_chroma::html_escape;
use recur_domain::{FileDiff, Finding};

use super::{parse_line, to_slash};

/// Attaches risk-flag markup for open findings to the matching new-side
/// lines (`extra_html`, before what is already there). Reports whether any
/// line changed.
pub fn project_findings_onto_diff(files: &mut [FileDiff], findings: &[Finding]) -> bool {
    let mut by_file: BTreeMap<String, Vec<&Finding>> = BTreeMap::new();
    for f in findings {
        if !f.status.is_empty() && f.status != "open" {
            continue;
        }
        if f.file.is_empty() || f.line <= 0 {
            continue;
        }
        let path = to_slash(&f.file);
        let path = path.strip_prefix("./").unwrap_or(&path).to_string();
        by_file.entry(path).or_default().push(f);
    }
    if by_file.is_empty() {
        return false;
    }
    let mut changed = false;
    for file in files.iter_mut() {
        let path = to_slash(&file.path);
        let mut fs = by_file.get(&path);
        if fs.is_none() {
            // Agents sometimes report short paths: try the basename.
            let base = path.rsplit('/').next().unwrap_or(&path);
            fs = by_file
                .iter()
                .find(|(k, _)| k.ends_with(&format!("/{base}")) || k.as_str() == base)
                .map(|(_, v)| v);
        }
        let Some(fs) = fs else {
            continue;
        };
        for l in file.lines.iter_mut().flatten() {
            if l.r#type != "add" && l.r#type != "ctx" {
                continue;
            }
            let line_no = parse_line(&l.new);
            if line_no <= 0 {
                continue;
            }
            let matched: Vec<&Finding> = fs
                .iter()
                .copied()
                .filter(|f| f.covers_line(line_no))
                .collect();
            let Some(first) = matched.first() else {
                continue;
            };
            if l.extra_html.contains(r#"class="flag""#)
                && l.extra_html.contains(&html_escape(&first.message))
            {
                continue;
            }
            let owned: Vec<Finding> = matched.iter().map(|f| (*f).clone()).collect();
            l.extra_html = finding_flag_html(&owned) + &l.extra_html;
            changed = true;
        }
    }
    changed
}

/// The risk-flag block for one or more findings.
pub fn finding_flag_html(findings: &[Finding]) -> String {
    let mut b = String::new();
    for f in findings {
        let sev = match f.severity.as_str() {
            "" => "medium",
            s => s,
        };
        let title = if sev == "high" || sev == "critical" {
            "Reviewer flagged this region for human eyes"
        } else {
            "Reviewer flagged this region"
        };
        b.push_str(&format!(
            r#"<div class="flag" data-sev="{}"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><circle cx="11" cy="11" r="7"/><path d="M21 21l-4-4" stroke-linecap="round"/></svg><div><div class="ft">{} · {}</div><div class="fm">{}</div></div></div>"#,
            html_escape(sev),
            html_escape(title),
            html_escape(sev),
            html_escape(&f.message),
        ));
    }
    b
}
