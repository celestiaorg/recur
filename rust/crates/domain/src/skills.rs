//! Agent Skill discovery roots and enablement rules (shared with MCP).

/// Relative Agent Skill directory roots (under a worktree or $HOME) that
/// Recur scans for SKILL.md folders, as `(workspace, user)`.
///
/// Recur owns discovery and activation, so every provider sees the same
/// union of common skill locations. Shipped Recur skills live in
/// ~/.recur/recur-skills; Cursor's ~/.cursor/skills-cursor is intentionally
/// excluded.
pub fn skill_roots() -> (Vec<&'static str>, Vec<&'static str>) {
    (
        vec![
            ".cursor/skills",
            ".agents/skills",
            ".claude/skills",
            ".codex/skills",
        ],
        vec![
            ".cursor/skills",
            ".agents/skills",
            ".claude/skills",
            ".codex/skills",
            ".recur/recur-skills",
        ],
    )
}

/// Applies project overrides over the global default. Project disable wins
/// over project enable if both list the same name. A blank name is enabled.
pub fn effective_skill_enabled(
    name: &str,
    global_disabled: &[String],
    project_disabled: &[String],
    project_enabled: &[String],
) -> bool {
    let name = name.trim();
    if name.is_empty() {
        return true;
    }
    if name_listed(project_disabled, name) {
        return false;
    }
    if name_listed(project_enabled, name) {
        return true;
    }
    !name_listed(global_disabled, name)
}

/// False when name appears in disabled. A blank name is enabled.
pub(crate) fn name_enabled(name: &str, disabled: &[String]) -> bool {
    let name = name.trim();
    name.is_empty() || !name_listed(disabled, name)
}

pub(crate) fn name_listed(list: &[String], name: &str) -> bool {
    !name.is_empty() && list.iter().any(|d| d == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn roots() {
        let (ws, user) = skill_roots();
        assert_eq!(
            ws,
            [
                ".cursor/skills",
                ".agents/skills",
                ".claude/skills",
                ".codex/skills"
            ]
        );
        assert_eq!(
            user,
            [
                ".cursor/skills",
                ".agents/skills",
                ".claude/skills",
                ".codex/skills",
                ".recur/recur-skills"
            ]
        );
    }

    #[test]
    fn effective_enabled() {
        let global = list(&["noisy", "go-code-review"]);
        assert!(
            !effective_skill_enabled("noisy", &global, &[], &[]),
            "inherit global off"
        );
        assert!(
            effective_skill_enabled("noisy", &global, &[], &list(&["noisy"])),
            "project enable wins"
        );
        assert!(
            !effective_skill_enabled("create-skill", &global, &list(&["create-skill"]), &[]),
            "project disable wins"
        );
        assert!(
            !effective_skill_enabled("noisy", &global, &list(&["noisy"]), &list(&["noisy"])),
            "project disable beats project enable"
        );
        assert!(effective_skill_enabled("  ", &global, &list(&[""]), &[]));
        assert!(
            !effective_skill_enabled(" noisy ", &global, &[], &[]),
            "names are trimmed"
        );
    }
}
