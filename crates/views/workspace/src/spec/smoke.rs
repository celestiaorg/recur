use gpui::TestAppContext;
use recur_services::fake::Overrides;

use super::harness::*;

#[gpui::test]
async fn opens_the_workspace_with_its_chat_dock(cx: &mut TestAppContext) {
    let mut h = boot(cx, Overrides::default());
    assert!(h.has("v-detail"));
    assert!(h.has("obs-left"));
    assert!(h.has("chat-composer"), "codes {:?}", h.codes());
}
