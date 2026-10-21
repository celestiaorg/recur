//! `lib/skills.ts`: the Settings → Skills and Project → Skills rules.

use std::sync::LazyLock;

use recur_domain::AgentSkill;
use regex::Regex;

/// One skill row in the settings lists.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SkillRow {
    pub name: String,
    pub description: String,
    pub scope: String,
    pub path: String,
}

/// Where a skill comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillKind {
    Recur,
    Project,
    User,
}

fn slashed(path: &str) -> String {
    path.replace('\\', "/")
}

pub fn is_recur_skill_path(path: &str) -> bool {
    slashed(path).contains("/.recur/recur-skills/")
}

pub fn skill_kind(s: &SkillRow) -> SkillKind {
    if is_recur_skill_path(&s.path) {
        SkillKind::Recur
    } else if s.scope == "workspace" {
        SkillKind::Project
    } else {
        SkillKind::User
    }
}

/// Skill-root markers. Several also appear inside repos (`.claude/skills`),
/// so they never infer "home" for workspace skills.
const SKILL_ROOT_MARKERS: [&str; 6] = [
    "/.recur/recur-skills/",
    "/.claude/skills/",
    "/.agents/skills/",
    "/.cursor/skills/",
    "/.codex/skills/",
    "/.gemini/skills/",
];

/// Never a workspace skill root.
const HOME_ONLY_MARKERS: [&str; 1] = ["/.recur/recur-skills/"];

static HOME_SHAPE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?:/Users/[^/]+|/home/[^/]+|[A-Za-z]:/Users/[^/]+)$").expect("home pattern")
});

/// The machine's home directory, inferred from a user or Recur skill path
/// (Unix and Windows shapes); `""` when unclear.
pub fn infer_home_from_skill_path(path: &str) -> String {
    let p = slashed(path);
    for m in HOME_ONLY_MARKERS {
        if let Some(i) = p.find(m).filter(|&i| i > 0) {
            return p[..i].to_string();
        }
    }
    for m in SKILL_ROOT_MARKERS {
        if let Some(i) = p.find(m).filter(|&i| i > 0)
            && HOME_SHAPE.is_match(&p[..i])
        {
            return p[..i].to_string();
        }
    }
    String::new()
}

static SKILL_MD_SUFFIX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)/SKILL\.md$").expect("SKILL.md pattern"));

/// The directory holding SKILL.md, for display: repo-relative for workspace
/// skills (`.claude/skills/foo`), `~`-shortened for user and Recur skills.
pub fn skill_location(path: &str, scope: &str, home_dir: &str) -> String {
    if path.is_empty() {
        return String::new();
    }
    // The UI strips `/SKILL.md` and then `/skill.md`, each case-insensitive.
    let once = SKILL_MD_SUFFIX.replace(&slashed(path), "").into_owned();
    let dir = SKILL_MD_SUFFIX.replace(&once, "").into_owned();
    if scope == "workspace" {
        for m in SKILL_ROOT_MARKERS {
            if let Some(i) = dir.find(m) {
                return dir[i + 1..].to_string();
            }
        }
        return dir;
    }
    let home = if home_dir.is_empty() {
        infer_home_from_skill_path(path)
    } else {
        home_dir.to_string()
    };
    let home = home.strip_suffix('/').unwrap_or(&home);
    if !home.is_empty() && (dir == home || dir.starts_with(&format!("{home}/"))) {
        return format!("~{}", &dir[home.len()..]);
    }
    dir
}

/// Discovered skills as rows (missing scope is `user`).
pub fn map_skill_rows(list: &[AgentSkill]) -> Vec<SkillRow> {
    list.iter()
        .map(|s| SkillRow {
            name: s.name.clone(),
            description: s.description.clone(),
            scope: if s.scope.is_empty() {
                "user".into()
            } else {
                s.scope.clone()
            },
            path: s.path.clone(),
        })
        .collect()
}

pub fn name_listed(list: &[String], name: &str) -> bool {
    list.iter().any(|n| n == name)
}

/// On or off given the global and project override lists: project disable,
/// then project enable, then the global default.
pub fn effective_skill_enabled(
    name: &str,
    global_disabled: &[String],
    project_disabled: &[String],
    project_enabled: &[String],
) -> bool {
    if name_listed(project_disabled, name) {
        return false;
    }
    if name_listed(project_enabled, name) {
        return true;
    }
    !name_listed(global_disabled, name)
}

/// The global disabled list after turning `name` on or off.
pub fn toggle_disabled_name(cur: &[String], name: &str, enabled: bool) -> Vec<String> {
    if enabled {
        return cur.iter().filter(|x| *x != name).cloned().collect();
    }
    let mut out = cur.to_vec();
    if !name_listed(cur, name) {
        out.push(name.into());
    }
    out
}

/// The project's override lists after making `name` effectively
/// `want_enabled`, storing only differences from the global default.
pub fn project_skill_override_lists(
    name: &str,
    want_enabled: bool,
    global_disabled: &[String],
    project_disabled: &[String],
    project_enabled: &[String],
) -> (Vec<String>, Vec<String>) {
    let global_on = !name_listed(global_disabled, name);
    let drop = |v: &[String]| -> Vec<String> { v.iter().filter(|x| *x != name).cloned().collect() };
    let add = |v: &[String]| -> Vec<String> {
        let mut out = v.to_vec();
        if !name_listed(v, name) {
            out.push(name.into());
        }
        out
    };
    if want_enabled {
        let enabled = if global_on {
            drop(project_enabled)
        } else {
            add(project_enabled)
        };
        (drop(project_disabled), enabled)
    } else {
        let disabled = if global_on {
            add(project_disabled)
        } else {
            drop(project_disabled)
        };
        (disabled, drop(project_enabled))
    }
}

/// The note shown when a project overrides the global state.
pub fn skill_override_hint(
    name: &str,
    global_disabled: &[String],
    project_disabled: &[String],
    project_enabled: &[String],
) -> &'static str {
    let global_on = !name_listed(global_disabled, name);
    let on = effective_skill_enabled(name, global_disabled, project_disabled, project_enabled);
    if on && !global_on {
        "On for this project (off globally)"
    } else if !on && global_on {
        "Off for this project"
    } else {
        ""
    }
}

/// Project overrides that name one of `skills`.
pub fn skill_kind_override_count(
    skills: &[SkillRow],
    project_disabled: &[String],
    project_enabled: &[String],
) -> usize {
    let names: std::collections::HashSet<&str> = skills.iter().map(|s| s.name.as_str()).collect();
    project_disabled
        .iter()
        .chain(project_enabled)
        .filter(|n| names.contains(n.as_str()))
        .count()
}

pub fn skill_kind_has_overrides(
    skills: &[SkillRow],
    kind: SkillKind,
    project_disabled: &[String],
    project_enabled: &[String],
) -> bool {
    let of_kind: Vec<SkillRow> = skills
        .iter()
        .filter(|s| skill_kind(s) == kind)
        .cloned()
        .collect();
    skill_kind_override_count(&of_kind, project_disabled, project_enabled) > 0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    fn row(name: &str, scope: &str, path: &str) -> SkillRow {
        SkillRow {
            name: name.into(),
            scope: scope.into(),
            path: path.into(),
            ..Default::default()
        }
    }

    #[test]
    fn kinds() {
        assert_eq!(
            skill_kind(&row("a", "user", "/Users/j/.recur/recur-skills/a/SKILL.md")),
            SkillKind::Recur
        );
        assert_eq!(
            skill_kind(&row("a", "workspace", "/r/.claude/skills/a/SKILL.md")),
            SkillKind::Project
        );
        assert_eq!(
            skill_kind(&row("a", "user", "/h/.claude/skills/a/SKILL.md")),
            SkillKind::User
        );
        assert!(is_recur_skill_path(
            r"C:\Users\j\.recur\recur-skills\x\SKILL.md"
        ));
    }

    #[test]
    fn infer_home() {
        assert_eq!(
            infer_home_from_skill_path("/opt/x/.recur/recur-skills/a/SKILL.md"),
            "/opt/x"
        );
        assert_eq!(
            infer_home_from_skill_path("/Users/j/.claude/skills/a/SKILL.md"),
            "/Users/j"
        );
        assert_eq!(
            infer_home_from_skill_path("/home/j/.cursor/skills/a/SKILL.md"),
            "/home/j"
        );
        assert_eq!(
            infer_home_from_skill_path(r"C:\Users\j\.codex\skills\a\SKILL.md"),
            "C:/Users/j"
        );
        assert_eq!(
            infer_home_from_skill_path("/work/repo/.claude/skills/a/SKILL.md"),
            ""
        );
        assert_eq!(infer_home_from_skill_path("/.claude/skills/a/SKILL.md"), "");
    }

    #[test]
    fn locations() {
        assert_eq!(
            skill_location("/work/repo/.claude/skills/foo/SKILL.md", "workspace", ""),
            ".claude/skills/foo"
        );
        assert_eq!(
            skill_location("/work/foo/skill.md", "workspace", ""),
            "/work/foo"
        );
        assert_eq!(
            skill_location("/Users/j/.claude/skills/foo/SKILL.md", "user", ""),
            "~/.claude/skills/foo"
        );
        assert_eq!(
            skill_location("/srv/u/.agents/skills/foo/SKILL.md", "user", "/srv/u/"),
            "~/.agents/skills/foo"
        );
        assert_eq!(skill_location("/x/foo/SKILL.md", "user", ""), "/x/foo");
        assert_eq!(skill_location("", "user", ""), "");
        assert_eq!(
            skill_location("/w/a/skill.md/SKILL.md", "workspace", ""),
            "/w/a"
        );
    }

    #[test]
    fn rows_from_skills() {
        let rows = map_skill_rows(&[AgentSkill {
            name: "a".into(),
            ..Default::default()
        }]);
        assert_eq!(rows[0].scope, "user");
    }

    #[test]
    fn enablement() {
        let global = s(&["noisy"]);
        assert!(!effective_skill_enabled("noisy", &global, &[], &[]));
        assert!(effective_skill_enabled(
            "noisy",
            &global,
            &[],
            &s(&["noisy"])
        ));
        assert!(!effective_skill_enabled("x", &[], &s(&["x"]), &s(&["x"])));
        assert_eq!(toggle_disabled_name(&s(&["a", "b"]), "a", true), ["b"]);
        assert_eq!(toggle_disabled_name(&s(&["a"]), "b", false), ["a", "b"]);
        assert_eq!(toggle_disabled_name(&s(&["a"]), "a", false), ["a"]);
    }

    #[test]
    fn project_overrides_store_only_differences() {
        // Globally on: turning it off adds a project disable.
        assert_eq!(
            project_skill_override_lists("x", false, &[], &[], &s(&["x"])),
            (s(&["x"]), vec![])
        );
        // Globally on: turning it back on just clears overrides.
        assert_eq!(
            project_skill_override_lists("x", true, &[], &s(&["x"]), &[]),
            (vec![], vec![])
        );
        // Globally off: turning it on needs a project enable.
        assert_eq!(
            project_skill_override_lists("x", true, &s(&["x"]), &s(&["x"]), &[]),
            (vec![], s(&["x"]))
        );
        assert_eq!(
            project_skill_override_lists("x", false, &s(&["x"]), &s(&["x"]), &s(&["x"])),
            (vec![], vec![])
        );
        assert_eq!(
            skill_override_hint("x", &s(&["x"]), &[], &s(&["x"])),
            "On for this project (off globally)"
        );
        assert_eq!(
            skill_override_hint("x", &[], &s(&["x"]), &[]),
            "Off for this project"
        );
        assert_eq!(skill_override_hint("x", &[], &[], &[]), "");
    }

    #[test]
    fn override_counts() {
        let skills = [
            row("a", "workspace", "/r/.claude/skills/a/SKILL.md"),
            row("b", "user", "/Users/j/.claude/skills/b/SKILL.md"),
        ];
        assert_eq!(
            skill_kind_override_count(&skills, &s(&["a", "z"]), &s(&["b"])),
            2
        );
        assert!(skill_kind_has_overrides(
            &skills,
            SkillKind::Project,
            &s(&["a"]),
            &[]
        ));
        assert!(!skill_kind_has_overrides(
            &skills,
            SkillKind::User,
            &s(&["a"]),
            &[]
        ));
    }
}
