//! A fixed Insights report shaped like a busy month, so the view renders every section.

use chrono::{Duration, Local, TimeZone};

use crate::api::*;

const H: i64 = 3_600_000;
const M: i64 = 60_000;

pub(super) fn report(end_ms: i64) -> InsightsReport {
    let end = Local
        .timestamp_millis_opt(end_ms)
        .single()
        .unwrap_or_else(Local::now)
        .date_naive();
    let days = (0..371)
        .rev()
        .map(|back| {
            let d = end - Duration::days(back + 1);
            let i = 371 - back;
            let ramp = (i - 270).clamp(0, 80);
            let weekend = matches!(d.format("%u").to_string().as_str(), "6" | "7");
            let h = if ramp == 0 || i % 17 == 0 {
                0
            } else {
                ((i * 7919) % 17 + 3) * ramp / 80 / if weekend { 2 } else { 1 }
            };
            DayHours {
                date: d.format("%Y-%m-%d").to_string(),
                agent_ms: h * H,
            }
        })
        .collect();
    let command = |name: &str, total, calls, p50, max, failed, no_match| CommandStats {
        name: name.into(),
        total_ms: total,
        calls,
        p50_ms: p50,
        max_ms: max,
        failed,
        no_match,
    };
    let model =
        |model: &str, provider: &str, h: i64, runs, typical, per_ask, cmds, failed, tokens| {
            ModelStats {
                model: model.into(),
                provider: provider.into(),
                agent_ms: h * H,
                runs,
                typical_task_ms: typical,
                ms_per_ask: per_ask,
                commands: cmds,
                failed_commands: failed,
                tokens_in: tokens,
            }
        };
    InsightsReport {
        preparing: None,
        agent_ms: 236 * H,
        prev_agent_ms: 180 * H,
        tasks: TaskStats {
            total: 813,
            finished: 781,
            failed: 14,
            failed_by: vec![("plan limit".into(), 8), ("provider error".into(), 6)],
            stopped: 13,
            running: 5,
            from_you: 412,
            from_agent: 197,
            from_schedule: 38,
            projects: 26,
        },
        peak_agents: 11,
        peak_at_ms: end_ms - 2 * 24 * H - 3 * H,
        asks: AskStats {
            count: 41,
            typical_wait_ms: Some(M + 52_000),
            longest_wait_ms: Some(9 * H + 10 * M),
        },
        merged: MergeStats {
            total: 67,
            by_pr: 46,
            direct: 21,
            open_prs: 5,
        },
        days,
        split: TimeSplit {
            commands_ms: 104 * H,
            model_ms: 120 * H,
            waiting_ms: 12 * H,
        },
        lost: vec![
            TimeLost {
                kind: LostKind::BuildLock,
                ms: 9 * H + 12 * M,
                count: 61,
            },
            TimeLost {
                kind: LostKind::RepeatedFailure,
                ms: 3 * H + 30 * M,
                count: 22,
            },
            TimeLost {
                kind: LostKind::OvernightAsk,
                ms: 2 * H + 12 * M,
                count: 4,
            },
            TimeLost {
                kind: LostKind::ProviderError,
                ms: H + M,
                count: 6,
            },
            TimeLost {
                kind: LostKind::ReviewNotSettling,
                ms: 45 * M,
                count: 3,
            },
        ],
        by_hour: [
            2, 1, 0, 0, 0, 0, 1, 3, 6, 9, 12, 13, 10, 11, 12, 11, 10, 9, 7, 6, 6, 5, 4, 3,
        ]
        .map(|h| h * 2 * H / 3),
        by_weekday: [132, 158, 164, 129, 111, 52, 46].map(|h| h * H / 10),
        models: vec![
            model(
                "opus 5.5",
                "claude-code",
                98,
                512,
                Some(38 * M),
                Some(6 * H),
                9_812,
                304,
                Some(2_100_000_000),
            ),
            model(
                "gpt 6.1-sol",
                "codex",
                64,
                398,
                Some(31 * M),
                Some(3 * H),
                7_420,
                430,
                Some(1_400_000_000),
            ),
            model(
                "fable 5.2",
                "claude-code",
                41,
                231,
                Some(44 * M),
                Some(5 * H),
                4_101,
                98,
                Some(900_000_000),
            ),
            model(
                "sonnet 5.5",
                "claude-code",
                19,
                142,
                Some(22 * M),
                Some(7 * H),
                2_204,
                44,
                Some(300_000_000),
            ),
            model("composer 2", "cursor", 6, 48, None, None, 1_303, 52, None),
        ],
        commands: vec![
            command("cargo test", 38 * H, 1_204, 72_000, 14 * M, 108, 0),
            command("cargo build / check", 21 * H, 2_310, 22_000, 4 * M, 92, 0),
            command("mise run test", 14 * H, 388, 3 * M + 50_000, 11 * M, 47, 0),
            command(
                "Read / Edit files",
                10 * H + 36 * M,
                15_332,
                100,
                2_000,
                92,
                0,
            ),
            command("git, gh", 6 * H + 36 * M, 3_013, 400, 38_000, 60, 0),
            command(
                "Search (rg, grep)",
                5 * H + 6 * M,
                6_880,
                200,
                9_000,
                0,
                2_133,
            ),
        ],
        handoffs: HandoffStats {
            to_reviewer: 142,
            to_child: 64,
            review_loops: 142,
            review_settled: 125,
            unfinished: 6,
        },
        context: ContextStats {
            compactions: 32,
            turns_over_90: 9,
            largest: 198_000,
        },
        tokens: TokenStats {
            input: 4_800_000_000,
            cached: 4_704_000_000,
            output: 18_000_000,
        },
        changes: ChangeStats {
            added: 412_000,
            removed: 298_000,
            files: 5_104,
        },
    }
}
