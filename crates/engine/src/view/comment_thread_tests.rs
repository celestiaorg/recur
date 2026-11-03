use recur_domain::PrSource;

use super::*;

fn c(f: impl FnOnce(&mut PrComment)) -> PrComment {
    let mut c = PrComment::default();
    f(&mut c);
    c
}

fn row(old: &str, new: &str, kind: &str) -> DiffLine {
    DiffLine {
        old: old.into(),
        new: new.into(),
        r#type: kind.into(),
        ..DiffLine::default()
    }
}

fn ids(list: &[&PrComment]) -> Vec<String> {
    list.iter().map(|c| c.id.clone()).collect()
}

fn note(id: &str, path: &str, line: i64, side: &str) -> PrComment {
    c(|c| {
        c.id = id.into();
        c.path = path.into();
        c.line = line;
        c.side = side.into();
    })
}

fn gh(f: impl FnOnce(&mut Pr)) -> Pr {
    let mut p = Pr {
        source: PrSource::GitHub,
        ..Pr::default()
    };
    f(&mut p);
    p
}

#[test]
fn timeline_root_drops_description_and_nested_notes() {
    assert!(!is_timeline_root(Some(&c(
        |c| c.kind = "description".into()
    ))));
    assert!(!is_timeline_root(Some(&c(
        |c| c.parent_id = "gh-review-1".into()
    ))));
    assert!(!is_timeline_root(Some(&c(
        |c| c.in_reply_to_id = "gh-inline-1".into()
    ))));
    assert!(!is_timeline_root(Some(&c(|c| c.kind = "hidden".into()))));
    assert!(is_timeline_root(Some(&c(|c| c.kind = "comment".into()))));
    assert!(is_timeline_root(Some(&c(|c| c.kind = "action".into()))));
    assert!(!is_timeline_root(None));
}

fn at(id: &str, at: &str, f: impl FnOnce(&mut PrComment)) -> PrComment {
    c(|c| {
        c.id = id.into();
        c.at = at.into();
        f(c);
    })
}

#[test]
fn review_children_oldest_first() {
    let items = [
        at("b", "2026-08-21T11:31:00Z", |c| c.parent_id = "r1".into()),
        at("a", "2026-08-21T11:30:00Z", |c| c.parent_id = "r1".into()),
        at("c", "2026-08-21T11:30:00Z", |c| c.parent_id = "r2".into()),
        at("d", "2026-08-21T11:32:00Z", |c| {
            c.parent_id = "r1".into();
            c.in_reply_to_id = "a".into();
        }),
    ];
    assert_eq!(ids(&review_children(&items, "r1")), ["a", "b"]);
    assert!(review_children(&items, "").is_empty());
}

#[test]
fn thread_replies_for_that_note() {
    let items = [
        at("r", "2026-08-21T11:32:00Z", |c| {
            c.in_reply_to_id = "gh-inline-1".into()
        }),
        at("q", "2026-08-21T11:31:00Z", |c| {
            c.in_reply_to_id = "1".into()
        }),
        at("x", "2026-08-21T11:31:00Z", |c| {
            c.in_reply_to_id = "gh-inline-2".into()
        }),
    ];
    assert_eq!(ids(&thread_replies(&items, "gh-inline-1")), ["q", "r"]);
}

#[test]
fn line_note_detection() {
    assert!(is_line_note(Some(&c(|c| c.id = "gh-inline-1".into()))));
    assert!(is_line_note(Some(&c(|c| c.path = "a.go".into()))));
    assert!(is_line_note(Some(&c(
        |c| c.in_reply_to_id = "gh-inline-1".into()
    ))));
    assert!(!is_line_note(Some(&c(|c| c.id = "c1".into()))));
    assert!(!is_line_note(None));
}

const OAUTH: &str = "src/auth/oauth.ts";

#[test]
fn notes_on_row_match_side_and_line() {
    let hmm = note("gh-inline-1", OAUTH, 12, "RIGHT");
    let left = note("gh-inline-2", OAUTH, 12, "LEFT");
    let mut reply = note("gh-inline-3", OAUTH, 12, "RIGHT");
    reply.in_reply_to_id = "gh-inline-1".into();
    let other = note("gh-inline-4", "src/other.ts", 12, "RIGHT");

    assert!(same_comment_path(OAUTH, "oauth.ts"));
    assert!(is_line_note_root(&hmm));
    assert!(!is_line_note_root(&reply));
    let all = [hmm.clone(), left.clone(), reply, other];
    assert_eq!(
        ids(&line_notes_on_row(&all, OAUTH, &row("", "12", "add"), None)),
        ["gh-inline-1"]
    );
    let two = [hmm.clone(), left];
    assert_eq!(
        ids(&line_notes_on_row(&two, OAUTH, &row("12", "", "del"), None)),
        ["gh-inline-2"]
    );
    assert_eq!(
        ids(&line_notes_on_row(
            &[hmm],
            OAUTH,
            &row("12", "12", "ctx"),
            Some(DiffSide::Right)
        )),
        ["gh-inline-1"]
    );
}

#[test]
fn multiline_range_marks_every_row() {
    let mut span = note("gh-inline-5", OAUTH, 12, "RIGHT");
    span.start_line = 11;
    let s = std::slice::from_ref(&span);
    assert_eq!(line_note_range(&span), (11, 12));
    assert_eq!(
        ids(&line_notes_covering_row(
            s,
            OAUTH,
            &row("11", "11", "ctx"),
            None
        )),
        ["gh-inline-5"]
    );
    assert_eq!(
        ids(&line_notes_covering_row(
            s,
            OAUTH,
            &row("", "12", "add"),
            None
        )),
        ["gh-inline-5"]
    );
    assert!(line_notes_covering_row(s, OAUTH, &row("10", "10", "ctx"), None).is_empty());
    assert!(line_notes_on_row(s, OAUTH, &row("11", "11", "ctx"), None).is_empty());
    assert_eq!(
        ids(&line_notes_on_row(s, OAUTH, &row("", "12", "add"), None)),
        ["gh-inline-5"]
    );
}

#[test]
fn outdated_notes_do_not_anchor() {
    let mut stale = note("gh-inline-9", OAUTH, 12, "RIGHT");
    stale.start_line = 7;
    stale.outdated = true;
    let s = std::slice::from_ref(&stale);
    assert!(!is_line_note_root(&stale));
    assert!(line_notes_on_row(s, OAUTH, &row("", "12", "add"), None).is_empty());
    assert!(line_notes_covering_row(s, OAUTH, &row("", "12", "add"), None).is_empty());
    assert!(unresolved_line_notes_by_path(s, &[OAUTH.into()]).is_empty());
}

#[test]
fn unresolved_groups_open_roots() {
    let open = note("gh-inline-1", OAUTH, 12, "");
    let later = note("gh-inline-2", OAUTH, 4, "");
    let mut resolved = note("gh-inline-3", "src/hooks/useOAuthSession.ts", 8, "");
    resolved.resolved = true;
    let mut reply = note("gh-inline-4", OAUTH, 12, "");
    reply.in_reply_to_id = "gh-inline-1".into();
    let items = [open, later, resolved, reply];
    let by_path = unresolved_line_notes_by_path(
        &items,
        &[OAUTH.into(), "src/hooks/useOAuthSession.ts".into()],
    );
    let keys: Vec<_> = by_path.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(keys, [OAUTH]);
    assert_eq!(ids(&by_path[0].1), ["gh-inline-2", "gh-inline-1"]);
    assert_eq!(comments_for_path(&by_path, OAUTH).len(), 2);
    assert_eq!(comment_count_for_path(&by_path, "src/auth", true), 2);
    assert_eq!(
        comment_count_for_path(&by_path, "src/hooks/useOAuthSession.ts", false),
        0
    );
}

fn reply(id: &str, to: &str, body: &str) -> PrComment {
    c(|c| {
        c.id = id.into();
        c.in_reply_to_id = to.into();
        c.body_md = body.into();
    })
}

#[test]
fn pending_replies() {
    let pending = reply("c-pending-1", "gh-inline-1", "thanks");
    let echoed = reply("gh-inline-9", "1", "thanks");
    assert!(is_pending_comment(Some(&pending)));
    assert!(!is_pending_comment(Some(&echoed)));

    assert!(same_thread_reply(&pending, &echoed));
    assert!(!same_thread_reply(
        &pending,
        &reply("gh-inline-9", "1", "later")
    ));
    assert!(!same_thread_reply(
        &pending,
        &reply("gh-inline-9", "gh-inline-2", "thanks")
    ));

    let root = c(|c| {
        c.id = "gh-inline-1".into();
        c.body_md = "hmm".into();
    });
    assert_eq!(
        keep_pending_replies(&[root.clone(), pending.clone()], vec![root.clone()]),
        [root.clone(), pending.clone()]
    );
    assert_eq!(
        keep_pending_replies(
            &[root.clone(), pending.clone()],
            vec![root.clone(), echoed.clone()]
        ),
        [root.clone(), echoed.clone()]
    );

    let older = reply("gh-inline-8", "gh-inline-1", "thanks");
    let next_pending = reply("c-pending-2", "gh-inline-1", "thanks");
    assert_eq!(
        keep_pending_replies(&[older.clone(), next_pending.clone()], vec![older.clone()]),
        [older, next_pending]
    );

    let two = vec![root.clone(), pending.clone()];
    assert_eq!(
        settle_posted_reply(two.clone(), &pending.id, Some(echoed.clone())),
        [root.clone(), echoed.clone()]
    );
    assert_eq!(
        settle_posted_reply(
            vec![root.clone(), pending.clone(), echoed.clone()],
            &pending.id,
            Some(echoed.clone())
        ),
        [root.clone(), echoed]
    );
    assert_eq!(settle_posted_reply(two, &pending.id, None), [root]);
}

#[test]
fn edit_and_delete_permissions() {
    let n = c(|c| {
        c.id = "gh-inline-1".into();
        c.path = "a.ts".into();
        c.author = "Avery Rourke".into();
        c.author_login = "avery".into();
        c.body_md = "hmm".into();
    });
    let writer = gh(|p| {
        p.viewer_login = "avery".into();
        p.viewer_can_write = true;
    });
    let other = gh(|p| p.viewer_login = "pat".into());
    let write_only = gh(|p| {
        p.viewer_login = "pat".into();
        p.viewer_can_write = true;
    });
    assert_eq!(comment_source(Some(&n)), "hmm");
    assert!(can_edit_pr_comment(Some(&writer), Some(&n)));
    assert!(!can_edit_pr_comment(Some(&other), Some(&n)));
    assert!(can_delete_pr_comment(Some(&writer), Some(&n)));
    assert!(can_delete_pr_comment(Some(&write_only), Some(&n)));
    assert!(!can_delete_pr_comment(Some(&other), Some(&n)));
    assert!(is_dropped_comment(
        &reply("gh-inline-2", "gh-inline-1", ""),
        "gh-inline-1"
    ));

    let pending = c(|c| {
        c.id = "c-pending-1".into();
        c.path = "a.ts".into();
        c.author_login = "avery".into();
    });
    assert!(!can_edit_pr_comment(Some(&writer), Some(&pending)));
    assert!(!can_delete_pr_comment(Some(&writer), Some(&pending)));

    let by = |id: &str, login: &str| {
        c(|c| {
            c.id = id.into();
            c.author_login = login.into();
            c.body_md = "x".into();
        })
    };
    assert!(can_delete_pr_comment(
        Some(&writer),
        Some(&by("gh-comment-IC_1", "avery"))
    ));
    assert!(can_delete_pr_comment(
        Some(&write_only),
        Some(&by("gh-comment-IC_2", "sam"))
    ));
    assert!(!can_delete_pr_comment(
        Some(&other),
        Some(&by("gh-comment-IC_2", "sam"))
    ));
    assert!(!can_delete_pr_comment(
        Some(&writer),
        Some(&by("gh-review-PRR_1", "avery"))
    ));
    let mut desc = by("gh-description", "avery");
    desc.kind = "description".into();
    assert!(!can_delete_pr_comment(Some(&writer), Some(&desc)));
    assert!(!can_edit_pr_comment(None, Some(&by("c1", "avery"))));
    assert!(can_edit_pr_comment(
        None,
        Some(&c(|c| c.author = "You".into()))
    ));
}

#[test]
fn react_permissions() {
    let writer = gh(|p| {
        p.viewer_login = "avery".into();
        p.viewer_can_write = true;
    });
    let other = gh(|p| p.viewer_login = "pat".into());
    let item = |id: &str, kind: &str, f: fn(&mut PrComment)| {
        c(|c| {
            c.id = id.into();
            c.kind = kind.into();
            f(c);
        })
    };
    let conv = item("gh-comment-IC_1", "", |c| c.author_login = "avery".into());
    assert!(can_react_pr_comment(Some(&writer), Some(&conv)));
    assert!(can_react_pr_comment(Some(&other), Some(&conv)));
    assert!(!can_react_pr_comment(
        Some(&writer),
        Some(&item("gh-review-action-1", "action", |c| c.verb =
            "approved".into()))
    ));
    assert!(!can_react_pr_comment(
        Some(&writer),
        Some(&item("gh-review-1", "comment", |c| c.verb = "approved".into()))
    ));
    assert!(can_react_pr_comment(
        Some(&writer),
        Some(&item("gh-review-1", "comment", |c| c.body_md = "ship it".into()))
    ));
    assert!(can_react_pr_comment(
        Some(&writer),
        Some(&item("gh-description", "description", |_| {}))
    ));
    assert!(!can_react_pr_comment(
        Some(&writer),
        Some(&item("gh-event-1", "action", |_| {}))
    ));
    assert!(!can_react_pr_comment(
        Some(&writer),
        Some(&item("gh-event-c", "action", |c| c.commit_sha = "abc".into()))
    ));
    assert!(!can_react_pr_comment(
        Some(&writer),
        Some(&item("c-pending-1", "", |_| {}))
    ));
    assert!(!can_react_pr_comment(Some(&gh(|_| {})), Some(&conv)));
    let local = Pr {
        source: PrSource::Local,
        ..Pr::default()
    };
    assert!(can_react_pr_comment(Some(&local), Some(&conv)));
}

#[test]
fn resolve_permissions() {
    let n = c(|c| {
        c.id = "gh-inline-1".into();
        c.path = "a.ts".into();
        c.thread_id = "PRRT_1".into();
    });
    let pr = |viewer: &str, write: bool| {
        gh(|p| {
            p.viewer_login = viewer.into();
            p.author_login = "loopbot".into();
            p.viewer_can_write = write;
        })
    };
    assert_eq!(line_thread_key(Some(&n)), "PRRT_1");
    assert!(can_resolve_line_note(Some(&pr("avery", true)), Some(&n)));
    assert!(can_resolve_line_note(Some(&pr("loopbot", false)), Some(&n)));
    assert!(!can_resolve_line_note(Some(&pr("pat", false)), Some(&n)));

    let mut no_thread = n.clone();
    no_thread.thread_id.clear();
    assert!(!can_resolve_line_note(
        Some(&pr("avery", true)),
        Some(&no_thread)
    ));
    let local = Pr {
        source: PrSource::Local,
        ..Pr::default()
    };
    let local_note = c(|c| {
        c.id = "c1".into();
        c.path = "a.ts".into();
    });
    assert!(can_resolve_line_note(Some(&local), Some(&local_note)));
    assert!(!can_resolve_line_note(
        Some(&pr("avery", true)),
        Some(&c(|c| c.id = "c1".into()))
    ));
}

fn thread(id: &str, thread: &str, to: &str) -> PrComment {
    c(|c| {
        c.id = id.into();
        c.thread_id = thread.into();
        c.in_reply_to_id = to.into();
    })
}

#[test]
fn thread_resolved_paint_and_holds() {
    let root = thread("gh-inline-1", "PRRT_1", "");
    let reply = thread("gh-inline-2", "PRRT_1", "gh-inline-1");
    let other = thread("gh-inline-3", "PRRT_2", "");
    assert!(comment_matches_thread(&root, "PRRT_1"));
    assert!(comment_matches_thread(&root, "gh-inline-1"));
    assert!(comment_matches_thread(&reply, "PRRT_1"));
    assert!(!comment_matches_thread(&other, "PRRT_1"));

    let resolved = |c: &PrComment| PrComment {
        resolved: true,
        ..c.clone()
    };
    let want = [resolved(&root), resolved(&reply), other.clone()];
    let mut list = [root.clone(), reply.clone(), other.clone()];
    apply_thread_resolved(&mut list, "PRRT_1", true);
    assert_eq!(list, want);

    let mut stale = [root, reply, other];
    apply_thread_resolved_holds(&mut stale, &HashMap::from([("PRRT_1".to_owned(), true)]));
    assert_eq!(stale, want);
}

#[test]
fn thread_editor_drafts_survive_and_drop_when_empty() {
    let mut drafts = ThreadEditorDrafts::default();
    let id = "gh-inline-draft-1";
    let half = ThreadEditorDraft {
        reply_open: true,
        reply: "half a thought".into(),
        ..ThreadEditorDraft::default()
    };
    drafts.set(id, half.clone());
    assert_eq!(drafts.get(id), Some(&half));
    let edit = ThreadEditorDraft {
        editing_id: "c2".into(),
        edit: "typo fix".into(),
        ..ThreadEditorDraft::default()
    };
    drafts.set(id, edit.clone());
    assert_eq!(drafts.get(id), Some(&edit));
    drafts.set(id, ThreadEditorDraft::default());
    assert_eq!(drafts.get(id), None);
}
