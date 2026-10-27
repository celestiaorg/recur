//! The HTML preview on Linux: GTK 4 cannot place a widget in GPUI's window, so the page is an
//! offscreen WebKitGTK view on the `recur-webkit` thread (`webhost`). It is laid out at the
//! preview's size (zoomed to the window's scale), its visible region is snapshotted and drawn
//! as an image, and pointer, wheel and key input reach it as DOM events through [`INPUT_JS`]
//! (GTK 4 has no public way to synthesize input events for a widget).
//!
//! The rules match the macOS WKWebView: the same CSP'd `srcdoc` loaded without a base URL (so
//! no sibling files), an ephemeral network session, no popups, and no navigation away from
//! `about:blank`. WebKitGTK does not say which frame a navigation is for, so subframe loads
//! are refused too, which is stricter than macOS.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use futures::channel::mpsc::UnboundedSender;
use serde_json::Value;
use webkit6::prelude::*;
use webkit6::{
    NetworkSession, PolicyDecisionType, SnapshotOptions, SnapshotRegion, UserContentInjectedFrames,
    UserContentManager, UserScript, UserScriptInjectionTime, WebView, gdk, glib,
};

use crate::html::BRIDGE_JS;

/// What the page sends back.
pub enum PageEvent {
    /// The visible region, `width` x `height` device pixels of straight BGRA.
    Frame {
        width: u32,
        height: u32,
        bgra: Vec<u8>,
    },
    /// A `parent.postMessage` from the page, or `"\0loaded"`.
    Message(String),
    /// What the input shim answered (the selection for a copy, the cursor for a move).
    Reply(Value),
}

/// Input as DOM events: `move` / `down` / `up` (`x`, `y` in CSS pixels, `button`, `detail`),
/// `leave`, `wheel` (`dx`, `dy`), `key` (`key`, modifiers, `copy` / `selectAll` / `paste`),
/// `pick` (`index`, the option chosen in a `<select>`'s menu).
///
/// Events a script dispatches are untrusted, so WebKit neither applies `:hover` / `:active` nor
/// opens a `<select>` from them, and GTK 4 cannot synthesize trusted ones (WebKit's automation
/// session is reachable only over the remote inspector's private protocol). The shim stands
/// in: every `:hover` / `:active` rule is mirrored onto an attribute it sets on the pointer's
/// chain (as WebKit's inspector forces a state), and a press, Space or Enter on a `<select>`
/// answers `{select: {x, y, w, h, index, options}}` for the host's native menu.
pub const INPUT_JS: &str = r#"(function(){
let hover=null,down=null,anchor=null,picking=null;
const HOVER='data-recur-hover',ACTIVE='data-recur-active',seen=new WeakSet();
function mirror(list,owner){for(let i=0;i<list.length;i++){const r=list[i];if(r.selectorText&&/:(hover|active)\b/.test(r.selectorText)&&!r.selectorText.includes('[data-recur-')){try{owner.insertRule(r.selectorText.replace(/:hover\b/g,'['+HOVER+']').replace(/:active\b/g,'['+ACTIVE+']')+'{'+r.style.cssText+'}',i+1);i++}catch(x){}}if(r.cssRules&&r.cssRules.length)mirror(r.cssRules,r)}}
function mirrorAll(){for(const s of document.styleSheets){if(seen.has(s))continue;seen.add(s);try{mirror(s.cssRules,s)}catch(x){}}}
document.addEventListener('DOMContentLoaded',mirrorAll);new MutationObserver(mirrorAll).observe(document,{childList:true,subtree:true});
function mark(attr,t){document.querySelectorAll('['+attr+']').forEach(n=>{if(!t||!n.contains(t))n.removeAttribute(attr)});for(let n=t;n&&n.nodeType===1;n=n.parentElement)n.setAttribute(attr,'')}
function menu(s){picking=s;const r=s.getBoundingClientRect();return{select:{x:r.left,y:r.top,w:r.width,h:r.height,index:s.selectedIndex,options:[...s.options].map(o=>({label:o.label||o.text,disabled:o.disabled||!!(o.parentElement&&o.parentElement.disabled)}))}}}
function choose(s,i){if(i>=0&&i<s.options.length&&s.selectedIndex!==i){s.selectedIndex=i;s.dispatchEvent(new Event('input',{bubbles:true}));s.dispatchEvent(new Event('change',{bubbles:true}))}}
const popup=t=>{const s=t&&t.closest?t.closest('select'):null;return s&&!s.multiple&&!s.disabled&&s.size<=1?s:null};
const mods=e=>({ctrlKey:!!e.ctrl,shiftKey:!!e.shift,altKey:!!e.alt,metaKey:!!e.meta});
const opts=e=>Object.assign({bubbles:true,cancelable:true,composed:true,view:window,clientX:e.x,clientY:e.y,screenX:e.x,screenY:e.y,button:e.button||0,buttons:e.buttons||0,detail:e.detail||0},mods(e));
const at=e=>document.elementFromPoint(e.x,e.y)||document.documentElement;
const caret=e=>document.caretRangeFromPoint?document.caretRangeFromPoint(e.x,e.y):null;
const editable=el=>!!el&&(el.isContentEditable||el.tagName==='TEXTAREA'||(el.tagName==='INPUT'&&!/^(button|submit|reset|checkbox|radio|file|image|range|color)$/i.test(el.type)));
const field=el=>!!el&&el.value!==undefined&&typeof el.setRangeText==='function';
function edit(t,text,dir){if(field(t)){let a=t.selectionStart,b=t.selectionEnd;if(a===b&&dir<0)a=Math.max(0,a-1);if(a===b&&dir>0)b=Math.min(t.value.length,b+1);t.setRangeText(text,a,b,'end');t.dispatchEvent(new InputEvent('input',{bubbles:true,inputType:text?'insertText':'deleteContent',data:text||null}))}else document.execCommand(text?'insertText':(dir<0?'delete':'forwardDelete'),false,text)}
function scroller(el,dx,dy){for(let n=el;n&&n!==document.documentElement&&n!==document.body;n=n.parentElement){const s=getComputedStyle(n);if((dy&&/(auto|scroll)/.test(s.overflowY)&&n.scrollHeight>n.clientHeight)||(dx&&/(auto|scroll)/.test(s.overflowX)&&n.scrollWidth>n.clientWidth))return n}return document.scrollingElement||document.documentElement}
window.__recurInput=function(e){
 if(e.type==='move'){const t=at(e);if(t!==hover){if(hover)hover.dispatchEvent(new MouseEvent('mouseout',Object.assign(opts(e),{relatedTarget:t})));t.dispatchEvent(new MouseEvent('mouseover',Object.assign(opts(e),{relatedTarget:hover})));hover=t;mark(HOVER,t)}
  t.dispatchEvent(new MouseEvent('mousemove',opts(e)));
  if(down&&anchor){const r=caret(e);if(r)getSelection().setBaseAndExtent(anchor.startContainer,anchor.startOffset,r.startContainer,r.startOffset)}
  return getComputedStyle(t).cursor}
 if(e.type==='leave'){if(hover)hover.dispatchEvent(new MouseEvent('mouseout',opts(e)));hover=null;mark(HOVER,null);return null}
 if(e.type==='pick'){const s=picking;picking=null;if(s){choose(s,e.index);s.focus()}return null}
 if(e.type==='down'){const t=at(e);down=t;t.dispatchEvent(new MouseEvent('mousedown',opts(e)));if(e.button===0)mark(ACTIVE,t);
  let f=t;while(f&&!(f.tabIndex>=0||editable(f)))f=f.parentElement;
  if(f)f.focus();else if(document.activeElement&&document.activeElement.blur)document.activeElement.blur();
  anchor=(e.detail||1)===1?caret(e):null;if(anchor&&!e.shift)getSelection().removeAllRanges();
  if(e.detail===2){const s=getSelection(),r=caret(e);if(r&&s.modify){s.removeAllRanges();s.addRange(r);s.modify('move','backward','word');s.modify('extend','forward','word')}}
  const s=e.button===0?popup(t):null;return s?menu(s):null}
 if(e.type==='up'){const t=at(e);t.dispatchEvent(new MouseEvent('mouseup',opts(e)));
  if(down&&e.button===0&&(t===down||down.contains(t)||t.contains(down)))down.dispatchEvent(new MouseEvent('click',opts(e)));
  down=null;anchor=null;mark(ACTIVE,null);return null}
 if(e.type==='wheel'){const t=at(e);if(t.dispatchEvent(new WheelEvent('wheel',Object.assign(opts(e),{deltaX:e.dx,deltaY:e.dy,deltaMode:0}))))scroller(t,e.dx,e.dy).scrollBy(e.dx,e.dy);return null}
 if(e.type==='key'){const t=document.activeElement||document.body;const init=Object.assign({key:e.key,code:e.code||'',bubbles:true,cancelable:true},mods(e));let out=null;
  if(t.dispatchEvent(new KeyboardEvent('keydown',init))){
   if(e.copy)out='\u0001'+(editable(t)&&t.value!==undefined?t.value.substring(t.selectionStart,t.selectionEnd):getSelection().toString());
   else if(e.selectAll){if(editable(t)&&t.select)t.select();else document.execCommand('selectAll')}
   else if(e.paste!=null){if(editable(t))edit(t,e.paste,0)}
   else if(editable(t)){if(e.key.length===1&&!e.ctrl&&!e.meta)edit(t,e.key,0);else if(e.key==='Backspace')edit(t,'',-1);else if(e.key==='Delete')edit(t,'',1);else if(e.key==='Enter'){if(t.tagName==='INPUT'){if(t.form&&t.form.requestSubmit)t.form.requestSubmit()}else edit(t,'\n',0)}}
   else if(popup(t)&&(e.key===' '||e.key==='Enter'))out=menu(t);
   else if(popup(t)&&(e.key==='ArrowDown'||e.key==='ArrowUp')){const d=e.key==='ArrowDown'?1:-1;let i=t.selectedIndex+d;while(i>=0&&i<t.options.length&&t.options[i].disabled)i+=d;choose(t,i)}
   else if(e.key==='Enter'||(e.key===' '&&t.tagName==='BUTTON')){if(t!==document.body&&t.click)t.click()}
   else{const h=innerHeight,sh=(document.scrollingElement||document.documentElement).scrollHeight;const d={ArrowDown:[0,40],ArrowUp:[0,-40],ArrowLeft:[-40,0],ArrowRight:[40,0],PageDown:[0,h*.875],PageUp:[0,-h*.875],' ':[0,(e.shift?-1:1)*h*.875],Home:[0,-sh],End:[0,sh]}[e.key];if(d)scroller(t,d[0],d[1]).scrollBy(d[0],d[1])}
   if(e.key==='Tab'){const f=[...document.querySelectorAll('a[href],button,input,select,textarea,[tabindex],[contenteditable]')].filter(x=>x.tabIndex>=0&&!x.disabled);const i=f.indexOf(t);const n=f[(i+(e.shift?-1:1)+f.length)%Math.max(f.length,1)];if(n)n.focus()}}
  t.dispatchEvent(new KeyboardEvent('keyup',init));return out}
 return null};
})();"#;

struct State {
    web: WebView,
    events: UnboundedSender<PageEvent>,
    active: bool,
    /// The last frame sent, to skip identical snapshots.
    last: u64,
    snapping: bool,
}

thread_local! {
    static PAGES: RefCell<HashMap<u64, Rc<RefCell<State>>>> = RefCell::default();
}

/// A page on the WebKitGTK thread; dropping it destroys the view.
pub struct Page {
    id: u64,
}

fn on_thread(f: impl FnOnce() + Send + 'static) -> bool {
    match crate::webhost::gtk_context() {
        Ok(ctx) => {
            ctx.invoke(f);
            true
        }
        Err(_) => false,
    }
}

fn with(id: u64, f: impl FnOnce(&Rc<RefCell<State>>)) {
    PAGES.with(|p| {
        if let Some(s) = p.borrow().get(&id).cloned() {
            f(&s);
        }
    });
}

fn hash(bytes: &[u8]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut h);
    h.finish()
}

/// Takes a snapshot of the visible region and sends it when it changed.
fn snap(state: &Rc<RefCell<State>>) {
    let web = {
        let mut s = state.borrow_mut();
        if s.snapping {
            return;
        }
        s.snapping = true;
        s.web.clone()
    };
    let st = state.clone();
    web.snapshot(
        SnapshotRegion::Visible,
        SnapshotOptions::NONE,
        None::<&webkit6::gio::Cancellable>,
        move |res| {
            let mut s = st.borrow_mut();
            s.snapping = false;
            let Ok(texture) = res else { return };
            let mut dl = gdk::TextureDownloader::new(&texture);
            dl.set_format(gdk::MemoryFormat::B8g8r8a8);
            let (bytes, stride) = dl.download_bytes();
            let (w, h) = (texture.width() as usize, texture.height() as usize);
            let mut bgra = Vec::with_capacity(w * h * 4);
            for row in 0..h {
                bgra.extend_from_slice(&bytes[row * stride..row * stride + w * 4]);
            }
            let digest = hash(&bgra);
            if digest == s.last {
                return;
            }
            s.last = digest;
            let _ = s.events.unbounded_send(PageEvent::Frame {
                width: w as u32,
                height: h as u32,
                bgra,
            });
        },
    );
}

fn snap_soon(id: u64) {
    glib::timeout_add_local_once(Duration::from_millis(30), move || with(id, snap));
}

impl Page {
    /// Creates the view and loads `srcdoc`; `None` without a display.
    pub fn new(srcdoc: &str, events: UnboundedSender<PageEvent>) -> Option<Page> {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        let doc = srcdoc.to_string();
        on_thread(move || {
            let ucm = UserContentManager::new();
            ucm.register_script_message_handler("recur", None);
            for js in [BRIDGE_JS, INPUT_JS] {
                ucm.add_script(&UserScript::new(
                    js,
                    UserContentInjectedFrames::TopFrame,
                    UserScriptInjectionTime::Start,
                    &[],
                    &[],
                ));
            }
            let web = WebView::builder()
                .network_session(&NetworkSession::new_ephemeral())
                .user_content_manager(&ucm)
                .build();
            if let Some(settings) = WebViewExt::settings(&web) {
                settings.set_javascript_can_open_windows_automatically(false);
                settings.set_allow_file_access_from_file_urls(false);
                settings.set_allow_universal_access_from_file_urls(false);
            }
            // No popups, and the document never navigates away from `about:blank`.
            web.connect_create(|_, _| None);
            web.connect_decide_policy(|_, decision, ty| match ty {
                PolicyDecisionType::NavigationAction | PolicyDecisionType::NewWindowAction => {
                    let uri = decision
                        .downcast_ref::<webkit6::NavigationPolicyDecision>()
                        .and_then(|d| d.navigation_action())
                        .and_then(|a| a.request())
                        .and_then(|r| r.uri())
                        .map(|u| u.to_string())
                        .unwrap_or_default();
                    if uri == "about:blank" {
                        decision.use_();
                    } else {
                        decision.ignore();
                    }
                    true
                }
                _ => false,
            });
            let state = Rc::new(RefCell::new(State {
                web: web.clone(),
                events,
                active: false,
                last: 0,
                snapping: false,
            }));
            let st = state.clone();
            ucm.connect_script_message_received(Some("recur"), move |_, value| {
                let _ = st
                    .borrow()
                    .events
                    .unbounded_send(PageEvent::Message(value.to_str().to_string()));
            });
            let st = state.clone();
            web.connect_load_changed(move |_, ev| {
                if ev == webkit6::LoadEvent::Finished {
                    let _ = st
                        .borrow()
                        .events
                        .unbounded_send(PageEvent::Message("\u{0}loaded".into()));
                    snap(&st);
                }
            });
            // Pages animate and change on their own: poll while the preview shows.
            glib::timeout_add_local(Duration::from_millis(100), move || {
                let live = PAGES.with(|p| p.borrow().get(&id).cloned());
                match live {
                    None => glib::ControlFlow::Break,
                    Some(s) => {
                        if s.borrow().active {
                            snap(&s);
                        }
                        glib::ControlFlow::Continue
                    }
                }
            });
            web.load_html(&doc, None);
            PAGES.with(|p| p.borrow_mut().insert(id, state));
        })
        .then_some(Page { id })
    }

    pub fn load(&self, srcdoc: &str, events: UnboundedSender<PageEvent>) {
        let (id, doc) = (self.id, srcdoc.to_string());
        on_thread(move || {
            with(id, |s| {
                let mut s = s.borrow_mut();
                s.events = events;
                s.last = 0;
                s.web.load_html(&doc, None);
            })
        });
    }

    /// Lays the page out at `w` x `h` CSS pixels, rendered at `scale` device pixels each.
    pub fn resize(&self, w: f32, h: f32, scale: f32) {
        let id = self.id;
        on_thread(move || {
            with(id, |s| {
                let web = s.borrow().web.clone();
                web.set_zoom_level(scale as f64);
                let alloc = webkit6::gtk::Allocation::new(
                    0,
                    0,
                    (w * scale).round() as i32,
                    (h * scale).round() as i32,
                );
                web.size_allocate(&alloc, -1);
                snap_soon(id);
            })
        });
    }

    /// Whether the preview shows (snapshots poll only then).
    pub fn set_active(&self, active: bool) {
        let id = self.id;
        on_thread(move || {
            with(id, |s| {
                s.borrow_mut().active = active;
                if active {
                    snap(s);
                }
            })
        });
    }

    /// `iframe.contentWindow.postMessage(data, '*')`.
    pub fn dispatch(&self, data: &Value) {
        let js = format!("window.dispatchEvent(new MessageEvent('message', {{ data: {data} }}));");
        self.eval(js, false);
    }

    /// One input event for [`INPUT_JS`]; its answer comes back as [`PageEvent::Reply`].
    pub fn input(&self, event: &Value) {
        self.eval(format!("window.__recurInput({event})"), true);
    }

    fn eval(&self, js: String, reply: bool) {
        let id = self.id;
        on_thread(move || {
            with(id, |s| {
                let (web, events) = {
                    let s = s.borrow();
                    (s.web.clone(), s.events.clone())
                };
                web.evaluate_javascript(
                    &js,
                    None,
                    None,
                    None::<&webkit6::gio::Cancellable>,
                    move |res| {
                        if reply && let Ok(v) = res {
                            let text = v.to_json(0).map(|s| s.to_string()).unwrap_or_default();
                            if let Ok(v) = serde_json::from_str::<Value>(&text) {
                                let _ = events.unbounded_send(PageEvent::Reply(v));
                            }
                        }
                    },
                );
                snap_soon(id);
            })
        });
    }
}

impl Drop for Page {
    fn drop(&mut self) {
        let id = self.id;
        on_thread(move || {
            PAGES.with(|p| p.borrow_mut().remove(&id));
        });
    }
}
