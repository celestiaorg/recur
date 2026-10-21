//! The e2e seed fixture (`frontend/e2e/fixtures/seed.json`): a seeded
//! in-memory store's bound-service data as one JSON object, written like
//! Go's `cmd/seeddump` (`json.Encoder`, two-space indent, no HTML escaping).
//! The `seeddump` binary prints it; the UI parity harness seeds from it.

use std::collections::BTreeMap;
use std::io;

use recur_domain::{
    Agent, Escalation, FeedItem, InboxItem, Policy, Pr, Project, Resource, Settings, Workspace,
};
use serde::Serialize;
use serde_json::ser::{Formatter, PrettyFormatter};

use crate::Store;

/// Keys in Go's map order (sorted).
#[derive(Serialize)]
struct Dump {
    #[serde(rename = "agentsByLoop")]
    agents_by_loop: BTreeMap<String, Vec<Agent>>,
    #[serde(rename = "escByLoop")]
    esc_by_loop: BTreeMap<String, Escalation>,
    #[serde(rename = "feedByLoop")]
    feed_by_loop: BTreeMap<String, Option<Vec<FeedItem>>>,
    inbox: Option<Vec<InboxItem>>,
    #[serde(rename = "loopById")]
    loop_by_id: BTreeMap<String, Workspace>,
    loops: Vec<Workspace>,
    policy: Policy,
    #[serde(rename = "prById")]
    pr_by_id: BTreeMap<String, Pr>,
    projects: Option<Vec<Project>>,
    prs: Vec<Pr>,
    resource: Resource,
    settings: Settings,
}

/// The fixture JSON for st, ending in a newline. Projects are stamped
/// `isGit` (the demo repo paths are not real checkouts; e2e treats them as
/// git repos). Run with an empty `HOME` so the project.toml overlay and the
/// Codex login probe see nothing (the `seeddump` binary does that itself).
pub fn dump(st: &Store) -> String {
    let mut projects = st.projects();
    for p in projects.iter_mut().flatten() {
        p.is_git = true;
    }
    let loops = st.workspaces();
    let mut d = Dump {
        agents_by_loop: BTreeMap::new(),
        esc_by_loop: BTreeMap::new(),
        feed_by_loop: BTreeMap::new(),
        inbox: st.inbox(),
        loop_by_id: BTreeMap::new(),
        loops: loops.clone(),
        policy: st.policy(),
        pr_by_id: BTreeMap::new(),
        projects,
        prs: st.prs(),
        resource: st.resource(),
        settings: st.settings(),
    };
    for l in &loops {
        if let Some(w) = st.workspace(&l.id) {
            d.loop_by_id.insert(l.id.clone(), w);
        }
        d.feed_by_loop.insert(l.id.clone(), st.feed(&l.id));
        d.agents_by_loop.insert(l.id.clone(), st.agents(&l.id));
        if let Some(e) = st.escalation(&l.id) {
            d.esc_by_loop.insert(l.id.clone(), e);
        }
    }
    for p in st.prs() {
        if let Some(pr) = st.pr(&p.id) {
            d.pr_by_id.insert(p.id.clone(), pr);
        }
    }
    let mut out = Vec::new();
    let mut ser = serde_json::Serializer::with_formatter(
        &mut out,
        GoIndent(PrettyFormatter::with_indent(b"  ")),
    );
    // Plain domain structs: encoding cannot fail.
    if d.serialize(&mut ser).is_err() {
        return String::new();
    }
    out.push(b'\n');
    String::from_utf8(out).unwrap_or_default()
}

/// Go's indented encoder without HTML escaping still escapes U+2028 and
/// U+2029.
struct GoIndent<'a>(PrettyFormatter<'a>);

macro_rules! delegate {
    ($($name:ident($($arg:ident: $ty:ty),*);)*) => {
        $(fn $name<W: ?Sized + io::Write>(&mut self, w: &mut W $(, $arg: $ty)*) -> io::Result<()> {
            self.0.$name(w $(, $arg)*)
        })*
    };
}

impl Formatter for GoIndent<'_> {
    delegate! {
        begin_array();
        end_array();
        begin_array_value(first: bool);
        end_array_value();
        begin_object();
        end_object();
        begin_object_key(first: bool);
        end_object_key();
        begin_object_value();
        end_object_value();
    }

    fn write_string_fragment<W: ?Sized + io::Write>(
        &mut self,
        w: &mut W,
        fragment: &str,
    ) -> io::Result<()> {
        let mut start = 0;
        for (i, c) in fragment.char_indices() {
            let esc = match c {
                '\u{2028}' => "\\u2028",
                '\u{2029}' => "\\u2029",
                _ => continue,
            };
            w.write_all(&fragment.as_bytes()[start..i])?;
            w.write_all(esc.as_bytes())?;
            start = i + c.len_utf8();
        }
        w.write_all(&fragment.as_bytes()[start..])
    }
}
