//! Real-GitHub checks that the
//! snapshot keeps Copilot work events (REST timeline) and a timing
//! comparison of snapshot strategies. Run with
//! `RECUR_GH_BENCH=1 [RECUR_GH_BENCH_PR=owner/repo#N] cargo test -p recur-forge -- --ignored live_`.

use std::path::Path;
use std::time::{Duration, Instant};

use chrono::Utc;
use serde::Deserialize;

use super::snapshot::{SNAPSHOT_QUERY, finish, parse_snapshot_graphql};
use super::*;
use crate::json::decode;

fn parse_bench_pr(spec: &str) -> Result<(String, String, i64), String> {
    let spec = spec.trim();
    let bad = || "want owner/repo#N".to_owned();
    let hash = spec.rfind('#').filter(|&h| h > 0).ok_or_else(bad)?;
    let slash = spec
        .find('/')
        .filter(|&s| s > 0 && s <= hash)
        .ok_or_else(bad)?;
    let (owner, repo) = (&spec[..slash], &spec[slash + 1..hash]);
    let n: i64 = spec[hash + 1..].parse().map_err(|_| bad())?;
    if owner.is_empty() || repo.is_empty() || n <= 0 {
        return Err(bad());
    }
    Ok((owner.to_owned(), repo.to_owned(), n))
}

#[test]
fn bench_pr_spec() {
    assert_eq!(
        parse_bench_pr("cli/cli#14347"),
        Ok(("cli".into(), "cli".into(), 14347))
    );
    assert!(parse_bench_pr("nope").is_err());
}

/// The bench PR, or `None` (skip) without `RECUR_GH_BENCH=1` and gh.
async fn live_bench_pr(gh: &GitHub) -> Option<(String, String, i64)> {
    if std::env::var("RECUR_GH_BENCH").ok().as_deref() != Some("1") {
        eprintln!("set RECUR_GH_BENCH=1 to compare GraphQL vs parallel gh snapshot fetches");
        return None;
    }
    if !gh.cli_available().await {
        eprintln!("gh not available/authenticated");
        return None;
    }
    let mut spec = std::env::var("RECUR_GH_BENCH_PR")
        .unwrap_or_default()
        .trim()
        .to_owned();
    if spec.is_empty() {
        spec = "cli/cli#14347".into();
    }
    Some(parse_bench_pr(&spec).unwrap_or_else(|e| panic!("parse_bench_pr({spec:?}) = {e}")))
}

fn gql(query: &str, owner: &str, repo: &str, n: i64) -> Vec<String> {
    graphql_args(query, owner, repo, n)
}

#[tokio::test]
#[ignore = "live GitHub: set RECUR_GH_BENCH=1"]
async fn live_graphql_omits_copilot_work_events() {
    let gh = GitHub::new();
    let Some((owner, repo, number)) = live_bench_pr(&gh).await else {
        return;
    };
    let cwd = Path::new("");
    let schema = gh
        .gh
        .run(
            cwd,
            &[
                "api",
                "graphql",
                "-f",
                "query={ started: __type(name:\"CopilotWorkStartedEvent\") { name }\n\t\t   finished: __type(name:\"CopilotWorkFinishedEvent\") { name }\n\t\t   enum: __type(name:\"PullRequestTimelineItemsItemType\") { enumValues { name } }\n\t\t   union: __type(name:\"PullRequestTimelineItems\") { possibleTypes { name } } }",
            ],
        )
        .await
        .expect("schema probe");
    let env: serde_json::Value = serde_json::from_str(&schema).expect("schema json");
    let d = &env["data"];
    assert!(
        d["started"].is_null(),
        "__type(CopilotWorkStartedEvent) = {}, want null",
        d["started"]
    );
    assert!(
        d["finished"].is_null(),
        "__type(CopilotWorkFinishedEvent) = {}, want null",
        d["finished"]
    );
    for v in d["enum"]["enumValues"].as_array().into_iter().flatten() {
        let name = v["name"].as_str().unwrap_or_default();
        assert!(
            !name.to_uppercase().contains("COPILOT"),
            "PullRequestTimelineItemsItemType includes {name}"
        );
    }
    for v in d["union"]["possibleTypes"].as_array().into_iter().flatten() {
        let name = v["name"].as_str().unwrap_or_default();
        assert!(
            !name.to_lowercase().contains("copilot"),
            "PullRequestTimelineItems includes {name}"
        );
    }

    let tl = gh
        .gh
        .run(
            cwd,
            &[
                "api".to_owned(),
                format!("repos/{owner}/{repo}/issues/{number}/timeline?per_page=100"),
                "-H".to_owned(),
                "Accept: application/vnd.github+json".to_owned(),
            ],
        )
        .await
        .expect("REST timeline");
    #[derive(Default, Deserialize)]
    #[serde(default)]
    struct Rest {
        event: String,
        node_id: String,
    }
    let rest: Vec<Rest> = decode(&tl).expect("REST timeline json");
    let copilot: Vec<&Rest> = rest
        .iter()
        .filter(|e| e.event.starts_with("copilot_work"))
        .collect();
    if copilot.is_empty() {
        eprintln!("{owner}/{repo}#{number} REST timeline has no copilot_work_* events");
        return;
    }

    let items = gh
        .gh
        .run(
            cwd,
            &gql(
                "query($o:String!,$r:String!,$n:Int!){\n\t\t  repository(owner:$o,name:$r){ pullRequest(number:$n){\n\t\t    timelineItems(first:100){ totalCount filteredCount nodes { __typename } } } } }",
                &owner,
                &repo,
                number,
            ),
        )
        .await
        .expect("GraphQL timeline");
    let items: serde_json::Value = serde_json::from_str(&items).expect("GraphQL timeline json");
    let items = &items["data"]["repository"]["pullRequest"]["timelineItems"];
    for n in items["nodes"].as_array().into_iter().flatten() {
        let t = n["__typename"].as_str().unwrap_or_default();
        assert!(
            !t.to_lowercase().contains("copilot"),
            "timelineItems node __typename = {t}"
        );
    }
    let (total, filtered) = (
        items["totalCount"].as_i64().unwrap_or(0),
        items["filteredCount"].as_i64().unwrap_or(0),
    );
    assert!(
        filtered < total,
        "filteredCount={filtered} totalCount={total}, want hidden Copilot items"
    );
    assert!(total - filtered >= copilot.len() as i64);

    let out = gh
        .gh
        .run(cwd, &gql(SNAPSHOT_QUERY, &owner, &repo, number))
        .await
        .expect("hybrid snapshot");
    let snap = finish(
        parse_snapshot_graphql(&out, Utc::now()).expect("parse snapshot"),
        &tl,
        Utc::now(),
    );
    let got = snap
        .comments
        .iter()
        .filter(|c| c.body_md.contains("on behalf of"))
        .count();
    assert_eq!(got, copilot.len(), "hybrid snapshot Copilot actions");

    let node = &copilot[0].node_id;
    if !node.is_empty() {
        let res = gh
            .gh
            .run(
                cwd,
                &[
                    "api",
                    "graphql",
                    "-f",
                    "query=query($id:ID!){ node(id:$id) { __typename } }",
                    "-F",
                    &format!("id={node}"),
                ],
            )
            .await;
        assert!(
            res.is_err(),
            "node({node}) succeeded, want a GraphQL error (CWSE ids are not public)"
        );
    }
}

struct Fetch {
    elapsed: Duration,
    bytes: usize,
    cost: i64,
    err: Option<String>,
}

async fn fetch_graphql(gh: &GitHub, owner: &str, repo: &str, n: i64) -> Fetch {
    let start = Instant::now();
    let res = gh
        .gh
        .run(Path::new(""), &gql(SNAPSHOT_QUERY, owner, repo, n))
        .await;
    let elapsed = start.elapsed();
    let out = match res {
        Ok(out) => out,
        Err(e) => {
            return Fetch {
                elapsed,
                bytes: 0,
                cost: 0,
                err: Some(e.to_string()),
            };
        }
    };
    let mut f = Fetch {
        elapsed,
        bytes: out.len(),
        cost: 0,
        err: None,
    };
    if let Err(e) = parse_snapshot_graphql(&out, Utc::now()) {
        f.err = Some(e);
        return f;
    }
    let v: serde_json::Value = serde_json::from_str(&out).unwrap_or_default();
    if v["errors"].as_array().is_some_and(|e| !e.is_empty()) {
        f.err = Some(format!("graphql errors: {}", v["errors"]));
    }
    f.cost = v["data"]["rateLimit"]["cost"].as_i64().unwrap_or(0);
    f
}

async fn fetch_hybrid(gh: &GitHub, owner: &str, repo: &str, n: i64) -> Fetch {
    let start = Instant::now();
    let tl_args = vec![
        "api".to_owned(),
        format!("repos/{owner}/{repo}/issues/{n}/timeline?per_page=100"),
        "-H".to_owned(),
        "Accept: application/vnd.github+json".to_owned(),
    ];
    let gql_args = gql(SNAPSHOT_QUERY, owner, repo, n);
    let (g, t) = tokio::join!(
        gh.gh.run(Path::new(""), &gql_args),
        gh.gh.run(Path::new(""), &tl_args)
    );
    let elapsed = start.elapsed();
    let bytes = g.as_ref().map_or(0, String::len) + t.as_ref().map_or(0, String::len);
    let fail = |e: String| Fetch {
        elapsed,
        bytes,
        cost: 0,
        err: Some(e),
    };
    let g = match g {
        Ok(g) => g,
        Err(e) => return fail(e.to_string()),
    };
    let snap = match parse_snapshot_graphql(&g, Utc::now()) {
        Ok(s) => s,
        Err(e) => return fail(e),
    };
    let t = match t {
        Ok(t) => t,
        Err(e) => return fail(e.to_string()),
    };
    if finish(snap, &t, Utc::now()).comments.is_empty() {
        return fail("hybrid snapshot comments = 0, want conversation rows".into());
    }
    Fetch {
        elapsed,
        bytes,
        cost: 0,
        err: None,
    }
}

async fn fetch_parallel(gh: &GitHub, owner: &str, repo: &str, n: i64) -> Fetch {
    let owner_repo = format!("{owner}/{repo}");
    let fields = "id,title,body,state,author,createdAt,updatedAt,reviewDecision,mergeable,mergeStateStatus,isDraft,comments,reviews,commits,url,headRefOid,headRefName,baseRefName,reviewRequests,assignees";
    let cwd = Path::new("");
    let start = Instant::now();
    let view = match gh
        .gh
        .run(
            cwd,
            &[
                "pr".to_owned(),
                "view".to_owned(),
                n.to_string(),
                "--repo".to_owned(),
                owner_repo.clone(),
                "--json".to_owned(),
                fields.to_owned(),
            ],
        )
        .await
    {
        Ok(v) => v,
        Err(e) => {
            return Fetch {
                elapsed: start.elapsed(),
                bytes: 0,
                cost: 0,
                err: Some(e.to_string()),
            };
        }
    };
    let comments_args = vec![
        "api".to_owned(),
        format!("repos/{owner_repo}/pulls/{n}/comments"),
    ];
    let timeline_args = vec![
        "api".to_owned(),
        format!("repos/{owner_repo}/issues/{n}/timeline?per_page=100"),
        "-H".to_owned(),
        "Accept: application/vnd.github+json".to_owned(),
    ];
    let (comments, threads, rollup, timeline, viewer, reactions) = tokio::join!(
        gh.gh.run(cwd, &comments_args),
        gh.review_threads(cwd, &owner_repo, n),
        gh.commit_rollup(cwd, &owner_repo, n),
        gh.gh.run(cwd, &timeline_args),
        gh.fetch_viewer(cwd, &owner_repo),
        gh.reactions(cwd, &owner_repo, n),
    );
    let mut total = view.len() + rollup.0.len() + rollup.1.len() + viewer.login.len();
    let mut first: Option<String> = None;
    let mut note = |n: usize, e: Option<String>| {
        total += n;
        if first.is_none() {
            first = e;
        }
    };
    match comments {
        Ok(c) => note(c.len(), None),
        Err(e) => note(e.out.len(), Some(e.to_string())),
    }
    match threads {
        Ok(t) => note(t.len(), None),
        Err(e) => note(0, Some(e.message)),
    }
    match timeline {
        Ok(t) => note(t.len(), None),
        Err(e) => note(e.out.len(), Some(e.to_string())),
    }
    match reactions {
        Ok((h, _)) => note(h.len(), None),
        Err(e) => note(0, Some(e.message)),
    }
    Fetch {
        elapsed: start.elapsed(),
        bytes: total,
        cost: 0,
        err: first,
    }
}

fn summarize(rs: &[Fetch]) -> (Duration, Duration) {
    let mut xs: Vec<Duration> = rs.iter().map(|r| r.elapsed).collect();
    xs.sort();
    let sum: Duration = xs.iter().sum();
    (xs[xs.len() / 2], sum / xs.len() as u32)
}

#[tokio::test]
#[ignore = "live GitHub: set RECUR_GH_BENCH=1"]
async fn live_snapshot_efficiency() {
    let gh = GitHub::new();
    let Some((owner, repo, n)) = live_bench_pr(&gh).await else {
        return;
    };
    fetch_graphql(&gh, &owner, &repo, n).await;
    fetch_hybrid(&gh, &owner, &repo, n).await;
    fetch_parallel(&gh, &owner, &repo, n).await;
    let (mut g, mut h, mut p) = (Vec::new(), Vec::new(), Vec::new());
    for _ in 0..5 {
        g.push(fetch_graphql(&gh, &owner, &repo, n).await);
        h.push(fetch_hybrid(&gh, &owner, &repo, n).await);
        p.push(fetch_parallel(&gh, &owner, &repo, n).await);
    }
    for (name, rs) in [("graphql", &g), ("hybrid", &h), ("parallel", &p)] {
        for (i, r) in rs.iter().enumerate() {
            assert!(r.err.is_none(), "{name}[{i}] error = {:?}", r.err);
        }
    }
    let (gm, gmean) = summarize(&g);
    let (hm, hmean) = summarize(&h);
    let (pm, pmean) = summarize(&p);
    eprintln!("{owner}/{repo}#{n}  n=5");
    eprintln!(
        "one GraphQL:  median {gm:?}  mean {gmean:?}  {} B  cost {}",
        g[0].bytes, g[0].cost
    );
    eprintln!(
        "GraphQL+REST: median {hm:?}  mean {hmean:?}  {} B",
        h[0].bytes
    );
    eprintln!("parallel gh:  median {pm:?}  mean {pmean:?}");
    if hm > pm {
        eprintln!("hybrid was slower on this PR (median {hm:?} vs parallel {pm:?})");
    }
}
