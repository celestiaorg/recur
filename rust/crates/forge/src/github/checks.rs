//! PR checks: the GitHub Checks tab rollup (check runs, status contexts,
//! queued workflows, and required checks that have not reported), from
//! GraphQL with a `gh pr checks` fallback.

use std::collections::HashSet;
use std::path::Path;

use chrono::{DateTime, Utc};
use recur_domain::Check;
use serde::Deserialize;

use super::GitHub;
use super::snapshot::GqlError;
use crate::json::{decode, decode_first};
use crate::model::{
    BUCKET_CANCEL, BUCKET_FAIL, BUCKET_PASS, BUCKET_PENDING, BUCKET_SKIPPING, all_checks_passed,
    first_non_empty, format_check_duration, format_time, parse_time,
};
use crate::{ChecksReport, ForgeError};

pub const CHECKS_QUERY: &str = r#"query($o:String!,$r:String!,$n:Int!){
  repository(owner:$o,name:$r){
    defaultBranchRef { name }
    rulesets(first:20){
      pageInfo { hasNextPage }
      nodes{
        enforcement
        target
        conditions { refName { include exclude } }
        rules(first:40){
          pageInfo { hasNextPage }
          nodes{
            type
            parameters {
              ... on RequiredStatusChecksParameters {
                requiredStatusChecks { context }
              }
            }
          }
        }
      }
    }
    pullRequest(number:$n){
      url
      baseRef {
        name
        branchProtectionRule { requiredStatusCheckContexts }
      }
      commits(last:1){
        nodes{
          commit{
            statusCheckRollup{
              contexts(first:100){
                pageInfo { hasNextPage }
                nodes{
                  __typename
                  ... on CheckRun {
                    name
                    status
                    conclusion
                    detailsUrl
                    permalink
                    startedAt
                    completedAt
                    checkSuite { workflowRun { workflow { name } } }
                  }
                  ... on StatusContext {
                    context
                    state
                    targetUrl
                    description
                    createdAt
                  }
                }
              }
            }
            checkSuites(first:30){
              pageInfo { hasNextPage }
              nodes{
                status
                conclusion
                workflowRun { url workflow { name } }
                checkRuns(first:1){ totalCount }
              }
            }
          }
        }
      }
    }
  }
}"#;

/// One row of `gh pr checks --json`, also built from GraphQL contexts.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct CheckRow {
    pub name: String,
    pub state: String,
    pub bucket: String,
    pub workflow: String,
    pub description: String,
    pub started_at: String,
    pub completed_at: String,
    pub link: String,
}

/// Go `bucketFromGHState`: a GitHub check state or conclusion as a bucket.
pub(crate) fn bucket_from_state(state: &str) -> &'static str {
    match state.trim().to_uppercase().as_str() {
        "SUCCESS" | "NEUTRAL" => BUCKET_PASS,
        "SKIPPED" => BUCKET_SKIPPING,
        "PENDING" | "QUEUED" | "IN_PROGRESS" | "WAITING" | "REQUESTED" | "EXPECTED" => {
            BUCKET_PENDING
        }
        "STALE" | "CANCELLED" | "CANCELED" => BUCKET_CANCEL,
        _ if state.is_empty() => BUCKET_PENDING,
        _ => BUCKET_FAIL,
    }
}

/// Go `parseGitHubPRChecksAt`: `gh pr checks --json` output. Empty output
/// or "no checks reported" is a clean rollup.
pub(crate) fn parse_cli_checks(
    raw: &str,
    now: DateTime<Utc>,
) -> Result<(Vec<Check>, bool), String> {
    let mut raw = raw.trim();
    if raw.is_empty() || raw.to_lowercase().contains("no checks reported") {
        return Ok((Vec::new(), true));
    }
    if let Some(i) = raw.find('[')
        && let Some(j) = raw.rfind(']')
        && j > i
    {
        raw = &raw[i..=j];
    }
    let rows: Vec<CheckRow> = decode(raw).map_err(|e| format!("parse gh pr checks json: {e}"))?;
    let checks: Vec<Check> = rows.iter().map(|r| check_from_row(r, now)).collect();
    let ok = all_checks_passed(&checks);
    Ok((checks, ok))
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Payload {
    data: Data,
    errors: Vec<GqlError>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Data {
    repository: Repo,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct PageInfo {
    has_next_page: bool,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct Paged<T> {
    page_info: PageInfo,
    nodes: Vec<T>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Named {
    name: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct Repo {
    default_branch_ref: Option<Named>,
    rulesets: Paged<Ruleset>,
    pull_request: Pull,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct Ruleset {
    enforcement: String,
    target: String,
    conditions: Conditions,
    rules: Paged<Rule>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct Conditions {
    ref_name: RefName,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RefName {
    include: Vec<String>,
    exclude: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Rule {
    #[serde(rename = "type")]
    kind: String,
    parameters: Option<RuleParams>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct RuleParams {
    required_status_checks: Vec<RequiredCheck>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RequiredCheck {
    context: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct Pull {
    url: String,
    base_ref: Option<BaseRef>,
    commits: CommitNodes,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct BaseRef {
    name: String,
    branch_protection_rule: Option<Protection>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct Protection {
    required_status_check_contexts: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct CommitNodes {
    nodes: Vec<CommitNode>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct CommitNode {
    commit: CommitBody,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct CommitBody {
    status_check_rollup: Option<Rollup>,
    check_suites: Paged<Suite>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Rollup {
    contexts: Paged<Context>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct Context {
    #[serde(rename = "__typename")]
    type_name: String,
    name: String,
    status: String,
    conclusion: String,
    details_url: String,
    permalink: String,
    started_at: String,
    completed_at: String,
    check_suite: ContextSuite,
    context: String,
    state: String,
    target_url: String,
    description: String,
    created_at: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct ContextSuite {
    workflow_run: WorkflowRun,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct WorkflowRun {
    url: String,
    workflow: Named,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct Suite {
    status: String,
    conclusion: String,
    workflow_run: Option<WorkflowRun>,
    check_runs: TotalCount,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct TotalCount {
    total_count: i64,
}

/// Go `parseGitHubPRChecksGraphQLAt`. A rollup that does not fit one page is
/// never green.
pub(crate) fn parse_graphql_checks(
    raw: &str,
    now: DateTime<Utc>,
) -> Result<(Vec<Check>, bool), String> {
    let mut raw = raw.trim();
    if raw.is_empty() {
        return Err("empty github checks graphql".into());
    }
    if let Some(i) = raw.find('{')
        && i > 0
    {
        raw = &raw[i..];
    }
    let payload: Payload =
        decode_first(raw).map_err(|e| format!("parse github checks graphql: {e}"))?;
    let first_error = || {
        payload
            .errors
            .first()
            .map(|e| format!("github checks graphql: {}", e.message))
    };
    let pr = &payload.data.repository.pull_request;
    if pr.url.trim().is_empty() {
        return Err(
            first_error().unwrap_or_else(|| "github checks graphql: missing pull request".into())
        );
    }
    let Some(commit) = pr.commits.nodes.first().map(|n| &n.commit) else {
        return match first_error() {
            Some(e) => Err(e),
            None => Ok((Vec::new(), true)),
        };
    };
    let pr_url = pr.url.trim();
    let mut rows: Vec<CheckRow> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    if let Some(rollup) = &commit.status_check_rollup {
        for n in &rollup.contexts.nodes {
            let row = row_from_context(n);
            if row.name.trim().is_empty() {
                continue;
            }
            seen.insert(row.name.clone());
            rows.push(row);
        }
    }
    for suite in &commit.check_suites.nodes {
        if suite.check_runs.total_count > 0 {
            continue;
        }
        let bucket = bucket_from_state(first_non_empty(&suite.conclusion, &suite.status));
        if bucket != BUCKET_PENDING {
            continue;
        }
        let name = suite
            .workflow_run
            .as_ref()
            .map(|w| w.workflow.name.trim().to_owned())
            .unwrap_or_default();
        // GitHub App suites with no workflow (e.g. "Depot Code Access") are not
        // Checks-tab rows. Only queued Actions workflows get a placeholder.
        if name.is_empty() || !seen.insert(name.clone()) {
            continue;
        }
        rows.push(CheckRow {
            name,
            state: suite.status.clone(),
            bucket: bucket.to_owned(),
            ..Default::default()
        });
    }
    for name in required_check_names(&payload) {
        if !seen.insert(name.clone()) {
            continue;
        }
        rows.push(CheckRow {
            name,
            state: "EXPECTED".into(),
            bucket: BUCKET_PENDING.into(),
            ..Default::default()
        });
    }
    if rows.is_empty()
        && let Some(e) = first_error()
    {
        return Err(e);
    }
    let checks: Vec<Check> = rows.iter().map(|r| check_from_row(r, now)).collect();
    let ok = all_checks_passed(&checks) && !graphql_truncated(&payload);
    Ok((apply_checks_tab_urls(checks, pr_url), ok))
}

fn graphql_truncated(p: &Payload) -> bool {
    let repo = &p.data.repository;
    if repo.rulesets.page_info.has_next_page
        || repo
            .rulesets
            .nodes
            .iter()
            .any(|rs| rs.rules.page_info.has_next_page)
    {
        return true;
    }
    let Some(n) = repo.pull_request.commits.nodes.first() else {
        return false;
    };
    n.commit
        .status_check_rollup
        .as_ref()
        .is_some_and(|r| r.contexts.page_info.has_next_page)
        || n.commit.check_suites.page_info.has_next_page
}

fn row_from_context(n: &Context) -> CheckRow {
    if n.type_name.eq_ignore_ascii_case("StatusContext")
        || (!n.context.trim().is_empty() && n.name.is_empty())
    {
        return CheckRow {
            name: first_non_empty(n.context.trim(), "check").to_owned(),
            state: n.state.clone(),
            bucket: bucket_from_state(&n.state).to_owned(),
            description: n.description.clone(),
            started_at: n.created_at.clone(),
            link: n.target_url.trim().to_owned(),
            ..Default::default()
        };
    }
    let state = if n.status.eq_ignore_ascii_case("COMPLETED") {
        n.conclusion.clone()
    } else {
        n.status.clone()
    };
    let mut link = n.permalink.trim();
    if link.is_empty() {
        link = n.details_url.trim();
    }
    let workflow = &n.check_suite.workflow_run.workflow.name;
    CheckRow {
        name: first_non_empty(first_non_empty(n.name.trim(), workflow), "check").to_owned(),
        bucket: bucket_from_state(&state).to_owned(),
        state,
        workflow: workflow.clone(),
        description: n.description.clone(),
        started_at: n.started_at.clone(),
        completed_at: n.completed_at.clone(),
        link: link.to_owned(),
    }
}

/// Required check names from classic branch protection and active branch
/// rulesets that apply to the PR base.
fn required_check_names(p: &Payload) -> Vec<String> {
    let repo = &p.data.repository;
    let base = repo
        .pull_request
        .base_ref
        .as_ref()
        .map(|b| b.name.trim())
        .unwrap_or_default();
    let default_branch = repo
        .default_branch_ref
        .as_ref()
        .map(|b| b.name.trim())
        .unwrap_or_default();
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    let mut add = |name: &str| {
        let name = name.trim();
        if !name.is_empty() && seen.insert(name.to_owned()) {
            out.push(name.to_owned());
        }
    };
    if let Some(rule) = repo
        .pull_request
        .base_ref
        .as_ref()
        .and_then(|b| b.branch_protection_rule.as_ref())
    {
        for name in &rule.required_status_check_contexts {
            add(name);
        }
    }
    for rs in &repo.rulesets.nodes {
        if !rs.enforcement.eq_ignore_ascii_case("ACTIVE") {
            continue;
        }
        if !rs.target.is_empty() && !rs.target.eq_ignore_ascii_case("BRANCH") {
            continue;
        }
        if !ruleset_applies_to_base(
            &rs.conditions.ref_name.include,
            &rs.conditions.ref_name.exclude,
            base,
            default_branch,
        ) {
            continue;
        }
        for rule in &rs.rules.nodes {
            let Some(params) = rule
                .parameters
                .as_ref()
                .filter(|_| rule.kind.eq_ignore_ascii_case("REQUIRED_STATUS_CHECKS"))
            else {
                continue;
            };
            for c in &params.required_status_checks {
                add(&c.context);
            }
        }
    }
    out
}

/// Whether a ruleset's ref conditions select `base`.
pub(crate) fn ruleset_applies_to_base(
    include: &[String],
    exclude: &[String],
    base: &str,
    default_branch: &str,
) -> bool {
    let base = base.trim();
    if base.is_empty() {
        return false;
    }
    if exclude
        .iter()
        .any(|p| ref_condition_matches(p, base, default_branch))
    {
        return false;
    }
    include.is_empty()
        || include
            .iter()
            .any(|p| ref_condition_matches(p, base, default_branch))
}

fn ref_condition_matches(pat: &str, base: &str, default_branch: &str) -> bool {
    let pat = pat.trim();
    let base = base.trim();
    if pat.is_empty() || base.is_empty() {
        return false;
    }
    match pat.to_uppercase().as_str() {
        "~ALL" => return true,
        "~DEFAULT_BRANCH" => return base == default_branch.trim(),
        _ => {}
    }
    let full = if base.starts_with("refs/") {
        base.to_owned()
    } else {
        format!("refs/heads/{base}")
    };
    ref_glob(pat, base) || ref_glob(pat, &full)
}

/// fnmatch-style ref pattern: `*` within a segment, `**` across segments,
/// `?` one non-slash character.
pub(crate) fn ref_glob(pat: &str, name: &str) -> bool {
    if pat == name {
        return true;
    }
    let mut re = String::from("^");
    let runes: Vec<char> = pat.chars().collect();
    let mut i = 0;
    while i < runes.len() {
        match runes[i] {
            '*' if runes.get(i + 1) == Some(&'*') => {
                re.push_str(".*");
                i += 1;
            }
            '*' => re.push_str("[^/]*"),
            '?' => re.push_str("[^/]"),
            r => re.push_str(&regex::escape(&r.to_string())),
        }
        i += 1;
    }
    re.push('$');
    regex::Regex::new(&re).is_ok_and(|re| re.is_match(name))
}

/// The PR's Checks tab (`/pull/N/checks`).
pub fn checks_tab_url(pr_url: &str) -> String {
    let u = pr_url.trim().trim_end_matches('/');
    if u.is_empty() {
        return String::new();
    }
    if u.to_lowercase().ends_with("/checks") {
        return u.to_owned();
    }
    format!("{u}/checks")
}

fn is_actions_url(u: &str) -> bool {
    u.contains("/actions/runs/")
}

/// Points pending Actions rows and rows without a link at the Checks tab
/// (Actions job pages do not show queued checks).
pub(crate) fn apply_checks_tab_urls(mut checks: Vec<Check>, pr_url: &str) -> Vec<Check> {
    let tab = checks_tab_url(pr_url);
    if tab.is_empty() {
        return checks;
    }
    for c in &mut checks {
        let pending = c.status == BUCKET_PENDING;
        if c.url.is_empty() || (pending && is_actions_url(&c.url)) {
            c.url = tab.clone();
        }
    }
    checks
}

/// Go `githubCheckFromRow`.
pub(crate) fn check_from_row(row: &CheckRow, now: DateTime<Utc>) -> Check {
    let mut bucket = row.bucket.trim().to_lowercase();
    if bucket.is_empty() {
        bucket = bucket_from_state(&row.state).to_owned();
    }
    let mut name = row.name.trim().to_owned();
    if name.is_empty() {
        name = first_non_empty(&row.workflow, "check").to_owned();
    }
    Check {
        name,
        detail: check_detail(row, &bucket, now),
        passed: bucket == BUCKET_PASS,
        url: row.link.trim().to_owned(),
        started_at: parse_time(&row.started_at)
            .map(format_time)
            .unwrap_or_default(),
        status: bucket,
    }
}

fn check_detail(row: &CheckRow, bucket: &str, now: DateTime<Utc>) -> String {
    if bucket == BUCKET_SKIPPING {
        return "skipped".into();
    }
    let start = parse_time(&row.started_at);
    let end = parse_time(&row.completed_at);
    if let (Some(s), Some(e)) = (start, end)
        && e >= s
    {
        return format_check_duration(e - s);
    }
    if bucket == BUCKET_PENDING
        && let Some(s) = start
        && now >= s
    {
        return format_check_duration(now - s);
    }
    match bucket {
        BUCKET_PENDING => "pending".into(),
        BUCKET_CANCEL => "cancelled".into(),
        _ => String::new(),
    }
}

/// Go `githubGraphQLCommandError`: gh's own diagnostic (capped) over JSON
/// parse noise when the command failed.
pub(crate) fn graphql_command_error(
    out: &str,
    exec_err: Option<&str>,
    parse_err: String,
) -> String {
    let Some(exec_err) = exec_err else {
        return parse_err;
    };
    let mut msg = truncate_diag(out);
    if msg.is_empty() {
        msg = exec_err.to_owned();
    }
    format!("gh api graphql: {msg}")
}

/// Trimmed and capped at 240 bytes for logs and errors.
pub(crate) fn truncate_diag(s: &str) -> String {
    let s = s.trim();
    if s.len() > 240 {
        let mut end = 240;
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        return format!("{}…", &s[..end]);
    }
    s.to_owned()
}

impl GitHub {
    /// Go `fetchGitHubPRChecks`: GraphQL first, then `gh pr checks`.
    pub(crate) async fn fetch_checks(
        &self,
        repo: &Path,
        number: i64,
        pr_url: &str,
    ) -> Result<ChecksReport, ForgeError> {
        if number <= 0 {
            return Err(ForgeError::invalid(format!(
                "invalid GitHub PR number {number}"
            )));
        }
        if !self.cli_available().await {
            return Err(ForgeError::unavailable(
                "gh CLI is not installed or not authenticated",
            ));
        }
        match self.fetch_checks_graphql(repo, number).await {
            Ok((checks, ok)) => return Ok(ChecksReport { checks, ok }),
            Err(e) => self.log_checks_graphql(repo, number, &e),
        }
        self.fetch_checks_cli(repo, number, pr_url).await
    }

    async fn fetch_checks_graphql(
        &self,
        repo: &Path,
        number: i64,
    ) -> Result<(Vec<Check>, bool), String> {
        let (owner, name) = self.repo_owner_name(repo).await.map_err(|e| e.message)?;
        let o = self
            .gh
            .output(
                repo,
                &[
                    "api".to_owned(),
                    "graphql".to_owned(),
                    "-f".to_owned(),
                    format!("query={CHECKS_QUERY}"),
                    "-f".to_owned(),
                    format!("o={owner}"),
                    "-f".to_owned(),
                    format!("r={name}"),
                    "-F".to_owned(),
                    format!("n={number}"),
                ],
            )
            .await;
        match parse_graphql_checks(&o.out, Utc::now()) {
            Ok(v) => Ok(v),
            Err(parse_err) => {
                let exec_err = (!o.ok()).then(|| exit_error_text(&o));
                Err(graphql_command_error(
                    &o.out,
                    exec_err.as_deref(),
                    parse_err,
                ))
            }
        }
    }

    async fn fetch_checks_cli(
        &self,
        repo: &Path,
        number: i64,
        pr_url: &str,
    ) -> Result<ChecksReport, ForgeError> {
        let o = self
            .gh
            .output(
                repo,
                &[
                    "pr".to_owned(),
                    "checks".to_owned(),
                    number.to_string(),
                    "--json".to_owned(),
                    "name,state,bucket,workflow,description,startedAt,completedAt,link".to_owned(),
                ],
            )
            .await;
        match parse_cli_checks(&o.out, Utc::now()) {
            Ok((checks, ok)) => {
                let mut url = pr_url.trim().to_owned();
                if url.is_empty() {
                    url = self.pr_view_url(repo, number).await;
                }
                Ok(ChecksReport {
                    checks: apply_checks_tab_urls(checks, &url),
                    ok,
                })
            }
            Err(parse_err) if o.ok() => Err(ForgeError::failed(parse_err)),
            Err(_) => {
                let mut msg = o.out.trim().to_owned();
                if msg.is_empty() {
                    msg = exit_error_text(&o);
                }
                Err(ForgeError::failed(format!("gh pr checks {number}: {msg}")))
            }
        }
    }

    /// Logs each distinct GraphQL checks failure once per PR.
    fn log_checks_graphql(&self, repo: &Path, number: i64, err: &str) {
        let msg = truncate_diag(err);
        let key = format!("{}\0{number}\0{msg}", repo.display());
        if self.checks_logged.lock().insert(key) {
            tracing::warn!("recur: github PR checks graphql {number}: {msg}");
        }
    }
}

/// Go's `exec.ExitError` text.
fn exit_error_text(o: &crate::cli::Outcome) -> String {
    match &o.exit {
        crate::cli::Exit::Code(-1) => "signal: killed".to_owned(),
        crate::cli::Exit::Code(c) => format!("exit status {c}"),
        crate::cli::Exit::Spawn(e) => e.clone(),
        crate::cli::Exit::Success => String::new(),
    }
}
