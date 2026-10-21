//! How PRs land (merge strategy, commit message formats) and how branches
//! catch up with their base. Each resolves project override, then the global
//! default, then a hardcoded fallback; invalid values at any tier are skipped
//! so they never shadow a valid lower tier.

use crate::{Project, Settings};

string_enum! {
    /// The strategy used to integrate a PR branch into its base, matching
    /// GitHub's three options and `gh pr merge` flags (D53).
    MergeMethod {
        /// Create a merge commit (--no-ff / --merge).
        Merge = "merge",
        /// Squash and merge (--squash).
        Squash = "squash",
        /// Rebase and merge (--rebase).
        Rebase = "rebase",
    }
}

string_enum! {
    /// How a PR branch catches up with its base (D54). Independent of
    /// [`MergeMethod`] (how a PR lands onto the base).
    UpdateBaseMethod {
        /// git merge <base> into the worktree.
        Merge = "merge",
        /// git rebase <base> in the worktree.
        Rebase = "rebase",
    }
}

string_enum! {
    /// The squash-merge commit message format (D65), matching GitHub's repo
    /// setting minus "Default message".
    SquashCommitMessage {
        Title = "title",
        TitleAndCommits = "title-and-commits",
        TitleAndDescription = "title-and-description",
    }
}

string_enum! {
    /// The merge-commit message format (D65), matching GitHub's repo setting
    /// minus "Default message".
    MergeCommitMessage {
        Title = "title",
        TitleAndDescription = "title-and-description",
    }
}

impl MergeMethod {
    /// A human-readable label.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Squash => "Squash and merge",
            Self::Rebase => "Rebase and merge",
            _ => "Merge",
        }
    }
}

impl UpdateBaseMethod {
    /// A short human-readable label.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Rebase => "rebase",
            _ => "merge",
        }
    }
}

/// A valid [`MergeMethod`], falling back to merge.
pub fn normalize_merge_method(s: &str) -> MergeMethod {
    match MergeMethod::from(s) {
        MergeMethod::Other(_) => MergeMethod::Merge,
        m => m,
    }
}

/// Project override, then global default, then a merge commit.
pub fn effective_merge_method(proj: &Project, settings: &Settings) -> MergeMethod {
    [&proj.merge_method, &settings.default_merge_method]
        .into_iter()
        .find(|m| m.is_known())
        .cloned()
        .unwrap_or(MergeMethod::Merge)
}

/// Reports whether method is allowed on this repo. An empty list means
/// unknown (not fetched): every method is permitted.
pub fn merge_method_permitted(allowed: &[String], m: &MergeMethod) -> bool {
    allowed.is_empty() || (m.is_known() && allowed.iter().any(|a| a == m.as_str()))
}

/// A valid [`UpdateBaseMethod`], falling back to merge.
pub fn normalize_update_base_method(s: &str) -> UpdateBaseMethod {
    match UpdateBaseMethod::from(s) {
        UpdateBaseMethod::Other(_) => UpdateBaseMethod::Merge,
        m => m,
    }
}

/// Project override, then global default, then merge.
pub fn effective_update_base_method(proj: &Project, settings: &Settings) -> UpdateBaseMethod {
    [
        &proj.update_base_method,
        &settings.default_update_base_method,
    ]
    .into_iter()
    .find(|m| m.is_known())
    .cloned()
    .unwrap_or(UpdateBaseMethod::Merge)
}

/// A valid squash message format, falling back to title-and-commits (the
/// Recur default).
pub fn normalize_squash_commit_message(s: &str) -> SquashCommitMessage {
    match SquashCommitMessage::from(s) {
        SquashCommitMessage::Other(_) => SquashCommitMessage::TitleAndCommits,
        m => m,
    }
}

/// Project override, then global default, then title-and-commits.
pub fn effective_squash_commit_message(proj: &Project, settings: &Settings) -> SquashCommitMessage {
    [
        &proj.squash_commit_message,
        &settings.default_squash_commit_message,
    ]
    .into_iter()
    .find(|m| m.is_known())
    .cloned()
    .unwrap_or(SquashCommitMessage::TitleAndCommits)
}

/// A valid merge-commit message format, falling back to title (the Recur
/// default).
pub fn normalize_merge_commit_message(s: &str) -> MergeCommitMessage {
    match MergeCommitMessage::from(s) {
        MergeCommitMessage::Other(_) => MergeCommitMessage::Title,
        m => m,
    }
}

/// Project override, then global default, then title.
pub fn effective_merge_commit_message(proj: &Project, settings: &Settings) -> MergeCommitMessage {
    [
        &proj.merge_commit_message,
        &settings.default_merge_commit_message,
    ]
    .into_iter()
    .find(|m| m.is_known())
    .cloned()
    .unwrap_or(MergeCommitMessage::Title)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proj(f: impl FnOnce(&mut Project)) -> Project {
        let mut p = Project::default();
        f(&mut p);
        p
    }

    fn set(f: impl FnOnce(&mut Settings)) -> Settings {
        let mut s = Settings::default();
        f(&mut s);
        s
    }

    #[test]
    fn normalize_merge_method_cases() {
        for (input, want) in [
            ("merge", MergeMethod::Merge),
            ("squash", MergeMethod::Squash),
            ("rebase", MergeMethod::Rebase),
            ("", MergeMethod::Merge),
            ("bogus", MergeMethod::Merge),
        ] {
            assert_eq!(normalize_merge_method(input), want, "{input:?}");
        }
    }

    #[test]
    fn effective_merge_method_cases() {
        let cases = [
            (
                "both empty → merge",
                Project::default(),
                Settings::default(),
                MergeMethod::Merge,
            ),
            (
                "global set",
                Project::default(),
                set(|s| s.default_merge_method = MergeMethod::Squash),
                MergeMethod::Squash,
            ),
            (
                "project overrides global",
                proj(|p| p.merge_method = MergeMethod::Rebase),
                set(|s| s.default_merge_method = MergeMethod::Squash),
                MergeMethod::Rebase,
            ),
            (
                "project empty inherits global",
                Project::default(),
                set(|s| s.default_merge_method = MergeMethod::Rebase),
                MergeMethod::Rebase,
            ),
            (
                "invalid project falls back to global",
                proj(|p| p.merge_method = "bad".into()),
                set(|s| s.default_merge_method = MergeMethod::Squash),
                MergeMethod::Squash,
            ),
            (
                "invalid both falls back to merge",
                proj(|p| p.merge_method = "bad".into()),
                set(|s| s.default_merge_method = "bad".into()),
                MergeMethod::Merge,
            ),
        ];
        for (name, p, s, want) in cases {
            assert_eq!(effective_merge_method(&p, &s), want, "{name}");
        }
    }

    #[test]
    fn merge_method_permitted_cases() {
        assert!(
            merge_method_permitted(&[], &MergeMethod::Rebase),
            "empty allowed permits all"
        );
        let squash = ["squash".to_string()];
        assert!(merge_method_permitted(&squash, &MergeMethod::Squash));
        assert!(!merge_method_permitted(&squash, &MergeMethod::Merge));
        let bogus = ["bogus".to_string()];
        assert!(!merge_method_permitted(&bogus, &"bogus".into()));
    }

    #[test]
    fn merge_method_label() {
        assert_eq!(MergeMethod::Merge.label(), "Merge");
        assert_eq!(MergeMethod::Squash.label(), "Squash and merge");
        assert_eq!(MergeMethod::Rebase.label(), "Rebase and merge");
        assert_eq!(MergeMethod::default().label(), "Merge");
    }

    #[test]
    fn normalize_update_base_method_cases() {
        for (input, want) in [
            ("merge", UpdateBaseMethod::Merge),
            ("rebase", UpdateBaseMethod::Rebase),
            ("", UpdateBaseMethod::Merge),
            ("squash", UpdateBaseMethod::Merge), // squash is not valid for update-base
            ("bogus", UpdateBaseMethod::Merge),
        ] {
            assert_eq!(normalize_update_base_method(input), want, "{input:?}");
        }
    }

    #[test]
    fn effective_update_base_method_cases() {
        let cases = [
            (
                "both empty → merge",
                Project::default(),
                Settings::default(),
                UpdateBaseMethod::Merge,
            ),
            (
                "global set",
                Project::default(),
                set(|s| s.default_update_base_method = UpdateBaseMethod::Rebase),
                UpdateBaseMethod::Rebase,
            ),
            (
                "project overrides global",
                proj(|p| p.update_base_method = UpdateBaseMethod::Merge),
                set(|s| s.default_update_base_method = UpdateBaseMethod::Rebase),
                UpdateBaseMethod::Merge,
            ),
            (
                "project empty inherits global",
                Project::default(),
                set(|s| s.default_update_base_method = UpdateBaseMethod::Rebase),
                UpdateBaseMethod::Rebase,
            ),
            (
                "invalid project falls back to global",
                proj(|p| p.update_base_method = "bad".into()),
                set(|s| s.default_update_base_method = UpdateBaseMethod::Rebase),
                UpdateBaseMethod::Rebase,
            ),
            (
                "invalid both falls back to merge",
                proj(|p| p.update_base_method = "bad".into()),
                set(|s| s.default_update_base_method = "bad".into()),
                UpdateBaseMethod::Merge,
            ),
            (
                "independent of MergeMethod",
                proj(|p| p.merge_method = MergeMethod::Squash),
                set(|s| s.default_merge_method = MergeMethod::Squash),
                UpdateBaseMethod::Merge,
            ),
        ];
        for (name, p, s, want) in cases {
            assert_eq!(effective_update_base_method(&p, &s), want, "{name}");
        }
    }

    #[test]
    fn update_base_method_label() {
        assert_eq!(UpdateBaseMethod::Merge.label(), "merge");
        assert_eq!(UpdateBaseMethod::Rebase.label(), "rebase");
    }

    #[test]
    fn normalize_squash_commit_message_cases() {
        for (input, want) in [
            ("title", SquashCommitMessage::Title),
            ("title-and-commits", SquashCommitMessage::TitleAndCommits),
            (
                "title-and-description",
                SquashCommitMessage::TitleAndDescription,
            ),
            ("", SquashCommitMessage::TitleAndCommits),
            ("bogus", SquashCommitMessage::TitleAndCommits),
        ] {
            assert_eq!(normalize_squash_commit_message(input), want, "{input:?}");
        }
    }

    #[test]
    fn effective_squash_commit_message_cases() {
        use SquashCommitMessage as M;
        let cases = [
            (
                "both empty → title-and-commits",
                Project::default(),
                Settings::default(),
                M::TitleAndCommits,
            ),
            (
                "global set",
                Project::default(),
                set(|s| s.default_squash_commit_message = M::Title),
                M::Title,
            ),
            (
                "project overrides global",
                proj(|p| p.squash_commit_message = M::TitleAndDescription),
                set(|s| s.default_squash_commit_message = M::Title),
                M::TitleAndDescription,
            ),
            (
                "project empty inherits global",
                Project::default(),
                set(|s| s.default_squash_commit_message = M::TitleAndDescription),
                M::TitleAndDescription,
            ),
            (
                "invalid project falls back to global",
                proj(|p| p.squash_commit_message = "bad".into()),
                set(|s| s.default_squash_commit_message = M::Title),
                M::Title,
            ),
            (
                "invalid both falls back to title-and-commits",
                proj(|p| p.squash_commit_message = "bad".into()),
                set(|s| s.default_squash_commit_message = "bad".into()),
                M::TitleAndCommits,
            ),
        ];
        for (name, p, s, want) in cases {
            assert_eq!(effective_squash_commit_message(&p, &s), want, "{name}");
        }
    }

    #[test]
    fn normalize_merge_commit_message_cases() {
        for (input, want) in [
            ("title", MergeCommitMessage::Title),
            (
                "title-and-description",
                MergeCommitMessage::TitleAndDescription,
            ),
            ("", MergeCommitMessage::Title),
            ("title-and-commits", MergeCommitMessage::Title), // squash-only value is invalid here
            ("bogus", MergeCommitMessage::Title),
        ] {
            assert_eq!(normalize_merge_commit_message(input), want, "{input:?}");
        }
    }

    #[test]
    fn effective_merge_commit_message_cases() {
        use MergeCommitMessage as M;
        let cases = [
            (
                "both empty → title",
                Project::default(),
                Settings::default(),
                M::Title,
            ),
            (
                "global set",
                Project::default(),
                set(|s| s.default_merge_commit_message = M::TitleAndDescription),
                M::TitleAndDescription,
            ),
            (
                "project overrides global",
                proj(|p| p.merge_commit_message = M::Title),
                set(|s| s.default_merge_commit_message = M::TitleAndDescription),
                M::Title,
            ),
            (
                "project empty inherits global",
                Project::default(),
                set(|s| s.default_merge_commit_message = M::TitleAndDescription),
                M::TitleAndDescription,
            ),
            (
                "invalid project falls back to global",
                proj(|p| p.merge_commit_message = "bad".into()),
                set(|s| s.default_merge_commit_message = M::TitleAndDescription),
                M::TitleAndDescription,
            ),
            (
                "invalid both falls back to title",
                proj(|p| p.merge_commit_message = "bad".into()),
                set(|s| s.default_merge_commit_message = "bad".into()),
                M::Title,
            ),
        ];
        for (name, p, s, want) in cases {
            assert_eq!(effective_merge_commit_message(&p, &s), want, "{name}");
        }
    }
}
