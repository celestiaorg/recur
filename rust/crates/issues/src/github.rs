//! GitHub Issues through `gh search issues`: assigned to the user, plus
//! unassigned issues in every GitHub repo a project has as a remote.

use std::collections::BTreeMap;
use std::path::Path;

use async_trait::async_trait;
use futures::future::join_all;
use recur_domain::{IssueRef, PrParticipant, Project, Settings};
use recur_forge::Gh;
use recur_forge::github::owner_repo_from_remote_url;
use recur_forge::json::decode;
use serde::Deserialize;

use crate::{AssignedIssue, IssueError, IssueLabel, IssueQuery, IssueTracker};

const SEARCH_FIELDS: &str =
    "id,number,title,url,createdAt,updatedAt,repository,labels,commentsCount,assignees";
/// GitHub search caps a query at 256 characters, so `--repo` filters go in
/// batches of this many.
const REPO_BATCH: usize = 5;

pub struct GitHubIssues {
    gh: Gh,
    git: String,
}

impl Default for GitHubIssues {
    fn default() -> Self {
        Self::new()
    }
}

impl GitHubIssues {
    pub fn new() -> Self {
        Self::with_gh(Gh::default())
    }

    pub fn with_gh(gh: Gh) -> Self {
        Self {
            gh,
            git: "git".into(),
        }
    }

    /// Go `githubIssues`: one `gh search issues` call; nothing on failure.
    async fn search(&self, filter: &[String]) -> Vec<AssignedIssue> {
        if !self.gh.installed() {
            return Vec::new();
        }
        let mut args: Vec<String> = [
            "search",
            "issues",
            "--state",
            "open",
            "--limit",
            "100",
            "--json",
            SEARCH_FIELDS,
        ]
        .map(str::to_owned)
        .to_vec();
        args.extend(filter.iter().cloned());
        match self.gh.stdout(Path::new(""), &args).await {
            Ok(raw) => parse_search(&raw),
            Err(_) => Vec::new(),
        }
    }

    /// Go `githubRemoteRepos`: lower-cased `owner/repo` of every remote
    /// (origin, upstream, …) → the first project that has it, so fork
    /// checkouts still claim upstream issues.
    pub async fn remote_repos(&self, projects: &[Project]) -> BTreeMap<String, String> {
        let mut out = BTreeMap::new();
        for p in projects {
            if p.repo_path.is_empty() {
                continue;
            }
            let Ok(raw) =
                recur_forge::plain_stdout(&self.git, &["-C", &p.repo_path, "remote", "-v"]).await
            else {
                continue;
            };
            for line in raw.split('\n') {
                let f: Vec<&str> = line.split_whitespace().collect();
                if f.len() < 2 {
                    continue;
                }
                if let Some(repo) = owner_repo_from_remote_url(f[1]) {
                    out.entry(repo.to_lowercase())
                        .or_insert_with(|| p.id.clone());
                }
            }
        }
        out
    }
}

#[derive(Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct SearchRow {
    id: String,
    number: i64,
    title: String,
    url: String,
    created_at: String,
    updated_at: String,
    comments_count: i64,
    labels: Option<Vec<IssueLabel>>,
    assignees: Vec<Login>,
    repository: Repo,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct Login {
    login: String,
}

#[derive(Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct Repo {
    name_with_owner: String,
}

fn parse_search(raw: &str) -> Vec<AssignedIssue> {
    let Ok(rows) = decode::<Vec<SearchRow>>(raw) else {
        return Vec::new();
    };
    rows.into_iter()
        .map(|r| {
            let people: Vec<PrParticipant> = r
                .assignees
                .into_iter()
                .map(|a| PrParticipant {
                    login: a.login,
                    ..Default::default()
                })
                .collect();
            AssignedIssue {
                source: "github".into(),
                id: r.id,
                r#ref: format!("{}#{}", r.repository.name_with_owner, r.number),
                title: r.title,
                url: r.url,
                created_at: r.created_at,
                updated_at: r.updated_at,
                group: "todo".into(),
                state: "Open".into(),
                project: r.repository.name_with_owner,
                labels: r.labels,
                comments: r.comments_count,
                assignees: (!people.is_empty()).then_some(people),
                ..Default::default()
            }
        })
        .collect()
}

/// Sets `project_id` on GitHub issues from the remote map.
pub fn link_projects(issues: &mut [AssignedIssue], repos: &BTreeMap<String, String>) {
    for it in issues.iter_mut().filter(|i| i.source == "github") {
        it.project_id = repos
            .get(&it.project.to_lowercase())
            .cloned()
            .unwrap_or_default();
    }
}

#[async_trait]
impl IssueTracker for GitHubIssues {
    fn id(&self) -> &str {
        "github"
    }

    fn label(&self) -> &str {
        "GitHub"
    }

    fn configured(&self, _settings: &Settings) -> bool {
        self.gh.installed()
    }

    async fn list_open(&self, q: IssueQuery<'_>) -> Vec<AssignedIssue> {
        let Ok(repos) = tokio::time::timeout_at(q.deadline, self.remote_repos(q.projects)).await
        else {
            return Vec::new();
        };
        let mut filters = vec![vec!["--assignee".to_owned(), "@me".to_owned()]];
        let names: Vec<&String> = repos.keys().collect();
        for batch in names.chunks(REPO_BATCH) {
            let mut args = vec!["--no-assignee".to_owned()];
            for r in batch {
                args.extend(["--repo".to_owned(), (*r).clone()]);
            }
            filters.push(args);
        }
        let lists = join_all(filters.iter().map(|f| async move {
            tokio::time::timeout_at(q.deadline, self.search(f))
                .await
                .unwrap_or_default()
        }))
        .await;
        let mut out: Vec<AssignedIssue> = lists.into_iter().flatten().collect();
        link_projects(&mut out, &repos);
        out
    }

    /// Go `claimGitHubIssue`: `gh issue edit --add-assignee @me` unless the
    /// issue already has an assignee.
    async fn claim(&self, issue: &IssueRef, _settings: &Settings) -> Result<(), IssueError> {
        let key = issue.key.as_str();
        let Some(i) = key.rfind('#').filter(|&i| i > 0) else {
            return Err(IssueError(format!("issue key {key:?} has no owner/repo")));
        };
        let (repo, num) = (&key[..i], &key[i + 1..]);
        let raw = self
            .gh
            .stdout(
                Path::new(""),
                &["issue", "view", num, "--repo", repo, "--json", "assignees"],
            )
            .await
            .map_err(IssueError)?;
        #[derive(Default, Deserialize)]
        #[serde(default)]
        struct View {
            assignees: Vec<serde_json::Value>,
        }
        let v: View = serde_json::from_str(&raw).map_err(|e| IssueError(e.to_string()))?;
        if !v.assignees.is_empty() {
            return Ok(());
        }
        self.gh
            .stdout(
                Path::new(""),
                &[
                    "issue",
                    "edit",
                    num,
                    "--repo",
                    repo,
                    "--add-assignee",
                    "@me",
                ],
            )
            .await
            .map(drop)
            .map_err(IssueError)
    }

    /// `owner/repo#N` → the issue page.
    fn issue_url(&self, issue: &IssueRef) -> Option<String> {
        let key = issue.key.as_str();
        let (repo, num) = key.rsplit_once('#')?;
        let (owner, name) = repo.split_once('/')?;
        let ok = !owner.is_empty()
            && !owner.contains(['/', ' ', '\t', '\n', '#'])
            && !name.is_empty()
            && !name.contains([' ', '\t', '\n', '#'])
            && !num.is_empty()
            && num.bytes().all(|b| b.is_ascii_digit());
        ok.then(|| format!("https://github.com/{repo}/issues/{num}"))
    }
}

#[cfg(test)]
mod tests {
    use std::process::Command;

    use super::*;

    fn write_gh(dir: &Path, body: &str) -> Gh {
        let path = dir.join("gh");
        std::fs::write(&path, body).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        Gh::new(path)
    }

    #[test]
    fn link_projects_by_remote() {
        let repos = BTreeMap::from([("botfun/eden".to_owned(), "botfun".to_owned())]);
        let mut issues = vec![
            AssignedIssue {
                source: "github".into(),
                project: "BotFun/Eden".into(),
                ..Default::default()
            },
            AssignedIssue {
                source: "github".into(),
                project: "other/repo".into(),
                ..Default::default()
            },
        ];
        link_projects(&mut issues, &repos);
        assert_eq!(issues[0].project_id, "botfun");
        assert_eq!(issues[1].project_id, "");
    }

    #[test]
    fn search_rows_map_to_issues() {
        let got = parse_search(
            r#"[{"id":"I_1","number":12,"title":"Bug","url":"https://github.com/a/b/issues/12","createdAt":"c","updatedAt":"u",
            "repository":{"name":"b","nameWithOwner":"a/b"},"labels":[{"name":"bug","color":"d73a4a","id":"L"}],"commentsCount":3,
            "assignees":[{"login":"sam"}]},{"id":"I_2","number":1,"repository":{"nameWithOwner":"a/b"},"assignees":[]}]"#,
        );
        assert_eq!(got.len(), 2);
        assert_eq!(
            (
                got[0].r#ref.as_str(),
                got[0].group.as_str(),
                got[0].state.as_str()
            ),
            ("a/b#12", "todo", "Open")
        );
        assert_eq!(got[0].labels.as_ref().unwrap()[0].color, "d73a4a");
        assert_eq!(got[0].assignees.as_ref().unwrap()[0].login, "sam");
        assert_eq!(got[0].comments, 3);
        assert!(got[1].assignees.is_none() && got[1].labels.is_none());
        assert!(parse_search("nope").is_empty());
    }

    #[test]
    fn issue_urls() {
        let gh = GitHubIssues::new();
        let r = |key: &str| IssueRef {
            source: "github".into(),
            id: "x".into(),
            key: key.into(),
        };
        assert_eq!(
            gh.issue_url(&r("acme/app#12")).as_deref(),
            Some("https://github.com/acme/app/issues/12")
        );
        assert_eq!(gh.issue_url(&r("acme#12")), None);
        assert_eq!(gh.issue_url(&r("acme/app#x")), None);
    }

    #[tokio::test]
    async fn lists_assigned_and_unassigned_batches() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        for args in [
            vec!["init", "-q"],
            vec!["remote", "add", "origin", "git@github.com:me/fork.git"],
            vec![
                "remote",
                "add",
                "upstream",
                "https://github.com/Acme/App.git",
            ],
        ] {
            assert!(
                Command::new("git")
                    .args(&args)
                    .current_dir(&repo)
                    .status()
                    .unwrap()
                    .success()
            );
        }
        let gh = write_gh(
            dir.path(),
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"$0.args\"\ncase \"$*\" in\n  *--assignee*) echo '[{\"id\":\"A\",\"number\":1,\"updatedAt\":\"2026-08-02\",\"repository\":{\"nameWithOwner\":\"Acme/App\"}}]' ;;\n  *) echo '[{\"id\":\"B\",\"number\":2,\"updatedAt\":\"2026-08-01\",\"repository\":{\"nameWithOwner\":\"me/fork\"}}]' ;;\nesac\n",
        );
        let tracker = GitHubIssues::with_gh(gh);
        let projects = vec![Project {
            id: "p1".into(),
            repo_path: repo.to_string_lossy().into_owned(),
            ..Default::default()
        }];
        let settings = Settings::default();
        let got = tracker
            .list_open(IssueQuery {
                settings: &settings,
                projects: &projects,
                deadline: tokio::time::Instant::now() + std::time::Duration::from_secs(15),
            })
            .await;
        assert_eq!(got.len(), 2);
        assert!(got.iter().all(|i| i.project_id == "p1"));
        let log = std::fs::read_to_string(dir.path().join("gh.args")).unwrap();
        assert!(log.contains("--assignee @me"));
        assert!(
            log.contains("--no-assignee --repo acme/app --repo me/fork"),
            "{log}"
        );
    }

    #[tokio::test]
    async fn claim_assigns_only_unowned_issues() {
        let dir = tempfile::tempdir().unwrap();
        let gh = write_gh(
            dir.path(),
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"$0.args\"\nif [ \"$2\" = \"view\" ]; then\n  case \"$*\" in *a/b*) echo '{\"assignees\":[]}' ;; *) echo '{\"assignees\":[{\"login\":\"x\"}]}' ;; esac\nfi\n",
        );
        let tracker = GitHubIssues::with_gh(gh);
        let settings = Settings::default();
        let r = |key: &str| IssueRef {
            source: "github".into(),
            id: "I".into(),
            key: key.into(),
        };
        tracker.claim(&r("a/b#3"), &settings).await.unwrap();
        tracker.claim(&r("c/d#4"), &settings).await.unwrap();
        let log = std::fs::read_to_string(dir.path().join("gh.args")).unwrap();
        assert!(log.contains("issue edit 3 --repo a/b --add-assignee @me"));
        assert!(!log.contains("issue edit 4"));
        assert_eq!(
            tracker.claim(&r("#5"), &settings).await.unwrap_err().0,
            "issue key \"#5\" has no owner/repo"
        );
    }
}
