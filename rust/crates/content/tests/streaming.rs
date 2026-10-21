//! `components/StreamingMarkdown.test.ts`: hidden streams do no presentation work, catch up on
//! return, and wait for complete lines.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use futures::channel::oneshot;
use gpui::{AppContext, TestAppContext};
use recur_content::StreamingMarkdown;
use recur_content::markdown::RenderError;

type Pending = Rc<RefCell<Vec<(String, oneshot::Sender<Result<Arc<str>, RenderError>>)>>>;

#[gpui::test]
fn hidden_streams_do_no_work_catch_up_and_wait_for_lines(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        recur_ui::init(cx);
    });
    let calls: Rc<RefCell<Vec<String>>> = Rc::default();
    let held: Pending = Rc::default();
    let hold_next = Rc::new(RefCell::new(false));
    let (c, h, hold) = (calls.clone(), held.clone(), hold_next.clone());
    let sm = cx.new(|cx| {
        StreamingMarkdown::with_renderer(
            Rc::new(move |src: &str, _key: &str| {
                c.borrow_mut().push(src.to_string());
                let (tx, rx) = oneshot::channel();
                if std::mem::take(&mut *hold.borrow_mut()) {
                    h.borrow_mut().push((src.to_string(), tx));
                } else {
                    let _ = tx.send(Ok(format!("<p>{src}</p>").into()));
                }
                rx
            }),
            cx,
        )
    });
    let set = |cx: &mut TestAppContext, text: &str, active: bool, complete: bool| {
        sm.update(cx, |s, cx| s.set(text.to_string(), active, complete, cx));
        cx.run_until_parked();
    };
    let shown = |cx: &mut TestAppContext| sm.read_with(cx, |s, _| s.html().to_string());

    set(cx, "first reply", true, false);
    assert_eq!(shown(cx), "<p>first reply</p>");

    calls.borrow_mut().clear();
    set(cx, "hidden reply", false, false);
    assert!(calls.borrow().is_empty());
    set(cx, "latest hidden reply", false, false);
    assert!(calls.borrow().is_empty());

    set(cx, "latest hidden reply", true, false);
    assert_eq!(shown(cx), "<p>latest hidden reply</p>");

    set(cx, "latest hidden reply\nplus a line\npartial", true, false);
    assert_eq!(shown(cx), "<p>latest hidden reply\nplus a line\n</p>");

    set(cx, "latest hidden reply\nplus a line\npartial", true, true);
    assert_eq!(
        shown(cx),
        "<p>latest hidden reply\nplus a line\npartial</p>"
    );

    // The app hides (document.hidden): no work while hidden, the latest text on return.
    sm.update(cx, |s, cx| s.set_document_visible(false, cx));
    let before = calls.borrow().len();
    set(cx, "reply while the app is hidden", true, false);
    assert_eq!(calls.borrow().len(), before);
    sm.update(cx, |s, cx| s.set_document_visible(true, cx));
    cx.run_until_parked();
    assert_eq!(shown(cx), "<p>reply while the app is hidden</p>");

    // A render that finishes after the stream went inactive never paints.
    let painted = shown(cx);
    *hold_next.borrow_mut() = true;
    set(cx, "reply while the app is hidden\nmore\n", true, false);
    set(cx, "reply while the app is hidden\nmore\n", false, false);
    let (_, tx) = held.borrow_mut().pop().expect("held render");
    let _ = tx.send(Ok("<p>late worker result</p>".into()));
    cx.run_until_parked();
    assert_eq!(shown(cx), painted);
}
