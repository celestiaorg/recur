//! JavaScriptCore for TypeScript bundle code: macros, prompt modules and completers.
//!
//! The bundle contract is a classic Worker: ECMAScript plus Worker globals, with no
//! `require`, `process` or file access. A [`JsWorker`] owns one thread and one
//! JavaScriptCore context (the system framework on macOS, WebKitGTK's library on
//! Linux), so Recur runs bundle code without node. [`PRELUDE`] installs the Worker
//! globals; the worker thread runs timers, and JavaScriptCore's execution time limit
//! stops a call that times out or is cancelled. A stopped context is dropped, so the
//! next call starts fresh (as killing a Worker does).
//!
//! A worker's boot script defines `async function run(msg)`; [`JsWorker::call`] posts
//! `msg` and resolves with `run`'s result object, or the message it threw.
#![allow(unsafe_code)]

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::ffi::c_void;
use std::ptr::{null, null_mut};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde_json::Value;

use ffi::*;

/// `desktop`'s macro Worker (`WORKER_BOOT`): `{ text, output?, language?, openInPreview? }`.
pub const MACRO_BOOT: &str = r#"
async function run({ code, ctx }) {
  const frozen = Object.freeze(Object.assign({}, ctx, {
    prompts: Object.freeze(Object.assign({}, (ctx && ctx.prompts) || {})),
  }));
  const raw = await load(code, "macro")(frozen);
  let text = "";
  let output = undefined;
  let language = undefined;
  let openInPreview = undefined;
  if (typeof raw === "string") {
    text = raw;
  } else if (raw && typeof raw === "object") {
    text = raw.text != null ? String(raw.text) : "";
    if (raw.output != null) output = String(raw.output);
    if (raw.language != null && String(raw.language).trim()) language = String(raw.language).trim();
    if (typeof raw.openInPreview === "boolean") openInPreview = raw.openInPreview;
  } else if (raw != null) {
    throw new Error("macro must return a string or { text, output?, language?, openInPreview? }");
  }
  return { text, output, language, openInPreview };
}
"#;

/// The prompt-module Worker (`PROMPT_WORKER_BOOT`): `{ prompt, display }`.
pub const PROMPT_BOOT: &str = r#"
async function run({ code, ctx, fallbackName }) {
  const raw = await load(code, "prompt")(Object.freeze(Object.assign({}, ctx)));
  if (!raw || typeof raw !== "object") {
    throw new Error("prompt must return { prompt, display? }");
  }
  const prompt = raw.prompt != null ? String(raw.prompt) : "";
  let display = raw.display != null ? String(raw.display) : "";
  display = display.replace(/[\r\n]+/g, " ").replace(/\s+/g, " ").trim();
  if (!display) display = String(fallbackName || "").replace(/[\r\n]+/g, " ").replace(/\s+/g, " ").trim();
  if (display.length > 120) display = display.slice(0, 119) + "…";
  return { prompt, display };
}
"#;

/// The completer Worker (`COMPLETE_WORKER_BOOT`): `{ hits, message }`.
pub const COMPLETE_BOOT: &str = r#"
async function run({ code, ctx }) {
  const raw = await load(code, "complete")(Object.freeze(Object.assign({}, ctx)));
  const hits = [];
  const list = raw && Array.isArray(raw.hits) ? raw.hits : (Array.isArray(raw) ? raw : []);
  for (const h of list) {
    if (!h || typeof h !== "object") continue;
    const insert = h.insert != null ? String(h.insert) : "";
    const label = h.label != null ? String(h.label) : insert;
    const detail = h.detail != null ? String(h.detail) : "";
    const badge = h.badge != null ? String(h.badge) : "";
    const action = h.action != null ? String(h.action) : "";
    if (!label && !action) continue;
    hits.push({ insert, label, detail, badge, action });
    if (hits.length >= 25) break;
  }
  const message = raw && raw.message != null ? String(raw.message) : "";
  return { hits, message };
}
"#;

/// The timeout for callers that end a call by dropping it on their own deadline.
pub const UNTIL_CANCELLED: Duration = Duration::from_secs(24 * 60 * 60);

/// A call's result and what the code wrote through `console`, one line per call.
#[derive(Debug)]
pub struct JsOutcome {
    pub result: Result<Value, String>,
    pub logs: String,
}

struct Job {
    msg: String,
    timeout: Duration,
    cancel: Arc<AtomicBool>,
    reply: tokio::sync::oneshot::Sender<JsOutcome>,
}

/// One Worker: a thread with a JavaScriptCore context running `boot`.
pub struct JsWorker {
    what: &'static str,
    tx: mpsc::Sender<Job>,
}

impl JsWorker {
    /// `what` ("macro", "prompt", "complete") words the timeout and worker errors.
    pub fn new(what: &'static str, boot: &str) -> Self {
        let (tx, rx) = mpsc::channel::<Job>();
        let boot = boot.to_owned();
        // A thread that fails to start drops `rx`, so every call reports a worker error.
        let _ = std::thread::Builder::new()
            .name(format!("js-{what}"))
            .stack_size(16 << 20)
            .spawn(move || serve(what, &boot, rx));
        Self { what, tx }
    }

    /// Runs `run(msg)`. Dropping the future before it resolves stops the call.
    pub async fn call(&self, msg: &Value, timeout: Duration) -> JsOutcome {
        let (reply, rx) = tokio::sync::oneshot::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_error = || JsOutcome {
            result: Err(format!("{} worker error", self.what)),
            logs: String::new(),
        };
        let job = Job {
            msg: msg.to_string(),
            timeout,
            cancel: cancel.clone(),
            reply,
        };
        if self.tx.send(job).is_err() {
            return worker_error();
        }
        let _stop = CancelOnDrop(cancel);
        rx.await.unwrap_or_else(|_| worker_error())
    }
}

/// Runs `boot` once on a fresh worker.
pub async fn run_once(what: &'static str, boot: &str, msg: &Value, timeout: Duration) -> JsOutcome {
    JsWorker::new(what, boot).call(msg, timeout).await
}

struct CancelOnDrop(Arc<AtomicBool>);

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

fn serve(what: &'static str, boot: &str, rx: mpsc::Receiver<Job>) {
    let mut engine: Option<Engine> = None;
    for job in rx {
        if job.cancel.load(Ordering::SeqCst) {
            continue;
        }
        if engine.is_none() {
            match Engine::new(boot) {
                Ok(e) => engine = Some(e),
                Err(e) => {
                    let _ = job.reply.send(JsOutcome {
                        result: Err(format!("{what} worker error: {e}")),
                        logs: String::new(),
                    });
                    continue;
                }
            }
        }
        let Some(eng) = engine.as_mut() else {
            continue;
        };
        let (result, stopped) = eng.run(what, &job);
        if stopped {
            engine = None;
        }
        let logs = STATE.with_borrow_mut(|s| std::mem::take(&mut s.logs));
        let _ = job.reply.send(JsOutcome { result, logs });
    }
}

#[derive(Clone, Copy)]
enum Stop {
    Timeout,
    Cancelled,
}

struct Timer {
    due: Instant,
    every: Option<Duration>,
    func: JSObjectRef,
}

#[derive(Default)]
struct State {
    timers: BTreeMap<u64, Timer>,
    next_timer: u64,
    done: Option<String>,
    logs: String,
    deadline: Option<Instant>,
    cancel: Option<Arc<AtomicBool>>,
    stop: Option<Stop>,
}

impl State {
    fn should_stop(&mut self) -> Option<Stop> {
        if self.stop.is_none() {
            if self
                .cancel
                .as_ref()
                .is_some_and(|c| c.load(Ordering::SeqCst))
            {
                self.stop = Some(Stop::Cancelled);
            } else if self.deadline.is_some_and(|d| Instant::now() >= d) {
                self.stop = Some(Stop::Timeout);
            }
        }
        self.stop
    }
}

thread_local! {
    static STATE: RefCell<State> = RefCell::default();
    static START: Instant = Instant::now();
}

/// A context group (its own execution time limit) with one global context.
struct Engine {
    group: JSContextGroupRef,
    ctx: JSGlobalContextRef,
    entry: JSObjectRef,
}

/// How often JavaScriptCore asks [`should_terminate`] while script runs.
const WATCHDOG: f64 = 0.05;

impl Engine {
    fn new(boot: &str) -> Result<Self, String> {
        // SAFETY: every ref below is created here, used on this thread only and released
        // in `Drop`; values kept across calls (`entry`, timer callbacks) are protected.
        unsafe {
            let group = JSContextGroupCreate();
            JSContextGroupSetExecutionTimeLimit(
                group,
                WATCHDOG,
                Some(should_terminate),
                group as *mut c_void,
            );
            let ctx = JSGlobalContextCreateInGroup(group, null_mut());
            let mut eng = Engine {
                group,
                ctx,
                entry: null_mut(),
            };
            let natives = JSObjectMake(ctx, null_mut(), null_mut());
            for (name, f) in NATIVES {
                let n = JsString::new(name);
                let func = JSObjectMakeFunctionWithCallback(ctx, n.0, Some(*f));
                JSObjectSetProperty(ctx, natives, n.0, func, 0, null_mut());
            }
            let setup = eng.eval(PRELUDE)?;
            let install = eng.call(setup, &[natives as JSValueRef])?;
            let entry = eng.call(install, &[make_string(ctx, boot)])?;
            JSValueProtect(ctx, entry);
            eng.entry = entry as JSObjectRef;
            Ok(eng)
        }
    }

    unsafe fn eval(&self, src: &str) -> Result<JSObjectRef, String> {
        let script = JsString::new(src);
        let mut exc: JSValueRef = null();
        // SAFETY: `self.ctx` is live; `script` outlives the call.
        let v =
            unsafe { JSEvaluateScript(self.ctx, script.0, null_mut(), null_mut(), 1, &mut exc) };
        self.object(v, exc)
    }

    unsafe fn call(&self, f: JSObjectRef, args: &[JSValueRef]) -> Result<JSObjectRef, String> {
        let mut exc: JSValueRef = null();
        // SAFETY: `f` and `args` belong to `self.ctx`.
        let v = unsafe {
            JSObjectCallAsFunction(self.ctx, f, null_mut(), args.len(), args.as_ptr(), &mut exc)
        };
        self.object(v, exc)
    }

    fn object(&self, v: JSValueRef, exc: JSValueRef) -> Result<JSObjectRef, String> {
        if !exc.is_null() {
            // SAFETY: `exc` was just produced by `self.ctx`.
            return Err(unsafe { value_string(self.ctx, exc) });
        }
        // SAFETY: as above.
        if v.is_null() || !unsafe { JSValueIsObject(self.ctx, v) } {
            return Err("expected a function".into());
        }
        Ok(v as JSObjectRef)
    }

    /// Runs one job to its result. `true` when the job was stopped and the context must go.
    fn run(&mut self, what: &str, job: &Job) -> (Result<Value, String>, bool) {
        let deadline = Instant::now() + job.timeout;
        STATE.with_borrow_mut(|s| {
            s.done = None;
            s.stop = None;
            s.deadline = Some(deadline);
            s.cancel = Some(job.cancel.clone());
        });
        // SAFETY: `entry` is protected and belongs to `self.ctx`.
        let first = unsafe {
            let arg = make_string(self.ctx, &job.msg);
            self.call_void(self.entry, &[arg])
        };
        let result = loop {
            let (stop, done) = STATE.with_borrow_mut(|s| (s.should_stop(), s.done.take()));
            if let Some(stop) = stop {
                break Err(stop);
            }
            if let Some(done) = done {
                break Ok(parse_done(what, &done));
            }
            if let Err(e) = &first {
                break Ok(Err(format!("{what} worker error: {e}")));
            }
            let next = STATE.with_borrow(|s| {
                s.timers
                    .iter()
                    .min_by_key(|(_, t)| t.due)
                    .map(|(id, t)| (*id, t.due))
            });
            let now = Instant::now();
            match next {
                Some((id, due)) if due <= now => self.fire(id),
                _ => {
                    let wake = next.map_or(deadline, |(_, due)| due.min(deadline));
                    let nap = wake
                        .saturating_duration_since(now)
                        .min(Duration::from_millis(20));
                    std::thread::sleep(nap);
                }
            }
        };
        self.clear_timers();
        match result {
            Ok(r) => (r, false),
            Err(Stop::Timeout) => (Err(format!("{what} timed out")), true),
            Err(Stop::Cancelled) => (Err(format!("{what} cancelled")), true),
        }
    }

    fn fire(&mut self, id: u64) {
        let Some(func) = STATE.with_borrow_mut(|s| {
            let t = s.timers.get_mut(&id)?;
            let func = t.func;
            match t.every {
                Some(every) => t.due = Instant::now() + every,
                None => {
                    s.timers.remove(&id);
                }
            }
            Some(func)
        }) else {
            return;
        };
        // SAFETY: `func` is protected until its timer is cleared, below or in `clear_timers`.
        unsafe {
            if let Err(e) = self.call_void(func, &[]) {
                log_line(&format!("Uncaught {e}"));
            }
            if STATE.with_borrow(|s| !s.timers.contains_key(&id)) {
                JSValueUnprotect(self.ctx, func);
            }
        }
    }

    unsafe fn call_void(&self, f: JSObjectRef, args: &[JSValueRef]) -> Result<(), String> {
        let mut exc: JSValueRef = null();
        // SAFETY: `f` and `args` belong to `self.ctx`.
        unsafe {
            JSObjectCallAsFunction(self.ctx, f, null_mut(), args.len(), args.as_ptr(), &mut exc)
        };
        if exc.is_null() {
            Ok(())
        } else {
            // SAFETY: as above.
            Err(unsafe { value_string(self.ctx, exc) })
        }
    }

    fn clear_timers(&mut self) {
        let timers = STATE.with_borrow_mut(|s| std::mem::take(&mut s.timers));
        for t in timers.into_values() {
            // SAFETY: every stored timer callback was protected by `set_timer`.
            unsafe { JSValueUnprotect(self.ctx, t.func) };
        }
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.clear_timers();
        // SAFETY: releases what `Engine::new` created, once.
        unsafe {
            if !self.entry.is_null() {
                JSValueUnprotect(self.ctx, self.entry);
            }
            JSGlobalContextRelease(self.ctx);
            JSContextGroupRelease(self.group);
        }
    }
}

fn parse_done(what: &str, done: &str) -> Result<Value, String> {
    let v: Value = serde_json::from_str(done).map_err(|e| format!("{what} worker error: {e}"))?;
    if v.get("ok") == Some(&Value::Bool(true)) {
        return Ok(v);
    }
    match v.get("error").and_then(Value::as_str) {
        Some(msg) if !msg.is_empty() => Err(msg.to_owned()),
        _ => Err(format!("{what} failed")),
    }
}

fn log_line(line: &str) {
    STATE.with_borrow_mut(|s| {
        s.logs.push_str(line);
        s.logs.push('\n');
    });
}

/// Stops running script once the call is cancelled or past its deadline. JavaScriptCore asks
/// once per limit and does not re-arm after a `false`, so this re-arms (`data` is the group).
unsafe extern "C" fn should_terminate(_ctx: JSContextRef, data: *mut c_void) -> bool {
    let stop = STATE.with_borrow_mut(|s| s.should_stop().is_some());
    if !stop {
        // SAFETY: `data` is the live group this callback was installed on.
        unsafe {
            JSContextGroupSetExecutionTimeLimit(
                data as JSContextGroupRef,
                WATCHDOG,
                Some(should_terminate),
                data,
            )
        };
    }
    stop
}

// Natives reach the prelude as one argument object, never as globals.
type Native = unsafe extern "C" fn(
    JSContextRef,
    JSObjectRef,
    JSObjectRef,
    usize,
    *const JSValueRef,
    *mut JSValueRef,
) -> JSValueRef;

const NATIVES: &[(&str, Native)] = &[
    ("log", native_log),
    ("setTimer", native_set_timer),
    ("clearTimer", native_clear_timer),
    ("urlParse", native_url_parse),
    ("urlSet", native_url_set),
    ("formParse", native_form_parse),
    ("formSerialize", native_form_serialize),
    ("random", native_random),
    ("now", native_now),
    ("done", native_done),
];

/// Declares a native: `|ctx, args, exc| body`, with `args` as a slice.
macro_rules! native {
    ($name:ident, |$ctx:ident, $args:ident, $exc:ident| $body:block) => {
        unsafe extern "C" fn $name(
            $ctx: JSContextRef,
            _f: JSObjectRef,
            _this: JSObjectRef,
            argc: usize,
            argv: *const JSValueRef,
            $exc: *mut JSValueRef,
        ) -> JSValueRef {
            // SAFETY: JavaScriptCore passes `argc` live values for this call.
            let $args: &[JSValueRef] = if argc == 0 || argv.is_null() {
                &[]
            } else {
                unsafe { std::slice::from_raw_parts(argv, argc) }
            };
            #[allow(unused_unsafe)]
            unsafe {
                $body
            }
        }
    };
}

unsafe fn arg_string(ctx: JSContextRef, args: &[JSValueRef], i: usize) -> String {
    match args.get(i) {
        // SAFETY: `v` is a live argument of the current call.
        Some(v) => unsafe { value_string(ctx, *v) },
        None => String::new(),
    }
}

unsafe fn arg_number(ctx: JSContextRef, args: &[JSValueRef], i: usize) -> f64 {
    match args.get(i) {
        // SAFETY: as above.
        Some(v) => unsafe { JSValueToNumber(ctx, *v, null_mut()) },
        None => f64::NAN,
    }
}

native!(native_log, |ctx, args, _exc| {
    log_line(&arg_string(ctx, args, 0));
    JSValueMakeUndefined(ctx)
});

native!(native_set_timer, |ctx, args, exc| {
    let Some(&f) = args.first() else {
        return throw(ctx, exc, "callback is not a function");
    };
    if !JSValueIsObject(ctx, f) {
        return throw(ctx, exc, "callback is not a function");
    }
    let ms = arg_number(ctx, args, 1);
    let delay = Duration::from_secs_f64(if ms.is_finite() && ms > 0.0 {
        ms / 1000.0
    } else {
        0.0
    });
    let repeat = args.get(2).is_some_and(|v| JSValueToBoolean(ctx, *v));
    JSValueProtect(ctx, f);
    let id = STATE.with_borrow_mut(|s| {
        s.next_timer += 1;
        let id = s.next_timer;
        s.timers.insert(
            id,
            Timer {
                due: Instant::now() + delay,
                every: repeat.then_some(delay.max(Duration::from_millis(1))),
                func: f as JSObjectRef,
            },
        );
        id
    });
    JSValueMakeNumber(ctx, id as f64)
});

native!(native_clear_timer, |ctx, args, _exc| {
    let id = arg_number(ctx, args, 0);
    if id.is_finite()
        && id >= 1.0
        && let Some(t) = STATE.with_borrow_mut(|s| s.timers.remove(&(id as u64)))
    {
        JSValueUnprotect(ctx, t.func);
    }
    JSValueMakeUndefined(ctx)
});

fn url_json(u: &url::Url) -> String {
    use url::quirks as q;
    serde_json::json!({
        "href": q::href(u),
        "origin": q::origin(u),
        "protocol": q::protocol(u),
        "username": q::username(u),
        "password": q::password(u),
        "host": q::host(u),
        "hostname": q::hostname(u),
        "port": q::port(u),
        "pathname": q::pathname(u),
        "search": q::search(u),
        "hash": q::hash(u),
    })
    .to_string()
}

native!(native_url_parse, |ctx, args, _exc| {
    let input = arg_string(ctx, args, 0);
    let has_base = args.get(2).is_some_and(|v| JSValueToBoolean(ctx, *v));
    let parsed = if has_base {
        url::Url::parse(&arg_string(ctx, args, 1)).and_then(|b| b.join(&input))
    } else {
        url::Url::parse(&input)
    };
    make_string(ctx, &parsed.map(|u| url_json(&u)).unwrap_or_default())
});

native!(native_url_set, |ctx, args, _exc| {
    use url::quirks as q;
    let Ok(mut u) = url::Url::parse(&arg_string(ctx, args, 0)) else {
        return make_string(ctx, "");
    };
    let v = arg_string(ctx, args, 2);
    match arg_string(ctx, args, 1).as_str() {
        "protocol" => {
            let _ = q::set_protocol(&mut u, &v);
        }
        "username" => {
            let _ = q::set_username(&mut u, &v);
        }
        "password" => {
            let _ = q::set_password(&mut u, &v);
        }
        "host" => {
            let _ = q::set_host(&mut u, &v);
        }
        "hostname" => {
            let _ = q::set_hostname(&mut u, &v);
        }
        "port" => {
            let _ = q::set_port(&mut u, &v);
        }
        "pathname" => q::set_pathname(&mut u, &v),
        "search" => q::set_search(&mut u, &v),
        "hash" => q::set_hash(&mut u, &v),
        _ => {}
    }
    make_string(ctx, &url_json(&u))
});

native!(native_form_parse, |ctx, args, _exc| {
    let s = arg_string(ctx, args, 0);
    let pairs: Vec<(String, String)> = url::form_urlencoded::parse(s.as_bytes())
        .into_owned()
        .collect();
    make_string(ctx, &serde_json::to_string(&pairs).unwrap_or_default())
});

native!(native_form_serialize, |ctx, args, _exc| {
    let pairs: Vec<(String, String)> =
        serde_json::from_str(&arg_string(ctx, args, 0)).unwrap_or_default();
    let out = url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs(pairs)
        .finish();
    make_string(ctx, &out)
});

native!(native_random, |ctx, args, exc| {
    let n = arg_number(ctx, args, 0);
    let n = if n.is_finite() && n > 0.0 {
        (n as usize).min(65536)
    } else {
        0
    };
    let mut buf = vec![0u8; n];
    if getrandom::fill(&mut buf).is_err() {
        return throw(ctx, exc, "no randomness available");
    }
    make_string(ctx, &recur_base::hex(buf))
});

native!(native_now, |ctx, _args, _exc| {
    JSValueMakeNumber(ctx, START.with(|s| s.elapsed().as_secs_f64() * 1000.0))
});

native!(native_done, |ctx, args, _exc| {
    let json = arg_string(ctx, args, 0);
    STATE.with_borrow_mut(|s| s.done = Some(json));
    JSValueMakeUndefined(ctx)
});

/// Installs the Worker globals and returns `install(boot)`, which evaluates `boot` with
/// `load` in scope and returns the entry the engine calls with each message's JSON.
const PRELUDE: &str = r#"(function (natives) {
"use strict";
const { log, setTimer, clearTimer, urlParse, urlSet, formParse, formSerialize, random, now, done } = natives;
const g = globalThis;
const define = (name, value) =>
  Object.defineProperty(g, name, { value, writable: true, configurable: true, enumerable: false });
const domError = (message, name) => {
  const e = new Error(message);
  e.name = name;
  return e;
};

const show = (v) => {
  if (typeof v === "string") return v;
  if (v instanceof Error) return v.stack ? `${v.name}: ${v.message}\n${v.stack}` : `${v.name}: ${v.message}`;
  if (typeof v === "function") return `[Function: ${v.name || "(anonymous)"}]`;
  try {
    const s = JSON.stringify(v);
    return s === undefined ? String(v) : s;
  } catch {
    return String(v);
  }
};
const write = (...a) => log(a.map(show).join(" "));
define("console", Object.freeze({ log: write, info: write, debug: write, warn: write, error: write, trace: write }));

define("setTimeout", (fn, ms, ...args) => setTimer(() => fn(...args), Number(ms) || 0, false));
define("setInterval", (fn, ms, ...args) => setTimer(() => fn(...args), Number(ms) || 0, true));
define("clearTimeout", (id) => clearTimer(Number(id) || 0));
define("clearInterval", (id) => clearTimer(Number(id) || 0));
if (typeof g.queueMicrotask !== "function") define("queueMicrotask", (fn) => { Promise.resolve().then(fn); });
const origin = Date.now() - now();
define("performance", Object.freeze({ now: () => now(), timeOrigin: origin }));

const B64 = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
define("btoa", (data) => {
  const s = String(data);
  let out = "";
  for (let i = 0; i < s.length; i += 3) {
    const a = s.charCodeAt(i), b = s.charCodeAt(i + 1), c = s.charCodeAt(i + 2);
    if (a > 255 || b > 255 || c > 255) throw domError("Invalid character", "InvalidCharacterError");
    const n = (a << 16) | ((b || 0) << 8) | (c || 0);
    out += B64[(n >> 18) & 63] + B64[(n >> 12) & 63] +
      (i + 1 < s.length ? B64[(n >> 6) & 63] : "=") + (i + 2 < s.length ? B64[n & 63] : "=");
  }
  return out;
});
define("atob", (data) => {
  let s = String(data).replace(/[\t\n\f\r ]+/g, "");
  if (s.length % 4 === 0) s = s.replace(/==?$/, "");
  if (s.length % 4 === 1 || /[^A-Za-z0-9+/]/.test(s)) {
    throw domError("The string to be decoded is not correctly encoded.", "InvalidCharacterError");
  }
  let out = "", buf = 0, bits = 0;
  for (const ch of s) {
    buf = ((buf << 6) | B64.indexOf(ch)) & 0xffffff;
    bits += 6;
    if (bits >= 8) {
      bits -= 8;
      out += String.fromCharCode((buf >> bits) & 255);
    }
  }
  return out;
});

class TextEncoder {
  get encoding() { return "utf-8"; }
  encode(input = "") {
    const s = String(input);
    const out = [];
    for (let i = 0; i < s.length; i++) {
      let c = s.charCodeAt(i);
      if (c >= 0xd800 && c <= 0xdbff && i + 1 < s.length) {
        const d = s.charCodeAt(i + 1);
        if (d >= 0xdc00 && d <= 0xdfff) {
          c = 0x10000 + ((c - 0xd800) << 10) + (d - 0xdc00);
          i++;
        }
      }
      if (c >= 0xd800 && c <= 0xdfff) c = 0xfffd;
      if (c < 0x80) out.push(c);
      else if (c < 0x800) out.push(0xc0 | (c >> 6), 0x80 | (c & 63));
      else if (c < 0x10000) out.push(0xe0 | (c >> 12), 0x80 | ((c >> 6) & 63), 0x80 | (c & 63));
      else out.push(0xf0 | (c >> 18), 0x80 | ((c >> 12) & 63), 0x80 | ((c >> 6) & 63), 0x80 | (c & 63));
    }
    return new Uint8Array(out);
  }
  encodeInto(input, dest) {
    const bytes = this.encode(input);
    const n = Math.min(bytes.length, dest.length);
    dest.set(bytes.subarray(0, n));
    return { read: n === bytes.length ? String(input).length : undefined, written: n };
  }
}
const bytesOf = (input) => {
  if (input instanceof ArrayBuffer) return new Uint8Array(input);
  if (ArrayBuffer.isView(input)) return new Uint8Array(input.buffer, input.byteOffset, input.byteLength);
  throw new TypeError("The input must be an ArrayBuffer or ArrayBufferView");
};
class TextDecoder {
  constructor(label = "utf-8", options = {}) {
    const l = String(label).trim().toLowerCase();
    if (l !== "utf-8" && l !== "utf8" && l !== "unicode-1-1-utf-8") {
      throw new RangeError(`The "${label}" encoding is not supported`);
    }
    this.fatal = !!(options && options.fatal);
    this.ignoreBOM = !!(options && options.ignoreBOM);
  }
  get encoding() { return "utf-8"; }
  decode(input) {
    if (input === undefined) return "";
    const b = bytesOf(input);
    let out = "", i = 0;
    if (!this.ignoreBOM && b[0] === 0xef && b[1] === 0xbb && b[2] === 0xbf) i = 3;
    const bad = () => {
      if (this.fatal) throw new TypeError("The encoded data was not valid utf-8");
      out += "�";
    };
    while (i < b.length) {
      const c = b[i];
      if (c < 0x80) { out += String.fromCharCode(c); i++; continue; }
      let n, cp, lo = 0x80, hi = 0xbf;
      if (c >= 0xc2 && c <= 0xdf) { n = 1; cp = c & 0x1f; }
      else if (c >= 0xe0 && c <= 0xef) { n = 2; cp = c & 0x0f; if (c === 0xe0) lo = 0xa0; if (c === 0xed) hi = 0x9f; }
      else if (c >= 0xf0 && c <= 0xf4) { n = 3; cp = c & 0x07; if (c === 0xf0) lo = 0x90; if (c === 0xf4) hi = 0x8f; }
      else { bad(); i++; continue; }
      let j = 1;
      for (; j <= n; j++) {
        const d = b[i + j];
        if (d === undefined || d < lo || d > hi) break;
        lo = 0x80; hi = 0xbf;
        cp = (cp << 6) | (d & 0x3f);
      }
      if (j <= n) { bad(); i += j; continue; }
      out += String.fromCodePoint(cp);
      i += n + 1;
    }
    return out;
  }
}
define("TextEncoder", TextEncoder);
define("TextDecoder", TextDecoder);

const urls = new WeakMap();
const parseUrl = (input, base) => {
  const r = base === undefined ? urlParse(String(input), "", false) : urlParse(String(input), String(base), true);
  if (!r) throw new TypeError(`Invalid URL: ${input}`);
  return JSON.parse(r);
};
class URLSearchParams {
  constructor(init = "") {
    this._list = [];
    this._url = null;
    if (init instanceof URLSearchParams) this._list = init._list.map(([k, v]) => [k, v]);
    else if (init && typeof init === "object") {
      if (typeof init[Symbol.iterator] === "function") {
        for (const pair of init) {
          const p = [...pair];
          if (p.length !== 2) throw new TypeError("Each query pair must be an iterable [name, value] tuple");
          this._list.push([String(p[0]), String(p[1])]);
        }
      } else for (const k of Object.keys(init)) this._list.push([k, String(init[k])]);
    } else {
      const s = String(init);
      this._list = JSON.parse(formParse(s.startsWith("?") ? s.slice(1) : s));
    }
  }
  _changed() {
    if (this._url) {
      const p = urls.get(this._url);
      const q = this.toString();
      urls.set(this._url, JSON.parse(urlSet(p.href, "search", q)));
    }
  }
  get size() { return this._list.length; }
  append(k, v) { this._list.push([String(k), String(v)]); this._changed(); }
  delete(k, v) {
    k = String(k);
    this._list = this._list.filter(([a, b]) => a !== k || (v !== undefined && b !== String(v)));
    this._changed();
  }
  get(k) { const e = this._list.find(([a]) => a === String(k)); return e ? e[1] : null; }
  getAll(k) { return this._list.filter(([a]) => a === String(k)).map(([, v]) => v); }
  has(k, v) { return this._list.some(([a, b]) => a === String(k) && (v === undefined || b === String(v))); }
  set(k, v) {
    k = String(k);
    const i = this._list.findIndex(([a]) => a === k);
    if (i < 0) this._list.push([k, String(v)]);
    else {
      this._list[i][1] = String(v);
      this._list = this._list.filter(([a], j) => a !== k || j === i);
    }
    this._changed();
  }
  sort() {
    this._list = this._list.map((e, i) => [e, i]).sort((x, y) => (x[0][0] < y[0][0] ? -1 : x[0][0] > y[0][0] ? 1 : x[1] - y[1])).map(([e]) => e);
    this._changed();
  }
  forEach(fn, thisArg) { for (const [k, v] of this._list) fn.call(thisArg, v, k, this); }
  keys() { return this._list.map(([k]) => k)[Symbol.iterator](); }
  values() { return this._list.map(([, v]) => v)[Symbol.iterator](); }
  entries() { return this._list.map(([k, v]) => [k, v])[Symbol.iterator](); }
  [Symbol.iterator]() { return this.entries(); }
  toString() { return formSerialize(JSON.stringify(this._list)); }
}
class URL {
  constructor(url, base) { urls.set(this, parseUrl(url, base)); }
  static canParse(url, base) {
    try { parseUrl(url, base); return true; } catch { return false; }
  }
  static parse(url, base) {
    try { return new URL(url, base); } catch { return null; }
  }
  get href() { return urls.get(this).href; }
  set href(v) { urls.set(this, parseUrl(v)); this._sync(); }
  get origin() { return urls.get(this).origin; }
  get searchParams() {
    if (!this._params) {
      this._params = new URLSearchParams(this.search);
      this._params._url = this;
    }
    return this._params;
  }
  _sync() {
    if (this._params) this._params._list = new URLSearchParams(this.search)._list;
  }
  toString() { return this.href; }
  toJSON() { return this.href; }
}
for (const name of ["protocol", "username", "password", "host", "hostname", "port", "pathname", "search", "hash"]) {
  Object.defineProperty(URL.prototype, name, {
    get() { return urls.get(this)[name]; },
    set(v) {
      const r = urlSet(urls.get(this).href, name, String(v));
      if (r) urls.set(this, JSON.parse(r));
      if (name === "search") this._sync();
    },
    configurable: true,
  });
}
define("URL", URL);
define("URLSearchParams", URLSearchParams);

const getRandomValues = (arr) => {
  if (!ArrayBuffer.isView(arr) || arr instanceof DataView || arr instanceof Float32Array || arr instanceof Float64Array) {
    throw domError("The data argument must be an integer-type TypedArray", "TypeMismatchError");
  }
  if (arr.byteLength > 65536) throw domError("The ArrayBufferView's byte length exceeds 65536", "QuotaExceededError");
  const hex = random(arr.byteLength);
  const bytes = new Uint8Array(arr.buffer, arr.byteOffset, arr.byteLength);
  for (let i = 0; i < bytes.length; i++) bytes[i] = parseInt(hex.substr(i * 2, 2), 16);
  return arr;
};
const randomUUID = () => {
  const b = getRandomValues(new Uint8Array(16));
  b[6] = (b[6] & 0x0f) | 0x40;
  b[8] = (b[8] & 0x3f) | 0x80;
  const h = Array.from(b, (x) => x.toString(16).padStart(2, "0")).join("");
  return `${h.slice(0, 8)}-${h.slice(8, 12)}-${h.slice(12, 16)}-${h.slice(16, 20)}-${h.slice(20)}`;
};
define("crypto", Object.freeze({ getRandomValues, randomUUID }));

// simplify: plain data, Date, RegExp, Map, Set, typed arrays and errors; no transfer list.
define("structuredClone", (value) => {
  const seen = new Map();
  const clone = (v) => {
    if (typeof v === "function" || typeof v === "symbol") throw domError(`${String(v)} could not be cloned.`, "DataCloneError");
    if (typeof v !== "object" || v === null) return v;
    if (seen.has(v)) return seen.get(v);
    let out;
    if (v instanceof Date) out = new Date(v.getTime());
    else if (v instanceof RegExp) out = new RegExp(v.source, v.flags);
    else if (v instanceof ArrayBuffer) out = v.slice(0);
    else if (ArrayBuffer.isView(v) && !(v instanceof DataView)) out = new v.constructor(v);
    else if (v instanceof Error) out = new (g[v.name] || Error)(v.message);
    else if (v instanceof Map) {
      out = new Map();
      seen.set(v, out);
      for (const [k, x] of v) out.set(clone(k), clone(x));
      return out;
    } else if (v instanceof Set) {
      out = new Set();
      seen.set(v, out);
      for (const x of v) out.add(clone(x));
      return out;
    } else {
      out = Array.isArray(v) ? [] : {};
      seen.set(v, out);
      for (const k of Object.keys(v)) out[k] = clone(v[k]);
      return out;
    }
    seen.set(v, out);
    return out;
  };
  return clone(value);
});

class AbortSignal {
  constructor() { this.aborted = false; this.reason = undefined; this.onabort = null; this._listeners = []; }
  addEventListener(type, fn) { if (type === "abort") this._listeners.push(fn); }
  removeEventListener(type, fn) { this._listeners = this._listeners.filter((f) => f !== fn); }
  throwIfAborted() { if (this.aborted) throw this.reason; }
  static abort(reason) { const c = new AbortController(); c.abort(reason); return c.signal; }
}
class AbortController {
  constructor() { this.signal = new AbortSignal(); }
  abort(reason = domError("This operation was aborted", "AbortError")) {
    const s = this.signal;
    if (s.aborted) return;
    s.aborted = true;
    s.reason = reason;
    const ev = { type: "abort", target: s };
    if (typeof s.onabort === "function") s.onabort(ev);
    for (const fn of s._listeners) fn(ev);
  }
}
define("AbortController", AbortController);
define("AbortSignal", AbortSignal);
define("self", g);

const message = (e) => (e && e.message ? String(e.message) : String(e));
const load = (code, what) => (0, eval)(
  "(async function (ctx) {\n" +
  "const module = { exports: {} };\n" +
  "const exports = module.exports;\n" +
  String(code) + "\n" +
  "const __fn = module.exports.default || module.exports;\n" +
  "if (typeof __fn !== 'function') throw new Error('" + what + " must export default function (ctx) { … }');\n" +
  "return await __fn(ctx);\n})",
);
return function install(boot) {
  const run = new Function("load", String(boot) + "\nreturn run;")(load);
  return (json) => {
    let msg;
    try {
      msg = JSON.parse(json);
    } catch (e) {
      done(JSON.stringify({ ok: false, error: message(e) }));
      return;
    }
    Promise.resolve().then(() => run(msg)).then(
      (r) => done(JSON.stringify(Object.assign({ ok: true }, r))),
      (e) => done(JSON.stringify({ ok: false, error: message(e) })),
    );
  };
};
})"#;

struct JsString(JSStringRef);

impl JsString {
    fn new(s: &str) -> Self {
        let units: Vec<u16> = s.encode_utf16().collect();
        // SAFETY: `units` is valid for `len` UTF-16 code units; JavaScriptCore copies them.
        Self(unsafe { JSStringCreateWithCharacters(units.as_ptr(), units.len()) })
    }
}

impl Drop for JsString {
    fn drop(&mut self) {
        // SAFETY: created by `JSStringCreateWithCharacters`, released once.
        unsafe { JSStringRelease(self.0) }
    }
}

unsafe fn value_string(ctx: JSContextRef, v: JSValueRef) -> String {
    // SAFETY: `v` is a live value of `ctx`; the copy is released below.
    unsafe {
        let s = JSValueToStringCopy(ctx, v, null_mut());
        if s.is_null() {
            return String::new();
        }
        let len = JSStringGetLength(s);
        let out = if len == 0 {
            String::new()
        } else {
            String::from_utf16_lossy(std::slice::from_raw_parts(JSStringGetCharactersPtr(s), len))
        };
        JSStringRelease(s);
        out
    }
}

unsafe fn make_string(ctx: JSContextRef, s: &str) -> JSValueRef {
    let js = JsString::new(s);
    // SAFETY: JavaScriptCore retains the string for the value.
    unsafe { JSValueMakeString(ctx, js.0) }
}

unsafe fn throw(ctx: JSContextRef, exc: *mut JSValueRef, msg: &str) -> JSValueRef {
    // SAFETY: `exc` is the out-pointer of the current native call.
    unsafe {
        let m = make_string(ctx, msg);
        let e = JSObjectMakeError(ctx, 1, &m, null_mut());
        if !exc.is_null() {
            *exc = e;
        }
        JSValueMakeUndefined(ctx)
    }
}

mod ffi {
    #![allow(non_snake_case, non_camel_case_types)]
    use std::ffi::c_void;

    #[repr(C)]
    pub struct OpaqueJSContextGroup([u8; 0]);
    #[repr(C)]
    pub struct OpaqueJSContext([u8; 0]);
    #[repr(C)]
    pub struct OpaqueJSString([u8; 0]);
    #[repr(C)]
    pub struct OpaqueJSClass([u8; 0]);
    #[repr(C)]
    pub struct OpaqueJSValue([u8; 0]);

    pub type JSContextGroupRef = *const OpaqueJSContextGroup;
    pub type JSContextRef = *const OpaqueJSContext;
    pub type JSGlobalContextRef = *mut OpaqueJSContext;
    pub type JSStringRef = *mut OpaqueJSString;
    pub type JSClassRef = *mut OpaqueJSClass;
    pub type JSValueRef = *const OpaqueJSValue;
    pub type JSObjectRef = *mut OpaqueJSValue;

    pub type JSObjectCallAsFunctionCallback = unsafe extern "C" fn(
        JSContextRef,
        JSObjectRef,
        JSObjectRef,
        usize,
        *const JSValueRef,
        *mut JSValueRef,
    ) -> JSValueRef;
    pub type JSShouldTerminateCallback = unsafe extern "C" fn(JSContextRef, *mut c_void) -> bool;

    // `JSContextGroupSetExecutionTimeLimit` is exported by both libraries; Apple declares
    // it in JSContextRefPrivate.h.
    #[cfg_attr(target_os = "macos", link(name = "JavaScriptCore", kind = "framework"))]
    #[cfg_attr(not(target_os = "macos"), link(name = "javascriptcoregtk-6.0"))]
    unsafe extern "C" {
        pub fn JSContextGroupCreate() -> JSContextGroupRef;
        pub fn JSContextGroupRelease(group: JSContextGroupRef);
        pub fn JSContextGroupSetExecutionTimeLimit(
            group: JSContextGroupRef,
            limit: f64,
            callback: Option<JSShouldTerminateCallback>,
            context: *mut c_void,
        );
        pub fn JSGlobalContextCreateInGroup(
            group: JSContextGroupRef,
            class: JSClassRef,
        ) -> JSGlobalContextRef;
        pub fn JSGlobalContextRelease(ctx: JSGlobalContextRef);
        pub fn JSStringCreateWithCharacters(chars: *const u16, len: usize) -> JSStringRef;
        pub fn JSStringGetLength(s: JSStringRef) -> usize;
        pub fn JSStringGetCharactersPtr(s: JSStringRef) -> *const u16;
        pub fn JSStringRelease(s: JSStringRef);
        pub fn JSEvaluateScript(
            ctx: JSContextRef,
            script: JSStringRef,
            this: JSObjectRef,
            url: JSStringRef,
            line: i32,
            exc: *mut JSValueRef,
        ) -> JSValueRef;
        pub fn JSValueIsObject(ctx: JSContextRef, v: JSValueRef) -> bool;
        pub fn JSValueToBoolean(ctx: JSContextRef, v: JSValueRef) -> bool;
        pub fn JSValueToNumber(ctx: JSContextRef, v: JSValueRef, exc: *mut JSValueRef) -> f64;
        pub fn JSValueToStringCopy(
            ctx: JSContextRef,
            v: JSValueRef,
            exc: *mut JSValueRef,
        ) -> JSStringRef;
        pub fn JSValueMakeString(ctx: JSContextRef, s: JSStringRef) -> JSValueRef;
        pub fn JSValueMakeNumber(ctx: JSContextRef, n: f64) -> JSValueRef;
        pub fn JSValueMakeUndefined(ctx: JSContextRef) -> JSValueRef;
        pub fn JSValueProtect(ctx: JSContextRef, v: JSValueRef);
        pub fn JSValueUnprotect(ctx: JSContextRef, v: JSValueRef);
        pub fn JSObjectMake(ctx: JSContextRef, class: JSClassRef, data: *mut c_void)
        -> JSObjectRef;
        pub fn JSObjectMakeFunctionWithCallback(
            ctx: JSContextRef,
            name: JSStringRef,
            callback: Option<JSObjectCallAsFunctionCallback>,
        ) -> JSObjectRef;
        pub fn JSObjectMakeError(
            ctx: JSContextRef,
            argc: usize,
            argv: *const JSValueRef,
            exc: *mut JSValueRef,
        ) -> JSObjectRef;
        pub fn JSObjectSetProperty(
            ctx: JSContextRef,
            obj: JSObjectRef,
            name: JSStringRef,
            v: JSValueRef,
            attrs: u32,
            exc: *mut JSValueRef,
        );
        pub fn JSObjectCallAsFunction(
            ctx: JSContextRef,
            f: JSObjectRef,
            this: JSObjectRef,
            argc: usize,
            argv: *const JSValueRef,
            exc: *mut JSValueRef,
        ) -> JSValueRef;
    }
}

#[cfg(test)]
mod tests;
