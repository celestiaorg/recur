//! Agent Skills (`SKILL.md`) discovery under the worktree and the user's home
//! skill roots, and the frontmatter rules for activation.

use std::collections::BTreeMap;
use std::fs;
use std::sync::LazyLock;

use recur_domain::AgentSkill;
use recur_domain::path;
use regex::Regex;
use tokio_util::sync::CancellationToken;

use crate::error::{Error, Result};
use crate::gostr::{collapse_space, rune_start};
use crate::walk::{Flow, walk_dir};

static SKILL_NAME: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[a-z][a-z0-9-]*$").expect("skill name pattern"));

/// Skills under the worktree and the user's home skill roots (from
/// `recur_domain::skill_roots`), deduped by name with the workspace winning,
/// sorted by name. Includes model-only skills (`user-invocable: false`); the
/// composer menu uses [`list_skills_for_menu`].
pub fn list_skills(cancel: &CancellationToken, worktree: &str) -> Result<Vec<AgentSkill>> {
    let home = std::env::var("HOME").unwrap_or_default();
    list_skills_in(cancel, worktree, &home)
}

pub(crate) fn list_skills_in(
    cancel: &CancellationToken,
    worktree: &str,
    home: &str,
) -> Result<Vec<AgentSkill>> {
    let (ws_roots, user_roots) = recur_domain::skill_roots();
    let mut by_name: BTreeMap<String, AgentSkill> = BTreeMap::new();
    let mut scan = |roots: &[&str], base: &str, scope: &str| -> Result<()> {
        for rel in roots {
            if cancel.is_cancelled() {
                return Err(Error::Canceled);
            }
            collect_skills(cancel, &path::join(&[base, rel]), scope, &mut by_name)?;
        }
        Ok(())
    };
    if !worktree.is_empty() {
        scan(&ws_roots, worktree, "workspace")?;
    }
    if !home.is_empty() {
        scan(&user_roots, home, "user")?;
    }
    Ok(by_name.into_values().collect())
}

/// [`list_skills`] limited to user-invocable skills, for the composer `/`
/// menu. Model-only skills stay available through skill activation.
pub fn list_skills_for_menu(cancel: &CancellationToken, worktree: &str) -> Result<Vec<AgentSkill>> {
    Ok(list_skills(cancel, worktree)?
        .into_iter()
        .filter(|s| s.user_invocable)
        .collect())
}

/// The skill named `name`, if any.
pub fn skill_by_name(
    cancel: &CancellationToken,
    worktree: &str,
    name: &str,
) -> Result<Option<AgentSkill>> {
    let name = name.trim();
    if name.is_empty() || !SKILL_NAME.is_match(name) {
        return Ok(None);
    }
    Ok(list_skills(cancel, worktree)?
        .into_iter()
        .find(|s| s.name == name))
}

fn collect_skills(
    cancel: &CancellationToken,
    root: &str,
    scope: &str,
    by_name: &mut BTreeMap<String, AgentSkill>,
) -> Result<()> {
    if !fs::metadata(root).is_ok_and(|m| m.is_dir()) {
        return Ok(());
    }
    walk_dir(root, |path, d, err| {
        let (Some(d), None) = (d, err) else {
            return Ok(Flow::Continue);
        };
        if cancel.is_cancelled() {
            return Err(Error::Canceled);
        }
        if d.is_dir() {
            // Don't descend into huge trees under a misplaced skills root.
            if path != root && (d.name == "node_modules" || d.name == ".git") {
                return Ok(Flow::SkipDir);
            }
            return Ok(Flow::Continue);
        }
        if d.name != "SKILL.md" && d.name != "skill.md" {
            return Ok(Flow::Continue);
        }
        let name = path::base(&path::dir(path));
        if !SKILL_NAME.is_match(&name) || by_name.contains_key(&name) {
            return Ok(Flow::Continue);
        }
        // Description is required for progressive-disclosure catalogs.
        let Some((meta, _)) = read_skill_file(path) else {
            return Ok(Flow::Continue);
        };
        if meta.description.is_empty() {
            return Ok(Flow::Continue);
        }
        by_name.insert(
            name.clone(),
            AgentSkill {
                name,
                description: truncate_skill_desc(&meta.description, 160),
                scope: scope.to_string(),
                path: path.to_string(),
                user_invocable: meta.user_invocable,
            },
        );
        Ok(Flow::Continue)
    })
}

struct SkillFileMeta {
    description: String,
    user_invocable: bool,
    disable_model_invocation: bool,
}

/// Frontmatter and body of a SKILL.md; `None` when unreadable or with
/// neither a description nor a body.
fn read_skill_file(path: &str) -> Option<(SkillFileMeta, String)> {
    let raw = String::from_utf8_lossy(&fs::read(path).ok()?).into_owned();
    let (meta, body, ok) = parse_skill_file(&raw);
    ok.then_some((meta, body))
}

fn parse_skill_file(raw: &str) -> (SkillFileMeta, String, bool) {
    let (fm, body) = frontmatter_block(raw);
    let meta = SkillFileMeta {
        description: parse_frontmatter_description(fm),
        user_invocable: !frontmatter_bool_false(fm, "user-invocable"),
        disable_model_invocation: frontmatter_bool_true(fm, "disable-model-invocation"),
    };
    let body = if fm.is_empty() {
        raw.trim()
    } else {
        body.trim()
    };
    let ok = !(meta.description.is_empty() && body.is_empty());
    (meta, body.to_string(), ok)
}

/// Every field `read_skill_file` returns, for the golden replay.
#[cfg(test)]
pub(crate) fn read_skill_meta(path: &str) -> (String, bool, bool, String, bool) {
    let Ok(raw) = fs::read(path) else {
        return (String::new(), false, false, String::new(), false);
    };
    let (m, body, ok) = parse_skill_file(&String::from_utf8_lossy(&raw));
    (
        m.description,
        m.user_invocable,
        m.disable_model_invocation,
        body,
        ok,
    )
}

/// The SKILL.md body without frontmatter (activation content); empty when
/// unreadable.
pub fn read_skill_body(path: &str) -> String {
    read_skill_file(path)
        .map(|(_, body)| body)
        .unwrap_or_default()
}

/// Whether the skill opted out of model-driven activation (user `/`
/// activation still works).
pub fn skill_disables_model_invocation(path: &str) -> bool {
    read_skill_file(path).is_some_and(|(meta, _)| meta.disable_model_invocation)
}

/// Skill-directory-relative paths of bundled scripts, references and assets
/// (not read), sorted and capped at `limit` (0 means 40) for prompt size.
pub fn list_skill_resources(skill_md_path: &str, limit: usize) -> Vec<String> {
    let limit = if limit == 0 { 40 } else { limit };
    let dir = path::dir(skill_md_path);
    let mut out = Vec::new();
    let _ = walk_dir::<()>(&dir, |path, d, err| {
        let (Some(d), None) = (d, err) else {
            return Ok(Flow::Continue);
        };
        if path == dir {
            return Ok(Flow::Continue);
        }
        if d.is_dir() {
            if d.name == "node_modules" || d.name == ".git" {
                return Ok(Flow::SkipDir);
            }
            return Ok(Flow::Continue);
        }
        if d.name.eq_ignore_ascii_case("SKILL.md") {
            return Ok(Flow::Continue);
        }
        let Some(rel) = path::rel(&dir, path) else {
            return Ok(Flow::Continue);
        };
        out.push(rel);
        Ok(if out.len() >= limit {
            Flow::SkipAll
        } else {
            Flow::Continue
        })
    });
    out.sort();
    out.truncate(limit);
    out
}

fn frontmatter_block(raw: &str) -> (&str, &str) {
    let text = raw.trim_start_matches(['\u{FEFF}', ' ', '\t']);
    let Some(rest) = text.strip_prefix("---") else {
        return ("", raw.trim());
    };
    let rest = rest.trim_start_matches(['\r', '\n']);
    let Some(end) = rest.find("\n---") else {
        return ("", raw.trim());
    };
    (
        &rest[..end],
        rest[end + 4..].trim_start_matches(['\r', '\n']),
    )
}

fn truncate_skill_desc(val: &str, max: usize) -> String {
    let val = collapse_space(val);
    if max == 0 || val.len() <= max {
        return val;
    }
    let mut cut = max - 1;
    while cut > 0 && !rune_start(val.as_bytes()[cut]) {
        cut -= 1;
    }
    format!("{}…", &val[..cut])
}

fn parse_frontmatter_description(fm: &str) -> String {
    let lines: Vec<&str> = fm.split('\n').collect();
    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim_end_matches('\r').trim();
        let Some(val) = trimmed.strip_prefix("description:") else {
            continue;
        };
        let mut val = val.trim().to_string();
        if matches!(val.as_str(), ">" | "|" | ">-" | "|-") {
            let mut parts: Vec<&str> = Vec::new();
            for l in &lines[i + 1..] {
                let l = l.trim_end_matches('\r');
                if l.starts_with([' ', '\t']) {
                    parts.push(l.trim());
                    continue;
                }
                if l.trim().is_empty() {
                    if !parts.is_empty() {
                        break;
                    }
                    continue;
                }
                break;
            }
            val = parts.join(" ");
        } else {
            val = val.trim_matches(['"', '\'']).to_string();
        }
        return collapse_space(&val);
    }
    String::new()
}

fn frontmatter_bool_true(fm: &str, key: &str) -> bool {
    frontmatter_scalar(fm, key)
        .is_some_and(|v| matches!(v.trim().to_lowercase().as_str(), "true" | "yes" | "1"))
}

/// True when the key is explicitly false (`user-invocable` defaults true).
fn frontmatter_bool_false(fm: &str, key: &str) -> bool {
    frontmatter_scalar(fm, key)
        .is_some_and(|v| matches!(v.trim().to_lowercase().as_str(), "false" | "no" | "0"))
}

fn frontmatter_scalar<'a>(fm: &'a str, key: &str) -> Option<&'a str> {
    let prefix = format!("{key}:");
    fm.split('\n').find_map(|line| {
        line.trim_end_matches('\r')
            .trim()
            .strip_prefix(prefix.as_str())
            .map(|v| v.trim().trim_matches(['"', '\'']))
    })
}

#[cfg(test)]
#[path = "skills_tests.rs"]
mod tests;
