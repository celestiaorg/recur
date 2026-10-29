use std::time::Duration;

use serde_json::{Value, json};

use super::*;

const T: Duration = Duration::from_secs(5);

/// Runs `body` (a macro's function body) through the macro boot and returns its text.
async fn eval(body: &str) -> Result<String, String> {
    let code = format!("module.exports = async (ctx) => {{ {body} }}");
    let out = run_once("macro", MACRO_BOOT, &json!({ "code": code, "ctx": {} }), T).await;
    out.result
        .map(|v| v["text"].as_str().unwrap_or_default().to_owned())
}

#[tokio::test]
async fn sandbox_has_worker_globals_and_no_node() {
    let got = eval(
        "return [typeof require, typeof process, typeof setTimeout, typeof fetch === 'undefined' ? 'nofetch' : 'fetch', typeof self, typeof Intl].join(',')",
    )
    .await
    .unwrap();
    assert_eq!(got, "undefined,undefined,function,nofetch,object,object");
}

#[tokio::test]
async fn text_codecs_and_base64() {
    let got = eval(
        "const b = new TextEncoder().encode('héllo 👋'); \
         return [b.length, new TextDecoder().decode(b), btoa('hi!'), atob('aGkh'), \
                 new TextDecoder().decode(new Uint8Array([0xff, 0x41]))].join('|')",
    )
    .await
    .unwrap();
    assert_eq!(got, "11|héllo 👋|aGkh|hi!|\u{fffd}A");
    let e = eval("return btoa('é✓')").await.unwrap_err();
    assert_eq!(e, "Invalid character");
}

#[tokio::test]
async fn url_and_search_params() {
    let got = eval(
        "const u = new URL('../b?x=1&y=two words#h', 'https://Example.com:443/a/c'); \
         u.searchParams.append('z', 'ä'); \
         const q = new URLSearchParams({ a: '1 2', b: '&' }); \
         return [u.href, u.host, u.pathname, u.searchParams.get('y'), q.toString(), URL.canParse('nope')].join('|')",
    )
    .await
    .unwrap();
    assert_eq!(
        got,
        "https://example.com/b?x=1&y=two+words&z=%C3%A4#h|example.com|/b|two words|a=1+2&b=%26|false"
    );
    let e = eval("return new URL('not a url').href").await.unwrap_err();
    assert_eq!(e, "Invalid URL: not a url");
}

#[tokio::test]
async fn timers_promises_and_intl() {
    let got = eval(
        "const order = []; \
         const t = setTimeout(() => order.push('cleared'), 0); clearTimeout(t); \
         await new Promise((r) => setTimeout(() => { order.push('timer'); r(); }, 10)); \
         queueMicrotask(() => order.push('micro')); await null; \
         let n = 0; await new Promise((r) => { const i = setInterval(() => { if (++n === 3) { clearInterval(i); r(); } }, 1); }); \
         order.push(String(n)); \
         order.push(new Intl.NumberFormat('en-US').format(1234.5)); \
         return order.join(',')",
    )
    .await
    .unwrap();
    assert_eq!(got, "timer,micro,3,1,234.5");
}

#[tokio::test]
async fn crypto_clone_and_console() {
    let code = "module.exports = () => { console.log('hi', { a: 1 }); console.error(new Error('boom').message); \
                const id = crypto.randomUUID(); const c = structuredClone({ d: new Date(0), m: new Map([[1, [2]]]) }); \
                return [/^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/.test(id), \
                        c.d.getTime(), c.m.get(1)[0], crypto.getRandomValues(new Uint8Array(4)).length].join(',') }";
    let out = run_once("macro", MACRO_BOOT, &json!({ "code": code, "ctx": {} }), T).await;
    assert_eq!(out.result.unwrap()["text"], "true,0,2,4");
    assert_eq!(out.logs, "hi {\"a\":1}\nboom\n");
}

#[tokio::test]
async fn a_hung_or_spinning_call_times_out_and_the_worker_recovers() {
    let w = JsWorker::new("macro", MACRO_BOOT);
    let call = |code: &str, timeout| {
        let msg = json!({ "code": format!("module.exports = {code}"), "ctx": {} });
        let w = &w;
        async move { w.call(&msg, timeout).await.result }
    };
    let short = Duration::from_millis(200);
    assert_eq!(
        call("() => new Promise(() => {})", short)
            .await
            .unwrap_err(),
        "macro timed out"
    );
    assert_eq!(
        call("() => { for (;;) {} }", short).await.unwrap_err(),
        "macro timed out"
    );
    let ok = call("() => 'again'", T).await.unwrap();
    assert_eq!(ok["text"], "again");
}

#[tokio::test]
async fn dropping_a_call_stops_it() {
    let w = JsWorker::new("complete", COMPLETE_BOOT);
    let warm = json!({ "code": "module.exports = () => []", "ctx": {} });
    w.call(&warm, Duration::from_secs(60)).await.result.unwrap();
    let spin = json!({ "code": "module.exports = () => { for (;;) {} }", "ctx": {} });
    let dropped = tokio::time::timeout(Duration::from_millis(100), w.call(&spin, T)).await;
    assert!(dropped.is_err());
    let quick = json!({ "code": "module.exports = () => [{ insert: 'x' }]", "ctx": {} });
    let started = std::time::Instant::now();
    let got = w.call(&quick, T).await.result.unwrap();
    assert_eq!(got["hits"][0]["label"], "x");
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "{:?}",
        started.elapsed()
    );
}

#[tokio::test]
async fn errors_carry_the_thrown_message() {
    assert_eq!(eval("throw new Error('nope')").await.unwrap_err(), "nope");
    assert_eq!(eval("throw 'plain'").await.unwrap_err(), "plain");
    let out = run_once(
        "macro",
        MACRO_BOOT,
        &json!({ "code": "module.exports = 42", "ctx": {} }),
        T,
    )
    .await;
    assert_eq!(
        out.result.unwrap_err(),
        "macro must export default function (ctx) { … }"
    );
    let out = run_once("prompt", PROMPT_BOOT, &Value::Null, T).await;
    assert!(out.result.is_err());
}
