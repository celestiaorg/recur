//! The Uncommitted tab's editable cards: a text file's card body is its host's editable diff
//! (the editor's `EditorKind::Merge` against HEAD), laid out at its content's height so the
//! stack scrolls as one.

use std::cell::Cell;
use std::collections::HashSet;
use std::rc::Rc;

use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, EntityId, InteractiveElement as _,
    IntoElement, ParentElement, Render, Styled, Window, canvas, div, px,
};
use recur_domain::FileDiff;
use recur_editor::view::EditorView;

/// The host's editors for the Uncommitted tab's cards. The host owns their buffers and saves
/// them; the stack asks for one per card near the viewport.
pub trait WorkingEditors {
    /// `file`'s editor, or `None` when it is not editable or its texts are still loading (the
    /// host redraws the stack when they land).
    fn editor(
        &self,
        file: &FileDiff,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Entity<EditorView>>;
    /// Only `paths` have cards near the viewport: release the other editors, keeping their
    /// unsaved text.
    fn retain(&self, paths: HashSet<String>, cx: &mut App);
}

/// A card's editor as an isolated view: a key renders it alone, and the card around it
/// redraws only when the editor's content height changes.
pub struct CardEditor {
    editor: Entity<EditorView>,
    /// The height the card laid the editor out at.
    height: Rc<Cell<f32>>,
    card: EntityId,
}

impl CardEditor {
    pub fn new(editor: Entity<EditorView>, card: EntityId, cx: &mut App) -> Entity<Self> {
        cx.new(|_| CardEditor {
            editor,
            height: Rc::default(),
            card,
        })
    }

    pub fn editor(&self) -> &Entity<EditorView> {
        &self.editor
    }

    /// The card's side: the editor at its content's height.
    pub fn element(this: &Entity<Self>, cx: &App) -> AnyElement {
        let me = this.read(cx);
        let h = me.editor.read(cx).content_height().ceil();
        me.height.set(h);
        let mut style = div().w_full().flex_none().h(px(h));
        recur_ui::style::isolated_view(this, style.style().clone())
    }

    fn render_view(&mut self, _: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let (editor, height, card) = (self.editor.clone(), self.height.clone(), self.card);
        let sel = format!("df-editor:{}", self.editor.read(cx).path());
        // After the editor's prepaint, which lays out what an edit added.
        let probe = canvas(
            move |_, _, cx| {
                let h = editor.read(cx).content_height().ceil();
                if height.replace(h) != h {
                    cx.defer(move |cx| cx.notify(card));
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .inset_0();
        div()
            .debug_selector(move || sel.clone())
            .relative()
            .size_full()
            .child(self.editor.clone())
            .child(probe)
            .into_any_element()
    }
}

impl Render for CardEditor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        recur_ui::guard::view(self, window, cx, Self::render_view)
    }
}
