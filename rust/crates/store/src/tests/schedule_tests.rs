use chrono::{TimeZone as _, Utc};
use recur_domain::{Flags, ScheduledPrompt, ScheduledPromptRun, Workspace};

use super::*;
use crate::schedules::{ACTIVE_RUN_FILTER, RUN_SELECT, iso_from_millis, millis_from_iso};

fn base(id: &str, proj: &str) -> ScheduledPrompt {
    ScheduledPrompt {
        id: id.into(),
        project_id: proj.into(),
        name: id.into(),
        prompt: "go".into(),
        base_branch: "main".into(),
        mode: "ask".into(),
        model: "claude-code/sonnet".into(),
        cadence: "cron".into(),
        cron: "0 9 * * *".into(),
        time_zone: "UTC".into(),
        next_run_at: 100,
        simplify: Some(false),
        ..Default::default()
    }
}

fn run(id: &str, sched: &str, due: i64, status: &str) -> ScheduledPromptRun {
    ScheduledPromptRun {
        id: id.into(),
        schedule_id: sched.into(),
        due_at: due,
        status: status.into(),
        ..Default::default()
    }
}

#[test]
fn scheduled_prompt_persists_claims_across_restart() {
    let dir = tmp();
    let st = open_temp(dir.path());
    let proj = st.projects().unwrap()[0].id.clone();
    let mut p = ScheduledPrompt {
        flags: Flags::from([("effort".to_owned(), "low".to_owned())]),
        name: "Daily".into(),
        prompt: "check".into(),
        ..base("daily", &proj)
    };
    st.save_scheduled_prompt(&p).unwrap();
    let r = run("run", &p.id, 100, "running");
    p.next_run_at = 200;
    assert!(st.claim_scheduled_prompt(&p, &r).unwrap());
    st.close().unwrap();

    let st = open_temp(dir.path());
    let all = st.scheduled_prompts().unwrap();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].next_run_at, 200);
    assert_eq!(all[0].last_run.as_ref().unwrap().status, "running");
    assert_eq!(all[0].flags["effort"], "low");
    assert_eq!(all[0].simplify, Some(false));
    let dup = ScheduledPromptRun {
        id: "duplicate".into(),
        ..r
    };
    assert!(!st.claim_scheduled_prompt(&p, &dup).unwrap());
    st.delete_project(&p.project_id).unwrap();
    assert!(st.scheduled_prompts().unwrap().is_empty());
    assert!(st.scheduled_prompt_runs().unwrap().is_empty());
}

#[test]
fn due_and_active_scheduled_prompt_queries() {
    let dir = tmp();
    let st = open_temp(dir.path());
    let proj = st.projects().unwrap()[0].id.clone();
    let due = base("due", &proj);
    let later = ScheduledPrompt {
        next_run_at: 9_999_999_999_000,
        ..base("later", &proj)
    };
    let paused = ScheduledPrompt {
        paused: true,
        ..base("paused", &proj)
    };
    let reboot = ScheduledPrompt {
        cron: "@reboot".into(),
        next_run_at: 0,
        ..base("boot", &proj)
    };
    for p in [&due, &later, &paused, &reboot] {
        st.save_scheduled_prompt(p).unwrap();
    }
    let now = Utc.timestamp_millis_opt(200).unwrap();
    let got = st.due_scheduled_prompts(now, false).unwrap();
    assert_eq!(
        got.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(),
        ["due"]
    );
    assert_eq!(st.due_scheduled_prompts(now, true).unwrap().len(), 2);

    assert!(
        st.claim_scheduled_prompt(&due, &run("r1", "due", 50, "running"))
            .unwrap()
    );
    assert!(
        st.claim_scheduled_prompt(&due, &run("r2", "due", 10, "finished"))
            .unwrap()
    );
    let active = st.active_scheduled_prompt_runs().unwrap();
    assert_eq!(
        active.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
        ["r1"]
    );
    assert!(
        st.claim_scheduled_prompt(&due, &run("r3", "due", 60, "interrupted"))
            .unwrap()
    );
    let active = st.active_scheduled_prompt_runs().unwrap();
    assert_eq!(
        active.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
        ["r1", "r3"]
    );
    assert_eq!(active[1].due_at, 60);
}

#[test]
fn active_scheduled_prompt_runs_uses_partial_index() {
    let dir = tmp();
    let st = open_temp(dir.path());
    let plan: Vec<String> = db(&st)
        .with(|c| {
            let mut stmt = c
                .prepare(&format!(
                    "EXPLAIN QUERY PLAN {RUN_SELECT} WHERE {ACTIVE_RUN_FILTER} ORDER BY created_at, rowid"
                ))
                .unwrap();
            let rows = stmt.query_map([], |r| r.get::<_, String>(3)).unwrap();
            Ok(rows.map(Result::unwrap).collect())
        })
        .unwrap();
    let joined = plan.join("; ");
    assert!(joined.contains("scheduled_prompt_run_active"), "{joined}");
    assert!(!joined.contains("TEMP B-TREE"), "{joined}");
}

#[test]
fn scheduled_prompt_run_outcome_keeps_cleared_workspace() {
    let dir = tmp();
    let st = open_temp(dir.path());
    let proj = st.projects().unwrap()[0].id.clone();
    let ws = st
        .add_workspace(Workspace {
            id: "sched-ws".into(),
            project_id: proj.clone(),
            goal: "g".into(),
            ..Default::default()
        })
        .unwrap();
    let p = ScheduledPrompt {
        cadence: "once".into(),
        cron: String::new(),
        date: "2026-09-10".into(),
        time: "09:00".into(),
        ..base("once", &proj)
    };
    st.save_scheduled_prompt(&p).unwrap();
    let mut r = run("run", &p.id, 100, "running");
    assert!(st.claim_scheduled_prompt(&p, &r).unwrap());
    st.link_scheduled_prompt_run(&r.id, &ws.id).unwrap();
    assert_eq!(st.scheduled_prompt_runs().unwrap()[0].workspace_id, ws.id);
    st.delete_workspace(&ws.id).unwrap();
    r.workspace_id = ws.id.clone();
    r.status = "failed".into();
    r.detail = "workspace gone".into();
    st.update_scheduled_prompt_run(&r).unwrap();
    let runs = st.scheduled_prompt_runs().unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].status, "failed");
    assert_eq!(runs[0].workspace_id, "");
}

#[test]
fn schedule_project_is_fixed_and_delete_drops_history() {
    let dir = tmp();
    let st = open_temp(dir.path());
    let p = base("s1", "storefront");
    st.save_scheduled_prompt(&p).unwrap();
    let moved = ScheduledPrompt {
        project_id: "payments".into(),
        ..p.clone()
    };
    assert_eq!(
        st.save_scheduled_prompt(&moved).unwrap_err().to_string(),
        "schedule s1 belongs to another project"
    );
    let unsimplified = ScheduledPrompt {
        simplify: None,
        ..p.clone()
    };
    st.save_scheduled_prompt(&unsimplified).unwrap();
    assert_eq!(
        st.scheduled_prompts().unwrap()[0].simplify,
        Some(true),
        "nil simplify saves as on"
    );
    assert!(
        st.claim_scheduled_prompt(&p, &run("r", "s1", 5, "running"))
            .unwrap()
    );
    st.delete_scheduled_prompt("s1").unwrap();
    assert!(st.scheduled_prompts().unwrap().is_empty());
    assert!(st.scheduled_prompt_runs().unwrap().is_empty());
}

#[test]
fn iso_millis_round_trip() {
    assert_eq!(iso_from_millis(0), "");
    assert_eq!(
        iso_from_millis(1_757_423_100_000),
        "2025-09-09T13:05:00.000Z"
    );
    assert_eq!(
        iso_from_millis(1_757_423_100_123),
        "2025-09-09T13:05:00.123Z"
    );
    assert_eq!(millis_from_iso("").unwrap(), 0);
    assert_eq!(
        millis_from_iso("2025-09-09T13:05:00.123Z").unwrap(),
        1_757_423_100_123
    );
    assert_eq!(
        millis_from_iso("2025-09-09T15:05:00+02:00").unwrap(),
        1_757_423_100_000
    );
    assert!(millis_from_iso("nope").is_err());
}
