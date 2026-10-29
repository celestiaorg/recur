//! The HTML preview on Linux through WPE WebKit 2.54's WPEPlatform (`wpe/platform.c`), in place
//! of [`crate::html_gtk`] when `RECUR_HTML_WPE=1` and the build found WPE (`cfg(recur_wpe)`).
//! WebKit hands frames over as it renders them (up to 60 a second, the view's frame clock), and
//! input reaches the page as real WPE events:
//! trusted, with user activation, native `:hover` / `:active`, IME composition and WebKit's own
//! `<select>` popup. The page and its rules are `html_gtk`'s, except that https iframes load
//! (the main frame's response is what gets refused), as on macOS.
//!
//! [`Page::input`] takes `html_gtk::INPUT_JS`'s events, so `html.rs` drives both the same way.
#![allow(unsafe_code)]

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ffi::{CStr, CString, c_char, c_int, c_uint, c_void};
use std::rc::Rc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

use futures::channel::mpsc::UnboundedSender;
use serde_json::Value;
use webkit6::glib;

use crate::html::BRIDGE_JS;
pub use crate::html_gtk::PageEvent;

/// Whether the HTML preview uses this page (`RECUR_HTML_WPE=1`).
pub fn enabled() -> bool {
    std::env::var_os("RECUR_HTML_WPE").is_some_and(|v| v == "1")
}

type EventFn = unsafe extern "C" fn(u64, c_int, *const c_char, c_int, c_int, c_int, *const u8);
type Raw = *mut c_void;

const EV_FRAME: c_int = 0;
const EV_MESSAGE: c_int = 1;
const EV_LOADED: c_int = 2;
const EV_CURSOR: c_int = 3;
const EV_REPLY: c_int = 4;

// WPEEventType and WPEModifiers.
const POINTER_DOWN: c_int = 1;
const POINTER_UP: c_int = 2;
const POINTER_MOVE: c_int = 3;
const POINTER_ENTER: c_int = 4;
const POINTER_LEAVE: c_int = 5;
const MOD_CONTROL: c_uint = 1;
const MOD_SHIFT: c_uint = 1 << 1;
const MOD_ALT: c_uint = 1 << 2;
const MOD_META: c_uint = 1 << 3;
const MOD_BUTTON1: c_uint = 1 << 8;

macro_rules! api {
    ($($name:ident: fn($($arg:ty),*) $(-> $ret:ty)?;)*) => {
        struct Api {
            $($name: unsafe extern "C" fn($($arg),*) $(-> $ret)?,)*
            _lib: libloading::Library,
        }

        /// The shim, with its own copy of WebKit's symbols ahead of WebKitGTK's.
        fn load() -> Result<Api, libloading::Error> {
            use libloading::os::unix::{Library, RTLD_LOCAL, RTLD_NOW};
            // SAFETY: the shim's constructors are GLib type registrations.
            let lib: libloading::Library = unsafe {
                Library::open(Some(env!("RECUR_WPE_SHIM")), RTLD_NOW | RTLD_LOCAL | libc::RTLD_DEEPBIND)?
            }
            .into();
            // SAFETY: the signatures match `wpe/platform.c`.
            unsafe {
                Ok(Api {
                    $($name: *lib.get(concat!("recur_wpe_page_", stringify!($name), "\0").as_bytes())?,)*
                    _lib: lib,
                })
            }
        }
    };
}

api! {
    new: fn(*const *const c_char, EventFn, u64) -> Raw;
    free: fn(Raw);
    load: fn(Raw, *const c_char);
    resize: fn(Raw, c_int, c_int, f64);
    set_active: fn(Raw, c_int);
    focus: fn(Raw, c_int);
    pointer: fn(Raw, c_int, f64, f64, c_uint, c_uint, c_uint);
    scroll: fn(Raw, f64, f64, f64, f64, c_uint);
    key: fn(Raw, *const c_char, c_uint) -> c_int;
    ime: fn(Raw, *const c_char, c_uint, c_int);
    command: fn(Raw, *const c_char);
    pick: fn(Raw, c_int);
    eval: fn(Raw, *const c_char, c_int);
}

/// The `recur-wpe` thread's main context (WebKit's main run loop) and the shim.
fn host() -> Option<&'static (glib::MainContext, Api)> {
    static HOST: OnceLock<Option<(glib::MainContext, Api)>> = OnceLock::new();
    HOST.get_or_init(|| {
        let api = load().map_err(|e| eprintln!("wpe: {e}")).ok()?;
        let ctx = glib::MainContext::new();
        let thread_ctx = ctx.clone();
        std::thread::Builder::new()
            .name("recur-wpe".into())
            .spawn(move || {
                let _ = thread_ctx.with_thread_default(|| {
                    glib::MainLoop::new(Some(&thread_ctx), false).run();
                });
            })
            .ok()?;
        Some((ctx, api))
    })
    .as_ref()
}

/// Borrowed only briefly: WebKit calls [`on_event`] from inside the shim's calls.
struct State {
    raw: Raw,
    events: RefCell<UnboundedSender<PageEvent>>,
    /// Whether the pointer is over the page (an enter goes before the first move).
    inside: Cell<bool>,
}

thread_local! {
    static PAGES: RefCell<HashMap<u64, Rc<State>>> = RefCell::default();
}

fn on_thread(f: impl FnOnce(&'static Api) + Send + 'static) -> bool {
    match host() {
        Some((ctx, api)) => {
            ctx.invoke(move || f(api));
            true
        }
        None => false,
    }
}

fn with(id: u64, f: impl FnOnce(&'static Api, &State) + Send + 'static) {
    on_thread(move |api| {
        if let Some(s) = PAGES.with(|p| p.borrow().get(&id).cloned()) {
            f(api, &s);
        }
    });
}

fn cstr(s: &str) -> CString {
    CString::new(s.replace('\0', "")).unwrap_or_default()
}

unsafe extern "C" fn on_event(
    id: u64,
    kind: c_int,
    text: *const c_char,
    width: c_int,
    height: c_int,
    stride: c_int,
    pixels: *const u8,
) {
    let Some(events) = PAGES.with(|p| p.borrow().get(&id).map(|s| s.events.borrow().clone()))
    else {
        return;
    };
    // SAFETY: `text` is a NUL-terminated string for the duration of the call, or null.
    let text = || unsafe {
        text.as_ref()
            .map(|t| CStr::from_ptr(t).to_string_lossy().into_owned())
    };
    let ev = match kind {
        EV_FRAME => {
            let (w, h, stride) = (width as usize, height as usize, stride as usize);
            // SAFETY: the SHM buffer holds `height` rows of `stride` bytes.
            let src = unsafe { std::slice::from_raw_parts(pixels, stride * h) };
            let mut bgra = Vec::with_capacity(w * h * 4);
            for row in src.chunks_exact(stride) {
                bgra.extend_from_slice(&row[..w * 4]);
            }
            PageEvent::Frame {
                width: w as u32,
                height: h as u32,
                bgra,
            }
        }
        EV_MESSAGE => PageEvent::Message(text().unwrap_or_default()),
        EV_LOADED => PageEvent::Message("\u{0}loaded".into()),
        EV_CURSOR => PageEvent::Reply(Value::String(text().unwrap_or_default())),
        EV_REPLY => match text().and_then(|t| serde_json::from_str(&t).ok()) {
            Some(v) => PageEvent::Reply(v),
            None => return,
        },
        _ => return,
    };
    let _ = events.unbounded_send(ev);
}

/// The selection, for a copy: `html_gtk::INPUT_JS`'s answer.
const COPY_JS: &str = r#"(function(){const t=document.activeElement;return '\u0001'+(t&&t.value!==undefined&&t.selectionStart!=null?t.value.substring(t.selectionStart,t.selectionEnd):getSelection().toString())})()"#;

/// A page on the `recur-wpe` thread; dropping it destroys the view.
pub struct Page {
    id: u64,
}

impl Page {
    /// Creates the view and loads `srcdoc`; `None` without the shim.
    pub fn new(srcdoc: &str, events: UnboundedSender<PageEvent>) -> Option<Page> {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        let doc = cstr(srcdoc);
        on_thread(move |api| {
            let bridge = cstr(BRIDGE_JS);
            let scripts = [bridge.as_ptr(), std::ptr::null()];
            // SAFETY: `scripts` is NULL-terminated; the shim copies it.
            let raw = unsafe { (api.new)(scripts.as_ptr(), on_event, id) };
            PAGES.with(|p| {
                p.borrow_mut().insert(
                    id,
                    Rc::new(State {
                        raw,
                        events: RefCell::new(events),
                        inside: Cell::new(false),
                    }),
                )
            });
            // SAFETY: `raw` is live until `free`.
            unsafe { (api.load)(raw, doc.as_ptr()) };
        })
        .then_some(Page { id })
    }

    pub fn load(&self, srcdoc: &str, events: UnboundedSender<PageEvent>) {
        let doc = cstr(srcdoc);
        with(self.id, move |api, s| {
            s.events.replace(events);
            // SAFETY: as in `new`.
            unsafe { (api.load)(s.raw, doc.as_ptr()) };
        });
    }

    /// Lays the page out at `w` x `h` CSS pixels, rendered at `scale` device pixels each.
    pub fn resize(&self, w: f32, h: f32, scale: f32) {
        with(self.id, move |api, s| unsafe {
            (api.resize)(s.raw, w.round() as c_int, h.round() as c_int, scale as f64)
        });
    }

    /// Whether the preview shows (WebKit renders only then).
    pub fn set_active(&self, active: bool) {
        with(self.id, move |api, s| unsafe {
            (api.set_active)(s.raw, c_int::from(active))
        });
    }

    /// `iframe.contentWindow.postMessage(data, '*')`.
    pub fn dispatch(&self, data: &Value) {
        let js = cstr(&format!(
            "window.dispatchEvent(new MessageEvent('message', {{ data: {data} }}));"
        ));
        with(self.id, move |api, s| unsafe {
            (api.eval)(s.raw, js.as_ptr(), 0)
        });
    }

    /// Composition text from the host's IME: the preedit (`cursor` in characters), or with
    /// `commit` the text to insert.
    pub fn ime(&self, text: &str, cursor: usize, commit: bool) {
        let text = cstr(text);
        with(self.id, move |api, s| unsafe {
            (api.ime)(s.raw, text.as_ptr(), cursor as c_uint, c_int::from(commit))
        });
    }

    /// One of `html_gtk::INPUT_JS`'s events, as WPE events.
    pub fn input(&self, ev: &Value) {
        let ev = ev.clone();
        with(self.id, move |api, s| {
            let num = |k: &str| ev.get(k).and_then(Value::as_f64).unwrap_or(0.);
            let flag = |k: &str| ev.get(k).and_then(Value::as_bool).unwrap_or(false);
            let mut mods = 0;
            for (k, m) in [
                ("ctrl", MOD_CONTROL),
                ("shift", MOD_SHIFT),
                ("alt", MOD_ALT),
                ("meta", MOD_META),
            ] {
                if flag(k) {
                    mods |= m;
                }
            }
            if num("buttons") as u32 & 1 != 0 {
                mods |= MOD_BUTTON1;
            }
            let (x, y, raw) = (num("x"), num("y"), s.raw);
            let pointer =
                |ty, button, count| unsafe { (api.pointer)(raw, ty, x, y, button, count, mods) };
            // DOM button numbers (0 left, 1 middle, 2 right) to WPE's (1, 2, 3).
            let button = num("button") as c_uint + 1;
            let count = (num("detail") as c_uint).max(1);
            match ev.get("type").and_then(Value::as_str).unwrap_or_default() {
                "move" => {
                    if !s.inside.replace(true) {
                        pointer(POINTER_ENTER, 0, 0);
                    }
                    pointer(POINTER_MOVE, 0, 0);
                }
                "leave" => {
                    s.inside.set(false);
                    // WebKit takes a leave as a move to its position: one off the page.
                    unsafe { (api.pointer)(raw, POINTER_LEAVE, -1., -1., 0, 0, mods) };
                }
                "down" => {
                    unsafe { (api.focus)(raw, 1) };
                    pointer(POINTER_DOWN, button, count);
                }
                "up" => pointer(POINTER_UP, button, count),
                "wheel" => unsafe { (api.scroll)(raw, x, y, -num("dx"), -num("dy"), mods) },
                "pick" => unsafe { (api.pick)(raw, num("index") as c_int) },
                "key" => {
                    let key = ev.get("key").and_then(Value::as_str).unwrap_or_default();
                    if flag("copy") {
                        let js = cstr(COPY_JS);
                        unsafe { (api.eval)(raw, js.as_ptr(), 1) };
                    } else if flag("selectAll") {
                        let cmd = cstr("SelectAll");
                        unsafe { (api.command)(raw, cmd.as_ptr()) };
                    } else if let Some(text) = ev.get("paste").and_then(Value::as_str) {
                        let text = cstr(text);
                        unsafe { (api.ime)(raw, text.as_ptr(), 0, 1) };
                    } else {
                        let key = cstr(key);
                        unsafe { (api.key)(raw, key.as_ptr(), mods) };
                    }
                }
                _ => {}
            }
        });
    }
}

impl Drop for Page {
    fn drop(&mut self) {
        let id = self.id;
        on_thread(move |api| {
            if let Some(s) = PAGES.with(|p| p.borrow_mut().remove(&id)) {
                unsafe { (api.free)(s.raw) };
            }
        });
    }
}
