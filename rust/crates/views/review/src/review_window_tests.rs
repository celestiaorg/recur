use super::*;
use crate::review_scroll::{PANE_SCROLL_BIAS, pane_scroll_to};

fn set(paths: &[&str]) -> HashSet<String> {
    paths.iter().map(|p| p.to_string()).collect()
}

fn strings(paths: &[&str]) -> Vec<String> {
    paths.iter().map(|p| p.to_string()).collect()
}

fn dense(h: &[f32]) -> Vec<Option<f32>> {
    h.iter().map(|v| Some(*v)).collect()
}

#[test]
fn estimate_is_a_short_header_when_collapsed() {
    assert!(estimate_diff_height(Some(400), 0, 0, true) < 50.);
}

#[test]
fn estimate_grows_with_line_count() {
    let small = estimate_diff_height(Some(10), 0, 0, false);
    let mid = estimate_diff_height(Some(500), 0, 0, false);
    let big = estimate_diff_height(Some(800), 0, 0, false);
    assert!(big > small);
    assert!(big > mid);
}

#[test]
fn estimate_uses_at_least_the_css_line_height() {
    let one = estimate_diff_height(Some(1), 0, 0, false);
    let two = estimate_diff_height(Some(2), 0, 0, false);
    assert!(two - one >= 20.);
}

#[test]
fn estimate_falls_back_to_add_del_when_lines_are_missing() {
    let from_stat = estimate_diff_height(None, 20, 4, false);
    assert!(from_stat > estimate_diff_height(None, 0, 0, false));
    assert!(from_stat > estimate_diff_height(Some(4), 0, 0, false));
}

#[test]
fn reserved_does_not_pin_a_live_card() {
    assert_eq!(
        reserved_card_height(Some(400), 0, 0, true, true, Some(8000.)),
        None
    );
    assert_eq!(
        reserved_card_height(Some(400), 0, 0, true, false, Some(8000.)),
        None
    );
    assert_eq!(
        reserved_card_height(Some(10), 0, 0, true, false, Some(400.)),
        None
    );
}

#[test]
fn reserved_keeps_the_measured_body_on_an_expanded_stub() {
    assert_eq!(
        reserved_card_height(Some(10), 0, 0, false, false, Some(400.)),
        Some(400.)
    );
}

#[test]
fn reserved_is_header_only_on_a_collapsed_stub() {
    assert!(reserved_card_height(Some(10), 0, 0, false, true, Some(400.)).expect("stub") < 50.);
}

#[test]
fn reserved_estimates_an_unmeasured_stub() {
    let estimated = estimate_diff_height(Some(10), 0, 0, false);
    assert_eq!(
        reserved_card_height(Some(10), 0, 0, false, false, None),
        Some(estimated)
    );
}

#[test]
fn header_is_stuck_only_while_the_card_spans_the_top() {
    assert!(!header_is_stuck(14., 400., 0.));
    assert!(!header_is_stuck(14., 400., 14.));
    assert!(header_is_stuck(14., 400., 15.));
    assert!(header_is_stuck(14., 400., 200.));
    assert!(!header_is_stuck(14., 400., 400.));
    assert!(!header_is_stuck(14., 400., 500.));
}

fn paths40() -> Vec<String> {
    (0..40).map(|i| format!("f{i}.ts")).collect()
}

#[test]
fn pick_hydrated_returns_empty_without_paths() {
    assert!(pick_hydrated(&[], "", &[], REVIEW_HYDRATE_MAX, REVIEW_HYDRATE_OVERSCAN).is_empty());
}

#[test]
fn pick_hydrated_keeps_the_selected_file_and_its_neighbors() {
    let got = pick_hydrated(&paths40(), "f10.ts", &[], 8, 2);
    assert!(got.contains("f10.ts") && got.contains("f8.ts") && got.contains("f12.ts"));
    assert!(got.len() <= 8);
}

#[test]
fn pick_hydrated_caps_even_when_many_files_are_in_view() {
    let paths = paths40();
    let got = pick_hydrated(&paths, "f0.ts", &paths, 6, 2);
    assert_eq!(got.len(), 6);
    assert!(got.contains("f0.ts"));
}

#[test]
fn pick_hydrated_prefers_files_closer_to_the_selection() {
    let got = pick_hydrated(&paths40(), "f20.ts", &strings(&["f0.ts", "f21.ts"]), 3, 1);
    assert!(got.contains("f20.ts") && got.contains("f21.ts"));
    assert!(!got.contains("f0.ts"));
}

#[test]
fn pick_hydrated_still_hydrates_a_far_viewport() {
    let got = pick_hydrated(
        &paths40(),
        "f0.ts",
        &strings(&["f30.ts", "f31.ts", "f32.ts"]),
        8,
        2,
    );
    for p in ["f0.ts", "f30.ts", "f31.ts", "f32.ts"] {
        assert!(got.contains(p), "{p}");
    }
}

fn range(path: &str, top: f32, bottom: f32) -> CardRange {
    CardRange {
        path: path.into(),
        top,
        bottom,
    }
}

#[test]
fn pick_hydrated_takes_the_true_viewport_before_padded_prefetch() {
    let ranges = vec![
        range("f0.ts", 200., 240.),
        range("f1.ts", 240., 280.),
        range("f2.ts", 280., 320.),
        range("f3.ts", 320., 360.),
        range("f4.ts", 360., 400.),
        range("f10.ts", 1000., 1400.),
        range("f11.ts", 1400., 1800.),
    ];
    let padded = in_view_paths(&ranges, 1000., 500., 900.);
    let got = pick_hydrated(&paths40(), "f0.ts", &padded, 5, 2);
    assert!(got.contains("f0.ts") && got.contains("f10.ts") && got.contains("f11.ts"));
    assert!(!got.contains("f3.ts"));
}

#[test]
fn in_view_paths_includes_files_overlapping_the_padded_viewport() {
    let ranges = vec![
        range("a.ts", 0., 100.),
        range("b.ts", 100., 400.),
        range("c.ts", 400., 900.),
    ];
    assert_eq!(
        in_view_paths(&ranges, 0., 80., 50.),
        strings(&["a.ts", "b.ts"])
    );
    assert_eq!(in_view_paths(&ranges, 500., 80., 0.), strings(&["c.ts"]));
}

#[test]
fn in_view_paths_lists_visible_files_before_the_prefetch_pad() {
    let stacked = vec![
        range("above.ts", 200., 250.),
        range("vis.ts", 1000., 1400.),
        range("below.ts", 2000., 2100.),
    ];
    assert_eq!(
        in_view_paths(&stacked, 1000., 500., 900.),
        strings(&["vis.ts", "above.ts", "below.ts"])
    );
}

#[test]
fn slot_pad_uses_fallback_for_a_missing_heights_list() {
    assert_eq!(slot_pad(None, 0, 10, 20.), 200.);
    assert_eq!(slot_pad(Some(&[]), 2, 5, 20.), 60.);
}

#[test]
fn slot_pad_prefers_measured_heights_and_falls_back_per_slot() {
    let h = vec![Some(40.), None, Some(80.)];
    assert_eq!(slot_pad(Some(&h), 0, 3, 20.), 140.);
}

#[test]
fn read_slot_heights_groups_non_row_siblings_onto_the_preceding_slot() {
    let mut into = Vec::new();
    let changed = read_slot_heights(&[(true, 20.), (false, 60.), (true, 24.)], 2, &mut into);
    assert!(changed);
    assert_eq!(into[2], Some(80.));
    assert_eq!(into[3], Some(24.));
}

#[test]
fn line_window_renders_everything_when_the_file_is_short() {
    assert_eq!(
        line_window(40, 0., 0., 400., DIFF_LINE_PX, DIFF_LINE_OVERSCAN, None),
        LineWin::new(0, 40)
    );
}

#[test]
fn line_window_keeps_the_viewport_plus_overscan() {
    let win = line_window(400, 0., 2000., 400., 20., 10, None);
    assert!(win.start > 0);
    assert!(win.end < 400);
    assert!(win.end - win.start > 20);
}

#[test]
fn line_window_pins_to_the_tail_when_the_body_is_above_the_port() {
    assert_eq!(
        line_window(200, 0., 8000., 400., 20., 8, None),
        LineWin::new(192, 200)
    );
}

#[test]
fn line_window_pins_to_the_head_when_the_body_is_below_the_port() {
    assert_eq!(
        line_window(200, 8000., 0., 400., 20., 8, None),
        LineWin::new(0, 8)
    );
}

#[test]
fn line_window_walks_measured_heights() {
    let heights: Vec<f32> = (0..200).map(|i| if i < 10 { 80. } else { 20. }).collect();
    let heights = dense(&heights);
    let win = line_window(200, 0., 800., 400., 20., 8, Some(&heights));
    assert!(win.start > 0);
    assert!(slot_pad(Some(&heights), 0, win.start, DIFF_LINE_PX) > win.start as f32 * 20.);
}

#[test]
fn cover_index_expands_the_slice_to_include_a_pinned_row() {
    let w = LineWin::new(40, 80);
    assert_eq!(cover_index(w, 10, 200), LineWin::new(10, 80));
    assert_eq!(cover_index(w, 90, 200), LineWin::new(40, 91));
    assert_eq!(cover_index(w, -1, 200), w);
    assert_eq!(cover_index(w, 200, 200), w);
}

#[test]
fn cover_near_leaves_a_distant_pin_out() {
    let view = LineWin::new(0, 80);
    assert_eq!(cover_near(view, 10_000, 12_000, 60), view);
}

#[test]
fn cover_near_expands_only_when_the_pin_is_near() {
    let view = LineWin::new(40, 80);
    assert_eq!(cover_near(view, 50, 200, 60), view);
    assert_eq!(cover_near(view, 90, 200, 60), LineWin::new(40, 91));
}

#[test]
fn window_around_centers_a_jump_without_spanning_the_file() {
    let w = window_around(10_000, 20_000, 60);
    assert_eq!(w.end - w.start, 121);
    assert_eq!(w, LineWin::new(9940, 10_061));
}

#[test]
fn a_later_viewport_does_not_snap_back_to_a_prior_jump_target() {
    let later = line_window(20_000, 0., 6000., 400., 20., 60, None);
    assert!(later.end < 1000);
    assert_eq!(cover_near(later, 10_000, 20_000, 60), later);
}

#[test]
fn jump_line_window_rehydrates_the_same_line_after_the_highlight_is_cleared() {
    let count = 1000;
    let header = line_window(count, 40., 0., 400., DIFF_LINE_PX, DIFF_LINE_OVERSCAN, None);
    let slot = 799;
    assert_eq!(header, LineWin::new(0, 78));
    let mut last_jump = 0;
    assert_eq!(
        jump_line_window(header, count, 800, last_jump, slot),
        LineWin::new(739, 860)
    );
    last_jump = 800;
    assert_eq!(jump_line_window(header, count, 0, last_jump, slot), header);
    assert_eq!(
        jump_line_window(header, count, 800, last_jump, slot),
        header
    );
    last_jump = 0;
    assert_eq!(
        jump_line_window(header, count, 800, last_jump, slot),
        LineWin::new(739, 860)
    );
}

#[test]
fn apply_slot_pin_corrects_layout_drift_when_the_pane_has_not_moved() {
    assert_eq!(apply_slot_pin(100., Some(50.), Some(80.), 100.), Some(70.));
}

#[test]
fn apply_slot_pin_ignores_a_row_that_unmounted() {
    assert_eq!(
        apply_slot_pin(990_049., None, Some(608_867.), 990_049.),
        None
    );
}

#[test]
fn apply_slot_pin_does_not_undo_a_jump_whose_old_first_row_stays_mounted() {
    let (count, view_h, head_h, jump_slot) = (500, 400., 40., 39);
    let view = line_window(
        count,
        head_h,
        0.,
        view_h,
        DIFF_LINE_PX,
        DIFF_LINE_OVERSCAN,
        None,
    );
    let next = window_around(jump_slot, count, DIFF_LINE_OVERSCAN);
    assert_eq!(next.start, view.start);
    let pin_y = head_h;
    let pin_scroll = 0.;
    let scroll = pane_scroll_to(
        0.,
        0.,
        view_h,
        head_h + jump_slot as f32 * DIFF_LINE_PX,
        PANE_SCROLL_BIAS,
    );
    assert_eq!(scroll, 708.);
    assert_eq!(
        apply_slot_pin(scroll, Some(pin_y - scroll), Some(pin_y), pin_scroll),
        None
    );
}

#[test]
fn same_path_list_compares_by_index() {
    assert!(same_path_list(&strings(&["a", "b"]), &strings(&["a", "b"])));
    assert!(!same_path_list(
        &strings(&["a", "b"]),
        &strings(&["b", "a"])
    ));
}

#[test]
fn same_path_set_ignores_insertion_order() {
    assert!(same_path_set(&set(&["a", "b"]), &set(&["b", "a"])));
    assert!(!same_path_set(&set(&["a"]), &set(&["a", "b"])));
}

#[test]
fn record_stub_height_keeps_live_expanded_heights() {
    let mut h = HashMap::new();
    assert!(record_stub_height(&mut h, "a", false, false, 400.));
    assert!(!record_stub_height(&mut h, "a", false, false, 400.));
    assert!(!record_stub_height(&mut h, "a", true, false, 20.));
    assert!(!record_stub_height(&mut h, "a", false, true, 38.));
    assert_eq!(h["a"], 400.);
}
