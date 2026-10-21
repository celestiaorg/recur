//! GitHub reaction rollups: parse, apply to thread rows, add/remove.

use std::collections::HashMap;
use std::path::Path;

use recur_domain::{
    Pr, PrComment, PrReaction, graphql_reaction_content, normalize_reaction_content, sort_reactions,
};
use serde::Deserialize;

use super::GitHub;
use super::repo::split_owner_repo;
use crate::ForgeError;
use crate::json::decode;

/// Reactions for one reactable node and the thread row ids it answers to.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReactionHit {
    pub ids: Vec<String>,
    pub node_id: String,
    pub reactions: Vec<PrReaction>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct Group {
    content: String,
    viewer_has_reacted: bool,
    reactors: Count,
    users: Count,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct Count {
    total_count: i64,
}

impl Group {
    fn count(&self) -> i64 {
        if self.reactors.total_count > 0 {
            self.reactors.total_count
        } else {
            self.users.total_count
        }
    }
}

fn parse_groups(groups: &[Group]) -> Vec<PrReaction> {
    let out: Vec<PrReaction> = groups
        .iter()
        .filter_map(|g| {
            let content = normalize_reaction_content(&g.content)?;
            let count = g.count();
            (count > 0).then(|| PrReaction {
                content: content.to_owned(),
                count,
                mine: g.viewer_has_reacted,
            })
        })
        .collect();
    sort_reactions(&out)
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct Reactable {
    id: String,
    database_id: i64,
    reaction_groups: Vec<Group>,
}

fn hit(n: &Reactable, prefix: &str) -> ReactionHit {
    let id = n.id.trim();
    let mut h = ReactionHit {
        node_id: id.to_owned(),
        reactions: parse_groups(&n.reaction_groups),
        ids: Vec::new(),
    };
    if !id.is_empty() {
        h.ids.push(format!("{prefix}{id}"));
    }
    if n.database_id > 0 {
        h.ids.push(format!("{prefix}{}", n.database_id));
    }
    h
}

/// Go `parseGitHubReactions`: hits for the description, comments, reviews
/// and line notes, plus the PR node id.
pub(crate) fn parse_reactions(raw: &str) -> Result<(Vec<ReactionHit>, String), String> {
    #[derive(Default, Deserialize)]
    #[serde(default)]
    struct Payload {
        data: Data,
    }
    #[derive(Default, Deserialize)]
    #[serde(default)]
    struct Data {
        repository: Repo,
    }
    #[derive(Default, Deserialize)]
    #[serde(default, rename_all = "camelCase")]
    struct Repo {
        pull_request: Pull,
    }
    #[derive(Default, Deserialize)]
    #[serde(default)]
    struct Nodes<T> {
        nodes: Vec<T>,
    }
    #[derive(Default, Deserialize)]
    #[serde(default, rename_all = "camelCase")]
    struct Pull {
        id: String,
        reaction_groups: Vec<Group>,
        comments: Nodes<Reactable>,
        reviews: Nodes<Reactable>,
        review_threads: Nodes<Thread>,
    }
    #[derive(Default, Deserialize)]
    #[serde(default)]
    struct Thread {
        comments: Nodes<Reactable>,
    }
    let raw = raw.trim();
    if raw.is_empty() {
        return Err("empty reaction graphql".into());
    }
    let p: Payload = decode(raw).map_err(|e| format!("parse reaction graphql: {e}"))?;
    let pr = p.data.repository.pull_request;
    let pr_node = pr.id.trim().to_owned();
    let mut hits = Vec::new();
    if !pr_node.is_empty() {
        hits.push(ReactionHit {
            ids: vec!["gh-description".to_owned()],
            node_id: pr_node.clone(),
            reactions: parse_groups(&pr.reaction_groups),
        });
    }
    hits.extend(pr.comments.nodes.iter().map(|n| hit(n, "gh-comment-")));
    for n in &pr.reviews.nodes {
        let mut h = hit(n, "gh-review-");
        let id = n.id.trim();
        if !id.is_empty() {
            h.ids.push(format!("gh-review-action-{id}"));
        }
        if n.database_id > 0 {
            h.ids.push(format!("gh-review-action-{}", n.database_id));
        }
        hits.push(h);
    }
    for th in &pr.review_threads.nodes {
        hits.extend(th.comments.nodes.iter().map(|n| hit(n, "gh-inline-")));
    }
    Ok((hits, pr_node))
}

/// Stamps reactions and node ids onto the rows each hit names.
pub(crate) fn apply_reactions(cs: &mut [PrComment], hits: &[ReactionHit]) {
    if hits.is_empty() {
        return;
    }
    let mut by_id: HashMap<&str, &ReactionHit> = HashMap::new();
    for h in hits {
        for id in &h.ids {
            if !id.is_empty() {
                by_id.insert(id, h);
            }
        }
    }
    for c in cs {
        if let Some(h) = by_id.get(c.id.as_str()) {
            c.reactions = h.reactions.clone();
            if !h.node_id.is_empty() {
                c.node_id = h.node_id.clone();
            }
        }
    }
}

/// The GraphQL node to react on for a thread row, or `None` (REST-only ids
/// cannot take reactions).
pub(crate) fn reaction_subject_id(c: &PrComment, pr: &Pr) -> Option<String> {
    let id = c.node_id.trim();
    if !id.is_empty() {
        return Some(id.to_owned());
    }
    if c.id == "gh-description" || c.kind == "description" {
        let n = pr.node_id.trim();
        return (!n.is_empty()).then(|| n.to_owned());
    }
    let id = c.id.trim();
    for p in [
        "gh-comment-",
        "gh-review-action-",
        "gh-review-",
        "gh-inline-",
    ] {
        let Some(rest) = id.strip_prefix(p) else {
            continue;
        };
        if rest.is_empty() || rest.parse::<i64>().is_ok() {
            return None;
        }
        return Some(rest.to_owned());
    }
    None
}

const REACTIONS_QUERY: &str = "query($o:String!,$r:String!,$n:Int!){repository(owner:$o,name:$r){pullRequest(number:$n){id reactionGroups{content viewerHasReacted reactors{totalCount}}comments(last:100){nodes{id databaseId reactionGroups{content viewerHasReacted reactors{totalCount}}}}reviews(last:100){nodes{id databaseId reactionGroups{content viewerHasReacted reactors{totalCount}}}}reviewThreads(first:100){nodes{comments(first:50){nodes{id databaseId reactionGroups{content viewerHasReacted reactors{totalCount}}}}}}}}}";

impl GitHub {
    /// Go `fetchGitHubReactions`: every reaction rollup on a PR.
    pub async fn reactions(
        &self,
        repo: &Path,
        owner_repo: &str,
        number: i64,
    ) -> Result<(Vec<ReactionHit>, String), ForgeError> {
        let Some((owner, name)) = split_owner_repo(owner_repo).filter(|_| number > 0) else {
            return Err(ForgeError::invalid("invalid repo for reactions"));
        };
        let out = self
            .gh
            .run(
                repo,
                &[
                    "api".to_owned(),
                    "graphql".to_owned(),
                    "-f".to_owned(),
                    format!("query={REACTIONS_QUERY}"),
                    "-F".to_owned(),
                    format!("o={owner}"),
                    "-F".to_owned(),
                    format!("r={name}"),
                    "-F".to_owned(),
                    format!("n={number}"),
                ],
            )
            .await?;
        parse_reactions(&out).map_err(ForgeError::failed)
    }

    /// Go `setGitHubReaction`.
    pub(crate) async fn set_reaction_on(
        &self,
        repo: &Path,
        subject: &str,
        content: &str,
        add: bool,
    ) -> Result<(), ForgeError> {
        let subject = subject.trim();
        let gql = graphql_reaction_content(content);
        let (false, Some(gql)) = (subject.is_empty(), gql) else {
            return Err(ForgeError::invalid("invalid reaction subject"));
        };
        let mutation = if add { "addReaction" } else { "removeReaction" };
        let query = format!(
            "mutation($id:ID!,$c:ReactionContent!){{{mutation}(input:{{subjectId:$id,content:$c}}){{reaction{{content}}}}}}"
        );
        let out = self
            .gh
            .run(
                repo,
                &[
                    "api".to_owned(),
                    "graphql".to_owned(),
                    "-f".to_owned(),
                    format!("query={query}"),
                    "-F".to_owned(),
                    format!("id={subject}"),
                    "-F".to_owned(),
                    format!("c={gql}"),
                ],
            )
            .await?;
        #[derive(Default, Deserialize)]
        #[serde(default)]
        struct Errors {
            errors: Vec<super::snapshot::GqlError>,
        }
        if let Ok(p) = decode::<Errors>(&out)
            && let Some(e) = p.errors.first()
        {
            let msg = e.message.trim();
            return Err(ForgeError::failed(if msg.is_empty() {
                "GitHub reaction failed"
            } else {
                msg
            }));
        }
        Ok(())
    }
}
