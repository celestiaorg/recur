//! The Settings nav: sections, their groups, labels, icons and search keywords, the header
//! titles, and the search filter.

use recur_ui::IconName;

/// `SettingsSection`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Section {
    Agent,
    Models,
    Loops,
    Prs,
    Integrations,
    General,
    Appearance,
    Editor,
    Skills,
    Mcp,
    Data,
    Licenses,
    BundleCommands,
    BundlePrompts,
    BundleGrammars,
    BundleLsp,
    BundleThemes,
    BundleSnippets,
}

impl Section {
    pub const ALL: [Section; 18] = [
        Section::Agent,
        Section::Models,
        Section::Loops,
        Section::Prs,
        Section::Integrations,
        Section::General,
        Section::Appearance,
        Section::Editor,
        Section::Skills,
        Section::Mcp,
        Section::Data,
        Section::Licenses,
        Section::BundleCommands,
        Section::BundlePrompts,
        Section::BundleGrammars,
        Section::BundleLsp,
        Section::BundleThemes,
        Section::BundleSnippets,
    ];

    pub fn id(self) -> &'static str {
        match self {
            Section::Agent => "agent",
            Section::Models => "models",
            Section::Loops => "loops",
            Section::Prs => "prs",
            Section::Integrations => "integrations",
            Section::General => "general",
            Section::Appearance => "appearance",
            Section::Editor => "editor",
            Section::Skills => "skills",
            Section::Mcp => "mcp",
            Section::Data => "data",
            Section::Licenses => "licenses",
            Section::BundleCommands => "bundle-commands",
            Section::BundlePrompts => "bundle-prompts",
            Section::BundleGrammars => "bundle-grammars",
            Section::BundleLsp => "bundle-lsp",
            Section::BundleThemes => "bundle-themes",
            Section::BundleSnippets => "bundle-snippets",
        }
    }

    /// The pane `AppState::settings_section` names (unknown ids show General's title-less
    /// body, as an unknown store value would render no pane).
    pub fn from_id(id: &str) -> Option<Section> {
        Section::ALL.into_iter().find(|s| s.id() == id)
    }

    /// `titles`: the header over the pane.
    pub fn title(self) -> &'static str {
        match self {
            Section::Agent => "Providers",
            Section::Models => "Models",
            Section::Loops => "Loops",
            Section::Prs => "Pull requests",
            Section::Integrations => "Integrations",
            Section::Mcp => "MCP",
            Section::General => "General",
            Section::Appearance => "Appearance",
            Section::Editor => "Code editor",
            Section::Skills => "Skills",
            Section::Data => "Data",
            Section::Licenses => "Licenses",
            Section::BundleCommands => "Commands",
            Section::BundlePrompts => "Prompt actions",
            Section::BundleGrammars => "Grammars",
            Section::BundleLsp => "Language servers",
            Section::BundleThemes => "Themes",
            Section::BundleSnippets => "Snippets",
        }
    }
}

/// One nav row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NavItem {
    pub id: Section,
    pub label: &'static str,
    pub icon: IconName,
    pub keywords: &'static str,
}

/// One nav group (`.set-group` heading and its rows).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NavGroup {
    pub group: &'static str,
    pub items: Vec<NavItem>,
}

const fn item(id: Section, label: &'static str, icon: IconName, keywords: &'static str) -> NavItem {
    NavItem {
        id,
        label,
        icon,
        keywords,
    }
}

/// `nav`, with `navIcons` resolved.
pub fn nav() -> Vec<NavGroup> {
    vec![
        NavGroup {
            group: "App",
            items: vec![
                item(
                    Section::General,
                    "General",
                    IconName::SlidersHorizontal,
                    "general simplify default yagni stdlib sleep caffeinate awake power prevent idle mac agent turn timeout minutes limit deadline concurrent loops agents slots cap governor branch prefix git workspace name github schedule keep last",
                ),
                item(
                    Section::Appearance,
                    "Appearance",
                    IconName::Monitor,
                    "theme color palette chrome app editor theme dark light charcoal slate cool glow catppuccin mocha latte paper nord terminal xterm ansi solarized vga tango campbell homebrew snazzy ubuntu native title bar window linux zoom scroll wheel mouse",
                ),
                item(
                    Section::Editor,
                    "Code editor",
                    IconName::Code,
                    "editor code font size face wrap tab indent spaces go python typescript keymap vim emacs whitespace caret ruler panel left right files scm source control outline problems search refs explorer exclude glob hidden max open tabs gutter diff hints terminal",
                ),
                item(
                    Section::Skills,
                    "Skills",
                    IconName::Slash,
                    "skills agent skill slash disable enable claude cursor codex recur",
                ),
                item(
                    Section::Data,
                    "Data",
                    IconName::Trash,
                    "clear wipe reset delete logs log reveal finder",
                ),
                item(
                    Section::Licenses,
                    "Licenses",
                    IconName::Info,
                    "license licenses legal open source third party dependencies notices copyright elastic apache",
                ),
            ],
        },
        NavGroup {
            group: "Agent",
            items: vec![
                item(
                    Section::Agent,
                    "Providers",
                    IconName::Plug,
                    "providers account claude cursor codex login auth permissions bypass bash enable disable off usage quota",
                ),
                item(
                    Section::Models,
                    "Models",
                    IconName::Cpu,
                    "orchestrator coder review opus sonnet haiku fable flags effort context child workspace inherit parent",
                ),
            ],
        },
        NavGroup {
            group: "Loops",
            items: vec![item(
                Section::Loops,
                "Loops",
                IconName::RefreshCcw,
                "loop policy cautious balanced aggressive default rails autonomy guardrails",
            )],
        },
        NavGroup {
            group: "Workflow",
            items: vec![
                item(
                    Section::Prs,
                    "Pull requests",
                    IconName::GitPullRequest,
                    "merge squash rebase method pr pull request strategy commit message title description",
                ),
                item(
                    Section::Integrations,
                    "Integrations",
                    IconName::Hash,
                    "linear api key github issues tracker ticket workspace kickoff jira bundle setting",
                ),
                item(
                    Section::Mcp,
                    "MCP",
                    IconName::Plug,
                    "mcp model context protocol linear tools http stdio server agent backlog triage",
                ),
            ],
        },
        NavGroup {
            group: "Bundles",
            items: vec![
                item(
                    Section::BundleCommands,
                    "Commands",
                    IconName::Command,
                    "bundles commands macros willsave format on save palette text source encode json go rust disable enable",
                ),
                item(
                    Section::BundlePrompts,
                    "Prompt actions",
                    IconName::Prompt,
                    "bundles prompts slash chat commit review simplify-review simplify-audit fix-findings fix-checks update open-pr open-draft-pr linear issue disable enable",
                ),
                item(
                    Section::BundleGrammars,
                    "Grammars",
                    IconName::Braces,
                    "bundles grammars syntax highlighting language file types go typescript python disable enable",
                ),
                item(
                    Section::BundleLsp,
                    "Language servers",
                    IconName::Braces,
                    "bundles language server lsp gopls pyright ruff rust-analyzer taplo marksman disable enable",
                ),
                item(
                    Section::BundleThemes,
                    "Themes",
                    IconName::Palette,
                    "bundles themes editor colour color charcoal paper cool glow syntax disable enable",
                ),
                item(
                    Section::BundleSnippets,
                    "Snippets",
                    IconName::Code,
                    "bundles snippets tab trigger wrap html go python javascript shell disable enable",
                ),
            ],
        },
    ]
}

/// `filteredNav`: rows whose label (case-insensitive) or keywords contain the trimmed,
/// lowercased query; empty groups drop out.
pub fn filtered_nav(query: &str) -> Vec<NavGroup> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return nav();
    }
    nav()
        .into_iter()
        .map(|g| NavGroup {
            group: g.group,
            items: g
                .items
                .into_iter()
                .filter(|i| i.label.to_lowercase().contains(&q) || i.keywords.contains(&q))
                .collect(),
        })
        .filter(|g| !g.items.is_empty())
        .collect()
}

/// The section the nav lands on when the filter hides the current one (the first match).
pub fn section_after_filter(groups: &[NavGroup], current: &str) -> Option<Section> {
    let ids: Vec<Section> = groups
        .iter()
        .flat_map(|g| g.items.iter().map(|i| i.id))
        .collect();
    let first = *ids.first()?;
    (!ids.iter().any(|s| s.id() == current)).then_some(first)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_section_is_in_the_nav_once() {
        let ids: Vec<Section> = nav()
            .iter()
            .flat_map(|g| g.items.iter().map(|i| i.id))
            .collect();
        assert_eq!(ids.len(), Section::ALL.len());
        for s in Section::ALL {
            assert_eq!(ids.iter().filter(|x| **x == s).count(), 1, "{s:?}");
            assert_eq!(Section::from_id(s.id()), Some(s));
        }
    }

    #[test]
    fn filter_matches_labels_and_keywords() {
        let g = filtered_nav("  VIM ");
        assert_eq!(g.len(), 1);
        assert_eq!(g[0].items[0].id, Section::Editor);
        let g = filtered_nav("linear");
        let ids: Vec<Section> = g
            .iter()
            .flat_map(|g| g.items.iter().map(|i| i.id))
            .collect();
        assert_eq!(
            ids,
            [Section::Integrations, Section::Mcp, Section::BundlePrompts]
        );
        assert!(filtered_nav("zzz-nothing").is_empty());
        assert_eq!(filtered_nav("").len(), 5);
        // A label match is case-insensitive; keywords are already lowercase.
        assert_eq!(
            filtered_nav("Pull")[0].items[0].id,
            Section::Prs,
            "label match"
        );
    }

    #[test]
    fn filter_moves_off_a_hidden_section() {
        let g = filtered_nav("vim");
        assert_eq!(section_after_filter(&g, "general"), Some(Section::Editor));
        assert_eq!(section_after_filter(&g, "editor"), None);
        assert_eq!(section_after_filter(&[], "general"), None);
    }
}
