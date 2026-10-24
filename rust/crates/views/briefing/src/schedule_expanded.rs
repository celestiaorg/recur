//! `lib/schedule-expanded.ts`: the open schedule rows on the Schedules page and in project
//! Settings → Schedules, kept in `schedule-view.json` through `ShellService`.

use std::collections::BTreeSet;

use gpui_kit::{App, Global};
use recur_services::Services;

/// The expanded schedule ids (`scheduleExpanded`). Observe it with `cx.observe_global`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ScheduleExpanded(pub BTreeSet<String>);

impl Global for ScheduleExpanded {}

/// `expandedFromIds`: the non-empty ids.
pub fn expanded_from_ids(ids: &[String]) -> BTreeSet<String> {
    ids.iter().filter(|id| !id.is_empty()).cloned().collect()
}

/// `idsFromExpanded`.
pub fn ids_from_expanded(expanded: &BTreeSet<String>) -> Vec<String> {
    expanded.iter().cloned().collect()
}

pub fn is_expanded(id: &str, cx: &App) -> bool {
    cx.try_global::<ScheduleExpanded>()
        .is_some_and(|e| e.0.contains(id))
}

/// `loadScheduleExpanded`: read `schedule-view.json` (a first run or a missing bridge keeps
/// the set empty).
pub fn load(services: &Services, cx: &mut App) {
    cx.default_global::<ScheduleExpanded>();
    let services = services.clone();
    cx.spawn(async move |cx| {
        if let Ok(ids) = services.shell.get_expanded_schedules().await {
            cx.update(|cx| cx.set_global(ScheduleExpanded(expanded_from_ids(&ids))));
        }
    })
    .detach();
}

/// `toggleScheduleExpanded`.
pub fn toggle(id: &str, services: &Services, cx: &mut App) {
    let mut next = cx.default_global::<ScheduleExpanded>().0.clone();
    if !next.remove(id) {
        next.insert(id.to_string());
    }
    let ids = ids_from_expanded(&next);
    cx.set_global(ScheduleExpanded(next));
    let services = services.clone();
    cx.spawn(async move |_| {
        let _ = services.shell.set_expanded_schedules(&ids).await;
    })
    .detach();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expanded_from_ids_keeps_non_empty_ids() {
        let ids = vec!["a".to_string(), String::new(), "b".to_string()];
        let set = expanded_from_ids(&ids);
        assert_eq!(set.len(), 2);
        assert!(set.contains("a") && set.contains("b"));
        assert!(expanded_from_ids(&[]).is_empty());
    }

    #[test]
    fn ids_from_expanded_lists_open_rows() {
        let set: BTreeSet<String> = ["x".to_string(), "y".to_string()].into();
        assert_eq!(ids_from_expanded(&set), vec!["x", "y"]);
        assert_eq!(expanded_from_ids(&ids_from_expanded(&set)), set);
    }
}
