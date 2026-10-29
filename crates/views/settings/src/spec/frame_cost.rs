//! What a key typed into a Settings field renders: the views on its path, nothing beside it.

use std::collections::BTreeMap;

use gpui::TestAppContext;

use super::harness::plain;

#[gpui::test]
fn a_key_in_a_settings_field_renders_only_its_path(cx: &mut TestAppContext) {
    let mut s = plain(cx);
    s.open_settings();
    s.section("General");
    s.fill("inp:Branch name prefix", "a");
    recur_ui::guard::count_renders(true);
    for ch in ["b", "c", "d"] {
        s.cx.simulate_input(ch);
        s.cx.run_until_parked();
    }
    let counts: BTreeMap<&'static str, u32> = recur_ui::guard::take_render_counts();
    recur_ui::guard::count_renders(false);
    assert!(
        counts.get("recur_view_settings::view::SettingsView") >= Some(&3),
        "{counts:?}"
    );
    for view in counts.keys() {
        assert!(
            [
                "recur_app::shell::Shell",
                "recur_view_settings::view::SettingsView"
            ]
            .contains(view),
            "a settings key rendered {view}: {counts:?}"
        );
    }
    assert!(s.has("branch-example=abcd/crispy-quiche"));
}
