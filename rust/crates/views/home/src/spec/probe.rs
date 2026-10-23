use gpui::TestAppContext;

use super::harness::plain;

#[gpui::test]
fn slash_menu_rows(cx: &mut TestAppContext) {
    let mut s = plain(cx);
    s.state.update(s.cx, |st, cx| st.open_new_task("", None, cx));
    s.settle();
    s.type_text("/");
    s.wait(300);
    for i in 0..3 {
        eprintln!("row {i}: {:?}", s.bounds(&format!("menu-row-on:{i}")).or(s.bounds(&format!("menu-row:{i}"))));
    }
    eprintln!("menu: {:?}", s.bounds("chat-mention"));
    eprintln!("name: {:?}", s.bounds("mention:/commit"));
    eprintln!("badge: {:?}", s.bounds("badge:Prompt"));
}
