//! The PR list halves of `pr-review.spec.ts`.

use gpui::{Modifiers, TestAppContext, point, px};
use recur_services::mock::{MockOptions, Reply};
use serde_json::json;

use super::harness::*;
use crate::list_filter::title_counts;

fn tb_sub(s: &mut S) -> String {
    let c = s.state.read_with(s.cx, |st, _| title_counts(&st.prs));
    format!("tb-sub={} need you · {} open · {} merged", c.you, c.open, c.merged)
}

fn row(s: &mut S, id: &str) -> bool {
    s.has(&format!("prrow:{id}"))
}

fn row_y(s: &mut S, id: &str) -> f32 {
    f32::from(s.bounds(&format!("prrow:{id}")).expect(id).origin.y)
}

fn lifecycle_seed(sd: &mut recur_services::mock::Seed) {
    sd.insert_pr(pr_row(json!({
        "id": "gh-storefront-90", "number": 90, "title": "WIP checkout polish",
        "branch": "feat/wip", "draft": true, "diff": {"add": 3, "del": 0}, "when": "4h",
    })));
    sd.insert_pr(pr_row(json!({
        "id": "gh-storefront-91", "number": 91, "title": "Abandoned experiment",
        "branch": "feat/abandoned", "state": "Closed", "diff": {"add": 1, "del": 8}, "when": "3d",
    })));
    sd.insert_pr(pr_row(json!({
        "id": "gh-storefront-92", "number": 92, "title": "CI is red", "group": "you",
        "branch": "feat/ci-red", "checksOk": false, "hasChecks": true, "when": "5h",
        "attentionReasons": ["checks_failing"],
    })));
}

#[gpui::test]
fn pr_list_filters_by_lifecycle_and_status(cx: &mut TestAppContext) {
    let mut s = boot(cx, lifecycle_seed, MockOptions::default());
    s.prs_nav();
    let sub = tb_sub(&mut s);
    assert!(!sub.contains("in the loop"));
    assert!(s.has(&sub), "{sub}");

    assert!(s.has("seg:Open:true"));
    for gone in ["Needs you", "Loop"] {
        assert!(!s.has(&format!("seg:{gone}:true")) && !s.has(&format!("seg:{gone}:false")));
    }
    // Default Open is live non-drafts: merged, closed and drafts stay hidden.
    for id in ["482", "479", "gh-storefront-92"] {
        assert!(row(&mut s, id), "{id}");
    }
    for id in ["gh-storefront-90", "gh-storefront-91", "471"] {
        assert!(!row(&mut s, id), "{id}");
    }

    s.seg("Draft");
    assert!(row(&mut s, "gh-storefront-90"));
    assert!(!row(&mut s, "482"));
    s.seg("Merged");
    assert!(row(&mut s, "471"));
    assert!(!row(&mut s, "482"));
    s.seg("Closed");
    assert!(row(&mut s, "gh-storefront-91"));
    assert!(!row(&mut s, "471"));
    s.seg("All");
    let ids: Vec<String> = s.state.read_with(s.cx, |st, _| st.prs.iter().map(|p| p.id.clone()).collect());
    for id in &ids {
        assert!(row(&mut s, id), "{id}");
    }

    s.seg("Open");
    assert!(s.has("sel:Filter by status=Any status"));
    s.click("sel:Filter by status=Any status");
    assert!(s.has("sel-menu"));
    s.click("sel-opt:Needs your review");
    assert!(row(&mut s, "482"));
    assert!(row(&mut s, "479"));
    assert!(!row(&mut s, "gh-storefront-92"));

    s.click("sel-opt:Needs your review");
    s.click("sel-opt:Checks failing");
    assert!(row(&mut s, "gh-storefront-92"));
    assert!(!row(&mut s, "482"));
}

#[gpui::test]
fn pr_list_sorts_within_groups(cx: &mut TestAppContext) {
    let mut s = boot(
        cx,
        |sd| {
            sd.set_prs(Vec::new());
            for (id, n, title, when, created, updated) in [
                ("gh-10", 10, "Zebra late", "now", "2026-08-28T12:00:00Z", "2026-08-26T12:00:00Z"),
                ("gh-20", 20, "Alpha recent", "2d", "2026-08-01T12:00:00Z", "2026-08-28T11:00:00Z"),
            ] {
                sd.insert_pr(pr_row(json!({
                    "id": id, "number": n, "title": title, "when": when, "state": "Open",
                    "createdAt": created, "updatedAt": updated,
                })));
            }
        },
        MockOptions::default(),
    );
    s.prs_nav();
    let order = |s: &mut S| {
        let (a, z) = (row_y(s, "gh-20"), row_y(s, "gh-10"));
        if a < z { ["Alpha recent", "Zebra late"] } else { ["Zebra late", "Alpha recent"] }
    };
    assert_eq!(order(&mut s), ["Alpha recent", "Zebra late"]);
    assert!(s.has("sel:Sort pull requests=Recently updated"));
    let mut pick = |s: &mut S, from: &str, opt: &str| {
        s.click(&format!("sel:Sort pull requests={from}"));
        assert!(s.has("sel-menu"));
        s.click(&format!("sel-opt:{opt}"));
    };
    pick(&mut s, "Recently updated", "Newest");
    assert_eq!(order(&mut s), ["Zebra late", "Alpha recent"]);
    pick(&mut s, "Newest", "Title A–Z");
    assert_eq!(order(&mut s), ["Alpha recent", "Zebra late"]);
    pick(&mut s, "Title A–Z", "Least recently updated");
    assert_eq!(order(&mut s), ["Zebra late", "Alpha recent"]);
    pick(&mut s, "Least recently updated", "Oldest");
    assert_eq!(order(&mut s), ["Alpha recent", "Zebra late"]);
}

#[gpui::test]
fn pr_list_titlebar_keeps_counts_at_the_minimum_window_width(cx: &mut TestAppContext) {
    let mut s = boot_sized(cx, |_| {}, MockOptions::default(), 1100., 720.);
    s.prs_nav();
    let sub = tb_sub(&mut s);
    let counts = s.bounds(&sub).expect("counts");
    let left = s.bounds("tb-left").expect("tb-left");
    assert!(f32::from(counts.right()) <= f32::from(left.right()) + 1.);
    // The sort trigger keeps its icon when the label hides.
    assert!(s.has("sel:Sort pull requests=Recently updated"));
}

#[gpui::test]
fn pr_list_filter_triggers_stay_in_the_pane_with_multiple_selections(cx: &mut TestAppContext) {
    let mut s = boot_sized(cx, |_| {}, MockOptions::default(), 1100., 720.);
    s.prs_nav();
    let hug = s.bounds("sel:Filter by status=Any status").expect("status");
    assert!(f32::from(hug.size.width) < 176. - 1.);
    s.click("sel:Filter by project=All projects");
    s.click("sel-opt:payments-api");
    s.click("sel-opt:storefront");
    s.keys("escape");
    s.click("sel:Filter by status=Any status");
    s.click("sel-opt:Needs your review");
    s.click("sel-opt:Changes requested");
    s.click("sel-opt:Assigned to you");
    s.keys("escape");
    let shown = "sel:Filter by status=Needs your review, Changes requested, Assigned to you";
    let status = s.bounds(shown).expect(shown);
    // Capped at `max-width: 176px`, the value ellipsizes.
    assert!(f32::from(status.size.width) <= 176. + 0.5);
    let pane = s.bounds("briefing-filters").expect("filters");
    let seg = s.bounds("seg:All:false").expect("seg");
    assert!(f32::from(seg.right()) <= f32::from(pane.right()) + 1.);
}

#[gpui::test]
fn pr_list_meta_columns_align_across_rows(cx: &mut TestAppContext) {
    let mut s = boot_sized(cx, |_| {}, MockOptions::default(), 1800., 900.);
    s.prs_nav();
    s.seg("All");
    let ids: Vec<String> = s.state.read_with(s.cx, |st, _| st.prs.iter().map(|p| p.id.clone()).collect());
    assert!(ids.len() > 1);
    let col = |s: &mut S, id: &str, sel: &str| -> f32 {
        let full = if sel == "pr-src:" {
            let gh = s.pr(id).source == recur_domain::PrSource::GitHub;
            format!("pr-src:{id}={}", if gh { "GitHub PR" } else { "Local PR" })
        } else {
            format!("{sel}{id}")
        };
        let b = s.bounds(&full).unwrap_or_else(|| panic!("{full}"));
        f32::from(b.origin.x).round()
    };
    let cols = |s: &mut S, id: &str| (col(s, id, "pr-people:row-"), col(s, id, "pr-src:"));
    let first = cols(&mut s, &ids[0]);
    for id in &ids {
        assert_eq!(cols(&mut s, id), first, "{id}");
    }
}

#[gpui::test]
fn pr_list_stacks_participant_avatars_and_names_them_on_hover(cx: &mut TestAppContext) {
    let crowd: Vec<_> = (0..8).map(|i| json!({ "login": format!("user{i}") })).collect();
    let mut s = boot_sized(
        cx,
        |sd| patch_pr(sd, "479", json!({ "participants": crowd })),
        MockOptions::default(),
        1800.,
        900.,
    );
    s.prs_nav();
    s.seg("All");
    let faces = s.state.read_with(s.cx, |st, _| crate::people::pr_people(st.pr("482").unwrap()));
    assert_eq!(faces.len().min(5), 5);
    let people = s.bounds("pr-people:row-482").expect("people");
    let why = s.bounds("pr-why:482=Needs your review").expect("why");
    let row = s.bounds("prrow:482").expect("row");
    assert!(people.origin.x > row.origin.x);
    assert!(people.origin.x < why.origin.x);
    s.hover("pr-people:row-482");
    assert!(s.has("pr-people-pop:row-482"));
    for f in ["avery", "sam", "loopbot"] {
        assert!(s.has(&format!("pr-face:login:{f}")), "{f}");
    }

    assert!(s.has("pr-face-more=+3"));
    s.hover("pr-people:row-479");
    assert!(s.has("pr-people-pop:row-479"));
}

#[gpui::test]
fn sticky_group_headers_stay_above_hovered_pr_row_chrome(cx: &mut TestAppContext) {
    let mut s = boot_sized(cx, |_| {}, MockOptions::default(), 1280., 320.);
    s.prs_nav();
    s.seg("All");
    let h = s.bounds("briefing-h:you").expect("header");
    let scroll = s.bounds("briefing-scroll").expect("scroll");
    s.cx.simulate_event(gpui::ScrollWheelEvent {
        position: scroll.center(),
        delta: gpui::ScrollDelta::Pixels(point(px(0.), px(-20.))),
        ..Default::default()
    });
    s.settle();
    let header = s.bounds("briefing-h:you").expect("header");
    assert_eq!(header.origin.y, h.origin.y, "the header sticks");
    let r = s.bounds("prrow:482").expect("row");
    assert!(r.origin.y < header.bottom());
    assert!(r.bottom() > header.bottom());
    // A press on the header lands on the header, not the row under it.
    let p = header.center();
    s.cx.simulate_mouse_move(p, None, Modifiers::none());
    s.settle();
    assert!(!s.has("pr-people-pop:row-482"));
    s.cx.simulate_click(p, Modifiers::none());
    s.settle();
    assert_eq!(s.view(), recur_view_core::ViewName::Pr);
}

#[gpui::test]
fn pr_list_prefixes_titles_with_a_source_icon_and_number_instead_of_a_chip(cx: &mut TestAppContext) {
    let mut s = plain(cx);
    s.prs_nav();
    s.seg("All");
    assert!(s.has("pr-src:482=GitHub PR"));
    assert!(s.has("pr-src:479=Local PR"));
    assert!(!s.has("pr-out:479"));
    let src = s.bounds("pr-src:482=GitHub PR").unwrap();
    let row = s.bounds("prrow:482").unwrap();
    assert!(src.origin.x > row.origin.x);

    s.click("pr-out:482");
    assert_eq!(s.cx.opened_url().as_deref(), Some("https://github.com/acme/app/pull/482"));
    assert_eq!(s.view(), recur_view_core::ViewName::Pr);

    s.click("prrow:482");
    assert_eq!(s.view(), recur_view_core::ViewName::PrDetail);
    assert!(s.has("prh-src=GitHub PR"));
    assert!(s.has("pr-people:prh"));
    s.hover("pr-people:prh");
    assert!(s.has("pr-people-pop:prh"));
    // Scrolled, the pane fades at the top; the popout stays above the fade.
    let pane = s.bounds("prpane:conv").expect("pane");
    s.cx.simulate_event(gpui::ScrollWheelEvent {
        position: pane.center(),
        delta: gpui::ScrollDelta::Pixels(point(px(0.), px(-80.))),
        ..Default::default()
    });
    s.settle();
    assert!(s.has("prpane:conv:fade-top"));
    s.hover("pr-people:prh");
    assert!(s.has("pr-people-pop:prh"));
}

#[gpui::test]
fn github_pr_detail_skeletons_the_thread_until_get_pr_hydrates_it(cx: &mut TestAppContext) {
    let full = recur_services::mock::fixtures::seed().pr_by_id("482").cloned().unwrap();
    let mut summary = full.clone();
    for k in ["comments", "files", "commits", "checks"] {
        summary[k] = json!([]);
    }
    let (reply, release) = Reply::deferred();
    let gate = std::sync::Arc::new(parking_lot::Mutex::new(Some(reply)));
    let full2 = full.clone();
    let mut s = boot(
        cx,
        move |sd| patch_pr(sd, "482", summary),
        MockOptions::default().resolver("GetPR", move |_, _, _| {
            gate.lock().take().unwrap_or_else(|| Reply::value(full2.clone()))
        }),
    );
    s.prs_nav();
    s.click("prrow:482");
    assert_eq!(s.view(), recur_view_core::ViewName::PrDetail);
    let title = full["title"].as_str().unwrap().to_string();
    assert!(s.has(&format!("prh-title={title}")));
    assert!(s.has("pr-skel"));
    assert!(!s.has("desc-cn=loopbot"));
    release.send(Reply::value(full));
    s.settle();
    assert!(s.has("desc-cn=loopbot"));
    assert!(!s.has("pr-skel"));
}

#[gpui::test]
fn local_pr_description_is_the_first_conversation_card(cx: &mut TestAppContext) {
    let mut s = plain_with(cx, |sd| {
        patch_pr(
            sd,
            "479",
            json!({
                "summary": [{"kind": "p", "html": "This loop implemented: <b>hidden when a description exists</b>."}],
                "focus": {"title": "Where to focus your review", "body": "The bulk of the change is in <code>e2e.spec.ts</code>."},
                "verify": [{"html": "Opened checkout", "passed": true}],
                "comments": [{
                    "id": "gh-description", "kind": "description", "author": "You",
                    "bodyMd": "Stabilize the flaky checkout tests.\n\n## Changes\n\n- 1 commit across 2 files",
                    "time": "now",
                }],
            }),
        )
    });
    s.open_pr("479");
    let desc = s.bounds("cmt-desc").expect("description");
    assert!(!s.has("sum-p-0"), "the summary hides behind a description");
    let verify = s.bounds("card:Verified in browser").expect("verify");
    assert!(desc.origin.y < verify.origin.y);
}

/// A 1×1 PNG for asset fetches.
const PNG_1X1: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==";

#[gpui::test]
fn renders_github_description_and_comment_images_via_gh_image(cx: &mut TestAppContext) {
    let asset = "https://github.com/user-attachments/assets/aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
    let mut comments = recur_services::mock::fixtures::seed().pr_by_id("482").unwrap()["comments"].clone();
    for c in comments.as_array_mut().unwrap() {
        match c["id"].as_str() {
            Some("gh-description") => c["bodyMd"] = json!(format!("<img alt=\"Screenshot\" src=\"{asset}\" />")),
            Some("c1") => c["bodyMd"] = json!(format!("![inline]({asset})")),
            _ => {}
        }
    }
    let mut s = boot(
        cx,
        move |sd| patch_pr(sd, "482", json!({ "comments": comments })),
        MockOptions::default().resolver("FetchAsset", |_, _, _| {
            Reply::value(json!({"contentType": "image/png", "cacheControl": "", "body": PNG_1X1}))
        }),
    );
    s.open_pr("482");
    let src = format!("md-img:{}", recur_content::markdown::github_image_src(asset));
    assert!(src.starts_with("md-img:/gh-image?url="), "{src}");
    assert!(s.until(|s| s.has(&src)));
    s.click(&src);
    let lb = s.bounds("chat-att-lb").expect("lightbox");
    assert_eq!(f32::from(lb.size.width), 1280.);
    assert_eq!(f32::from(lb.size.height), 800.);
    s.keys("escape");
    assert!(!s.has("chat-att-lb"));
}

#[gpui::test]
fn pr_list_drops_avatars_under_the_title_with_attention_copy_on_a_narrow_pane(cx: &mut TestAppContext) {
    let mut s = plain(cx);
    s.prs_nav();
    s.seg("All");
    let row = s.bounds("prrow:482").expect("row");
    let people = s.bounds("pr-people:row-482").expect("people");
    let why = s.bounds("pr-why:482=Needs your review").expect("why");
    assert!(people.origin.y > row.origin.y + px(10.));
    assert!((f32::from(people.origin.y) - f32::from(why.origin.y)).abs() < 8.);
    assert!(people.origin.x < why.origin.x);
    let ci = s.bounds("pr-ci-mark:482=ok").expect("ci");
    assert!(ci.origin.x > why.origin.x);
}

#[gpui::test]
fn pr_list_and_detail_show_why_a_github_pr_needs_you(cx: &mut TestAppContext) {
    let mut s = plain_with(cx, |sd| {
        sd.insert_pr(pr_row(json!({
            "id": "gh-storefront-88", "number": 88, "group": "you", "title": "Please look at this",
            "branch": "feat/review-me", "state": "Review required", "reviewRequested": true,
            "attentionReasons": ["review_requested"],
        })));
        sd.insert_pr(pr_row(json!({
            "id": "gh-storefront-87", "number": 87, "group": "you", "title": "My failing PR",
            "branch": "feat/ci", "checksOk": false, "hasChecks": true, "authorLogin": "avery",
            "viewerLogin": "avery", "reviewRequested": false, "attentionReasons": ["checks_failing"],
        })));
    });
    s.prs_nav();
    assert!(s.has("pr-why:gh-storefront-88=Needs your review"));
    assert!(s.has("pr-why:gh-storefront-87="));
    assert!(s.has("pr-ci-mark:gh-storefront-87=fail"));
    assert!(s.has("pr-why:482=Needs your review"));
    assert!(s.has("pr-ci-mark:482=ok"));
    s.click("prrow:gh-storefront-88");
    assert_eq!(s.view(), recur_view_core::ViewName::PrDetail);
    assert!(s.has("pr-attn-pill=Needs your review"));
}

#[gpui::test]
fn project_filter_uses_the_app_select(cx: &mut TestAppContext) {
    let mut s = plain(cx);
    s.prs_nav();
    s.click("sel:Filter by project=All projects");
    assert!(s.has("sel-opt:storefront"));
    assert!(s.has("sel-opt:payments-api"));
    s.click("sel-opt:payments-api");
    assert!(s.has("sel-menu"), "a multiple select stays open");
    s.click("sel-opt:storefront");
    let ids = s.cx.update(|_, cx| crate::ui::ui(cx).read(cx).project_ids.clone());
    assert_eq!(ids.len(), 2);
    s.keys("escape");
    assert!(!s.has("sel-menu"));
    assert!(s.has("sel:Filter by project=payments-api, storefront") || s.has("sel:Filter by project=storefront, payments-api"));
}
