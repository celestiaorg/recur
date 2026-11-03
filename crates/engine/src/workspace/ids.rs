//! Cozy-kitchen workspace ids: `{project}.{adj}-{dish}`, with `-N` on the
//! kitchen pair on collision (recur.toasty-bagel, demo.simmered-ramen-2). The
//! word lists are a contract (ids and branches already on disk use them).

use std::collections::HashSet;

use crate::Engine;

pub const KITCHEN_ADJECTIVES: [&str; 20] = [
    "toasty", "crispy", "buttery", "flaky", "golden", "zesty", "simmered", "steamy", "honeyed",
    "smoky", "herbed", "lemony", "velvety", "caramel", "pepper", "roasted", "cozy", "crusty",
    "bubbly", "savory",
];

pub const KITCHEN_DISHES: [&str; 44] = [
    // comfort
    "ramen",
    "risotto",
    "chowder",
    "stew",
    "curry",
    "lasagna",
    "potpie",
    "dumpling",
    "gratin",
    "porridge",
    "pho",
    "udon",
    "biryani",
    "goulash", // bakery
    "bagel",
    "scone",
    "brioche",
    "galette",
    "focaccia",
    "pretzel",
    "tart",
    "muffin",
    "biscuit",
    "croissant",
    "babka",
    "ciabatta", // egg / savory
    "omelet",
    "frittata",
    "hash",
    "quiche",
    "toastie",
    "tacos",
    "burrito",
    "pierogi", // sweet
    "cobbler",
    "brownie",
    "parfait",
    "pudding",
    "shortcake",
    "trifle",
    "cannoli",
    "churro",
    "macaron",
    "eclair",
];

const KITCHEN_ID_RESHUFFLES: usize = 16;

/// A uniform index below `n` from OS randomness (rejection sampling).
fn rand_index(n: usize) -> usize {
    let n = u64::try_from(n).unwrap_or(u64::MAX).max(1);
    let zone = u64::MAX - (u64::MAX % n);
    loop {
        let mut buf = [0u8; 8];
        if getrandom::fill(&mut buf).is_err() {
            return 0;
        }
        let v = u64::from_le_bytes(buf);
        if v < zone {
            return usize::try_from(v % n).unwrap_or(0);
        }
    }
}

/// A random `{adj}-{dish}` with no suffix.
pub fn kitchen_pair(adjectives: &[String], dishes: &[String]) -> String {
    let adj = &adjectives[rand_index(adjectives.len())];
    let dish = &dishes[rand_index(dishes.len())];
    format!("{adj}-{dish}")
}

/// Picks a free kitchen id: up to 16 random pairs, then one more pair with
/// the first free `-N` (N ≥ 2).
pub fn alloc_kitchen_id(
    adjectives: &[String],
    dishes: &[String],
    taken: impl Fn(&str) -> bool,
) -> String {
    for _ in 0..KITCHEN_ID_RESHUFFLES {
        let id = kitchen_pair(adjectives, dishes);
        if !taken(&id) {
            return id;
        }
    }
    let base = kitchen_pair(adjectives, dishes);
    if !taken(&base) {
        return base;
    }
    let mut n = 2;
    loop {
        let id = format!("{base}-{n}");
        if !taken(&id) {
            return id;
        }
        n += 1;
    }
}

/// `{project}.{kitchen}`; an empty project returns the kitchen id.
pub fn compose_workspace_id(project_id: &str, kitchen_slug: &str) -> String {
    let project_id = project_id.trim();
    let kitchen_slug = kitchen_slug.trim();
    if project_id.is_empty() {
        return kitchen_slug.to_owned();
    }
    if kitchen_slug.is_empty() {
        return project_id.to_owned();
    }
    format!("{project_id}.{kitchen_slug}")
}

/// A project RepoPath canonicalized for same-repo slug claims: `~`
/// expanded, cleaned, absolute, symlinks resolved when the path exists
/// (macOS /tmp → /private/tmp).
pub fn repo_key(p: &str) -> String {
    let p = clean(&recur_git::expand_home(p.trim()));
    if p.is_empty() || p == "." {
        return p;
    }
    let abs = if std::path::Path::new(&p).is_absolute() {
        p
    } else {
        match std::env::current_dir() {
            Ok(cwd) => clean(&format!("{}/{p}", cwd.to_string_lossy())),
            Err(_) => return p,
        }
    };
    match std::fs::canonicalize(&abs) {
        Ok(r) => r.to_string_lossy().into_owned(),
        Err(_) => abs,
    }
}

/// Lexically cleans a slash path (`.`, `..` and doubled slashes).
fn clean(p: &str) -> String {
    if p.is_empty() {
        return ".".into();
    }
    let rooted = p.starts_with('/');
    let mut out: Vec<&str> = Vec::new();
    for part in p.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                if out.last().is_some_and(|l| *l != "..") {
                    out.pop();
                } else if !rooted {
                    out.push("..");
                }
            }
            s => out.push(s),
        }
    }
    let joined = out.join("/");
    match (rooted, joined.is_empty()) {
        (true, _) => format!("/{joined}"),
        (false, true) => ".".into(),
        (false, false) => joined,
    }
}

impl Engine {
    fn kitchen_words(&self) -> (Vec<String>, Vec<String>) {
        if let Some(w) = self.hooks.lock().kitchen_words.clone() {
            return w;
        }
        (
            KITCHEN_ADJECTIVES.iter().map(|s| (*s).to_owned()).collect(),
            KITCHEN_DISHES.iter().map(|s| (*s).to_owned()).collect(),
        )
    }

    /// A free `{project}.{kitchen}` id (serialized so two creates cannot
    /// claim the same pair before either is stored). With `mgr`, local
    /// branches under the prefix count as taken too.
    pub async fn new_workspace_id(
        &self,
        project_id: &str,
        mgr: Option<&recur_git::Manager>,
    ) -> String {
        let _g = self.id_mu.lock().await;
        self.mint_workspace_id(project_id, mgr).await
    }

    /// Mints an id. The caller holds `id_mu`.
    pub(crate) async fn mint_workspace_id(
        &self,
        project_id: &str,
        mgr: Option<&recur_git::Manager>,
    ) -> String {
        let repo_path = self
            .store
            .project(project_id)
            .map(|p| repo_key(&p.repo_path))
            .unwrap_or_default();
        let mut taken_branch: HashSet<String> = HashSet::new();
        let mut prefix = self.branch_name_prefix();
        if let Some(mgr) = mgr {
            prefix = mgr.prefix();
            let listed = tokio::time::timeout(
                std::time::Duration::from_secs(10),
                mgr.list_local_branches(),
            )
            .await;
            match listed {
                Ok(Ok(names)) => {
                    let cut = if prefix.is_empty() {
                        String::new()
                    } else {
                        format!("{prefix}/")
                    };
                    for name in names {
                        if cut.is_empty() {
                            taken_branch.insert(name);
                        } else if let Some(slug) = name.strip_prefix(&cut) {
                            taken_branch.insert(slug.to_owned());
                        }
                    }
                }
                Ok(Err(e)) => tracing::warn!(
                    "recur: listing local branches while allocating workspace id: {e}"
                ),
                Err(_) => tracing::warn!(
                    "recur: listing local branches while allocating workspace id: context deadline exceeded"
                ),
            }
        }
        self.mark_same_repo_kitchen_taken(&repo_path, &prefix, &mut taken_branch);
        let (adjectives, dishes) = self.kitchen_words();
        let kitchen = alloc_kitchen_id(&adjectives, &dishes, |slug| {
            let id = compose_workspace_id(project_id, slug);
            self.store.workspace(&id).is_some()
                || self.is_deleting(&id)
                || taken_branch.contains(slug)
        });
        compose_workspace_id(project_id, &kitchen)
    }

    /// Slugs already claimed by a worktree workspace on the same checkout
    /// (main working copies and PR chat hosts do not use prefix/slug).
    fn mark_same_repo_kitchen_taken(
        &self,
        repo_path: &str,
        prefix: &str,
        taken: &mut HashSet<String>,
    ) {
        if repo_path.is_empty() || repo_path == "." {
            return;
        }
        let cut = if prefix.is_empty() {
            String::new()
        } else {
            format!("{prefix}/")
        };
        for ws in self.store.workspaces() {
            if ws.is_main || ws.is_pr_chat_host() {
                continue;
            }
            let Some(p) = self.store.project(&ws.project_id) else {
                continue;
            };
            if repo_key(&p.repo_path) != repo_path {
                continue;
            }
            let slug = recur_git::workspace_slug(&ws.id);
            if !slug.is_empty() {
                taken.insert(slug);
            }
            if prefix.is_empty() {
                let b = ws.branch.trim();
                if !b.is_empty() {
                    taken.insert(b.to_owned());
                }
            } else if let Some(slug) = ws.branch.strip_prefix(&cut)
                && !slug.is_empty()
            {
                taken.insert(slug.to_owned());
            }
        }
    }
}
