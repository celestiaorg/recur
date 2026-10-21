//! Top-level views and the titlebar breadcrumb (`ViewName`, `crumbFor` in `lib/stores.ts`).

/// A routable top-level view (`ViewName`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ViewName {
    Detail,
    Settings,
    Project,
    Pr,
    Issues,
    Schedules,
    PrDetail,
    Inbox,
    #[default]
    Briefing,
    Welcome,
    Escalation,
}

impl ViewName {
    pub const ALL: [ViewName; 11] = [
        ViewName::Detail,
        ViewName::Settings,
        ViewName::Project,
        ViewName::Pr,
        ViewName::Issues,
        ViewName::Schedules,
        ViewName::PrDetail,
        ViewName::Inbox,
        ViewName::Briefing,
        ViewName::Welcome,
        ViewName::Escalation,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            ViewName::Detail => "detail",
            ViewName::Settings => "settings",
            ViewName::Project => "project",
            ViewName::Pr => "pr",
            ViewName::Issues => "issues",
            ViewName::Schedules => "schedules",
            ViewName::PrDetail => "prdetail",
            ViewName::Inbox => "inbox",
            ViewName::Briefing => "briefing",
            ViewName::Welcome => "welcome",
            ViewName::Escalation => "escalation",
        }
    }

    pub fn parse(s: &str) -> Option<ViewName> {
        ViewName::ALL.into_iter().find(|v| v.as_str() == s)
    }

    /// `navKey`: the sidebar nav item that should appear active. Workspace detail highlights
    /// the tree row instead of a nav item.
    pub fn nav_key(self) -> Option<ViewName> {
        match self {
            ViewName::PrDetail => Some(ViewName::Pr),
            ViewName::Detail | ViewName::Escalation => None,
            ViewName::Inbox => Some(ViewName::Briefing),
            v => Some(v),
        }
    }
}

/// How a breadcrumb segment is styled (`.crumb b`, `.crumb .on`, `.crumb .crumb-branch`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CrumbStyle {
    /// Plain `--faint` text.
    Plain,
    /// `<b>`: `--muted`, weight 500.
    Bold,
    /// `<span class="on">`: `--text`.
    On,
    /// `<b class="on">`: `--text`, weight 500.
    BoldOn,
    /// `<span class="crumb-branch">`: the bordered branch chip.
    Branch,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CrumbSeg {
    pub text: String,
    pub style: CrumbStyle,
}

/// The titlebar breadcrumb. Structured instead of the web UI's escaped HTML string, so a
/// user-provided workspace name is always text.
pub type Crumb = Vec<CrumbSeg>;

pub(crate) fn seg(text: impl Into<String>, style: CrumbStyle) -> CrumbSeg {
    CrumbSeg {
        text: text.into(),
        style,
    }
}

/// The crumb's plain text.
pub fn crumb_text(crumb: &[CrumbSeg]) -> String {
    crumb.iter().map(|s| s.text.as_str()).collect()
}
