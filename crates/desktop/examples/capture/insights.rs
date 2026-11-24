//! The demo's Insights report: every figure follows from one series of daily agent hours, and
//! nothing falls after the day the capture runs.

use chrono::{Datelike, Duration, Local, NaiveDate, TimeZone, Utc};
use recur_services::api::*;

const M: i64 = 60_000;
const H: i64 = 60 * M;

/// Agent time on day `i` of 371, the last being today: none before the demo's user started,
/// then a ramp with quiet weekends.
fn day_ms(i: i64, d: NaiveDate) -> i64 {
    let ramp = (i - 190).clamp(0, 180);
    if ramp == 0 || i % 19 == 0 {
        return 0;
    }
    let weekday = [9, 11, 12, 10, 8, 3, 2][d.weekday().num_days_from_monday() as usize];
    let wobble = (i * 7919) % 5;
    (weekday + wobble) * 60 * ramp / 180 * M
}

fn ms_of(date: &str) -> i64 {
    NaiveDate::parse_from_str(date, "%Y-%m-%d")
        .ok()
        .and_then(|d| {
            Local
                .from_local_datetime(&d.and_hms_opt(0, 0, 0)?)
                .earliest()
        })
        .map_or(0, |t| t.timestamp_millis())
}

/// `total` split by `weights`, the remainder on the first.
fn split<const N: usize>(total: i64, weights: [i64; N]) -> [i64; N] {
    let sum: i64 = weights.iter().sum();
    let mut out = weights.map(|w| total / M * w / sum * M);
    out[0] += total - out.iter().sum::<i64>();
    out
}

pub fn report(prev_start_ms: i64, start_ms: i64, end_ms: i64) -> InsightsReport {
    let now = Utc::now();
    let today = Local::now().date_naive();
    let days: Vec<DayHours> = (0..371)
        .map(|i| {
            let d = today - Duration::days(370 - i);
            DayHours {
                date: d.to_string(),
                agent_ms: if d == today { 3 * H } else { day_ms(i, d) },
            }
        })
        .collect();
    let end_ms = end_ms.min(now.timestamp_millis());
    let total = |from: i64, to: i64| -> i64 {
        days.iter()
            .filter(|d| (from..to).contains(&ms_of(&d.date)))
            .map(|d| d.agent_ms)
            .sum()
    };
    let agent_ms = total(start_ms, end_ms);

    let mut weekday_ms = [0; 7];
    let mut weekday_n = [0; 7];
    for d in &days {
        let Ok(date) = NaiveDate::parse_from_str(&d.date, "%Y-%m-%d") else {
            continue;
        };
        if (start_ms..end_ms).contains(&ms_of(&d.date)) {
            let w = date.weekday().num_days_from_monday() as usize;
            weekday_ms[w] += d.agent_ms;
            weekday_n[w] += 1;
        }
    }
    let by_weekday = std::array::from_fn(|w| weekday_ms[w] / weekday_n[w].max(1));
    let busiest = days
        .iter()
        .filter(|d| (start_ms..end_ms).contains(&ms_of(&d.date)))
        .max_by_key(|d| d.agent_ms)
        .map_or(start_ms, |d| ms_of(&d.date));

    let [model_ms, commands_ms, waiting_ms] = split(agent_ms, [52, 43, 5]);
    let [opus, gpt, sonnet] = split(agent_ms, [58, 27, 15]);
    let tasks = agent_ms / (17 * M);
    let (failed, stopped, running) = (tasks * 2 / 100, tasks / 100, 4);
    let from_agent = tasks * 28 / 100;
    let from_schedule = tasks * 11 / 100;
    let model = |model: &str, provider: &str, ms: i64, typical: i64, per_ask: i64, tokens: i64| {
        ModelStats {
            model: model.into(),
            provider: provider.into(),
            agent_ms: ms,
            runs: ms / (17 * M),
            typical_task_ms: Some(typical * M),
            ms_per_ask: Some(per_ask * H),
            commands: ms / (38 * 1000),
            failed_commands: ms / (900 * 1000),
            tokens_in: Some(tokens),
        }
    };
    let command = |name: &str, share: i64, calls, p50_ms, max_ms, failed, no_match| CommandStats {
        name: name.into(),
        total_ms: commands_ms / M * share / 100 * M,
        calls,
        p50_ms,
        max_ms,
        failed,
        no_match,
    };
    let [opus_tok, gpt_tok, sonnet_tok] = split(2_600_000_000, [58, 27, 15]);
    InsightsReport {
        preparing: None,
        agent_ms,
        prev_agent_ms: total(prev_start_ms, prev_start_ms + (end_ms - start_ms)),
        tasks: TaskStats {
            total: tasks,
            finished: tasks - failed - stopped - running,
            failed,
            failed_by: vec![
                ("plan limit".into(), failed - failed / 3),
                ("provider error".into(), failed / 3),
            ],
            stopped,
            running,
            from_you: tasks - from_agent - from_schedule,
            from_agent,
            from_schedule,
            projects: 2,
        },
        peak_agents: 7,
        peak_at_ms: (busiest + 11 * H + 20 * M).min(end_ms),
        asks: AskStats {
            count: 34,
            typical_wait_ms: Some(4 * M + 10_000),
            longest_wait_ms: Some(8 * H + 35 * M),
        },
        merged: MergeStats {
            total: 52,
            by_pr: 41,
            direct: 11,
            open_prs: 4,
        },
        split: TimeSplit {
            commands_ms,
            model_ms,
            waiting_ms,
        },
        lost: [
            (LostKind::RepeatedFailure, 200, 24),
            (LostKind::OvernightAsk, 132, 3),
            (LostKind::ProviderError, 61, 6),
            (LostKind::ReviewNotSettling, 45, 3),
        ]
        .map(|(kind, mins, count)| TimeLost {
            kind,
            ms: mins * M,
            count,
        })
        .into(),
        by_hour: split(
            agent_ms,
            [
                1, 1, 4, 2, 0, 0, 2, 3, 8, 14, 17, 18, 11, 13, 16, 15, 13, 10, 6, 4, 5, 4, 2, 1,
            ],
        ),
        by_weekday,
        models: vec![
            model("opus 5.5", "claude-code", opus, 26, 7, opus_tok),
            model("gpt 6.1-sol", "codex", gpt, 19, 9, gpt_tok),
            model("sonnet 5.5", "claude-code", sonnet, 14, 6, sonnet_tok),
        ],
        commands: vec![
            command("go test", 41, 2_106, 24_000, 9 * M, 118, 0),
            command("playwright test", 24, 301, 3 * M + 5_000, 14 * M, 47, 0),
            command("go build", 11, 1_480, 11_000, 2 * M, 61, 0),
            command("Read / Edit files", 10, 9_732, 100, 2_000, 58, 0),
            command("git, gh", 8, 1_913, 500, 41_000, 33, 0),
            command("Search (rg, grep)", 6, 4_220, 200, 8_000, 0, 1_204),
        ],
        handoffs: HandoffStats {
            to_reviewer: 96,
            to_child: 41,
            review_loops: 96,
            review_settled: 87,
            unfinished: 3,
        },
        context: ContextStats {
            compactions: 18,
            turns_over_90: 5,
            largest: 182_000,
        },
        tokens: TokenStats {
            input: 2_600_000_000,
            cached: 2_530_000_000,
            output: 11_000_000,
        },
        changes: ChangeStats {
            added: 61_400,
            removed: 38_200,
            files: 1_206,
        },
        days,
    }
}
