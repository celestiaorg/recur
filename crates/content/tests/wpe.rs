//! The HTML preview's WPE WebKit page (`html_wpe`), built when pkg-config finds WPE 2.54
//! (build/linux/wpe.Dockerfile). Needs no display. `WPE_NETWORK=0` skips the https iframe.

fn main() {
    // nextest lists a custom harness's tests first; this binary is the one test `wpe`.
    if std::env::args().any(|a| a == "--list") {
        if !std::env::args().any(|a| a == "--ignored") {
            println!("wpe: test");
        }
        return;
    }
    run();
}

#[cfg(not(recur_wpe))]
fn run() {
    println!("wpe: skipped (built without WPE WebKit 2.54)");
}

#[cfg(recur_wpe)]
fn run() {
    page::trusted_input();
    page::ime();
    page::hover_and_select();
    page::animation_rate();
    page::navigation();
    println!("wpe: ok");
}

#[cfg(recur_wpe)]
mod page {
    use std::time::{Duration, Instant};

    use futures::channel::mpsc::{UnboundedReceiver, unbounded};
    use recur_content::html_wpe::{Page, PageEvent};
    use serde_json::{Value, json};

    /// Posts `{type: 'say', v}` to the host.
    const SAY: &str =
        "<script>function say(v){parent.postMessage({type:'say',v:String(v)},'*')}</script>";

    struct Host {
        page: Page,
        rx: UnboundedReceiver<PageEvent>,
    }

    impl Host {
        fn new(doc: &str, w: f32, h: f32) -> Host {
            let (tx, rx) = unbounded();
            let page = Page::new(doc, tx).expect("the WPE shim loads");
            page.resize(w, h, 1.);
            page.set_active(true);
            Host { page, rx }
        }

        fn next(&mut self, what: &str, want: impl Fn(&PageEvent) -> bool) -> PageEvent {
            let deadline = Instant::now() + Duration::from_secs(20);
            loop {
                match self.rx.try_recv() {
                    Ok(ev) if want(&ev) => return ev,
                    Ok(_) => {}
                    Err(_) => {
                        assert!(Instant::now() < deadline, "timed out waiting for {what}");
                        std::thread::sleep(Duration::from_millis(5));
                    }
                }
            }
        }

        /// The next `say(v)` from the page.
        fn said(&mut self, prefix: &str) -> String {
            let PageEvent::Message(m) = self.next(prefix, |e| {
                matches!(e, PageEvent::Message(m) if said(m).is_some_and(|v| v.starts_with(prefix)))
            }) else {
                unreachable!()
            };
            said(&m).unwrap()
        }

        /// The next frame whose pixel at (`x`, `y`) matches `want` (r, g, b).
        fn pixel(&mut self, what: &str, x: u32, y: u32, want: impl Fn(u8, u8, u8) -> bool) {
            self.next(what, |e| match e {
                PageEvent::Frame {
                    width,
                    height,
                    bgra,
                } if x < *width && y < *height => {
                    let i = ((y * width + x) * 4) as usize;
                    want(bgra[i + 2], bgra[i + 1], bgra[i])
                }
                _ => false,
            });
        }

        fn click(&self, x: f32, y: f32) {
            self.page.input(&json!({"type": "move", "x": x, "y": y}));
            self.page
                .input(&json!({"type": "down", "x": x, "y": y, "button": 0, "detail": 1}));
            self.page
                .input(&json!({"type": "up", "x": x, "y": y, "button": 0, "detail": 1}));
        }
    }

    fn said(m: &str) -> Option<String> {
        let v: Value = serde_json::from_str(m).ok()?;
        (v["type"] == "say").then(|| v["v"].as_str().unwrap_or_default().to_string())
    }

    /// Clicks and keys arrive as trusted events with user activation, and typing edits a field.
    pub fn trusted_input() {
        let doc = format!(
            r#"<body style="margin:0;background:#f00">{SAY}
<button id=b style="position:absolute;left:10px;top:10px;width:80px;height:30px">Go</button>
<input id=i style="position:absolute;left:10px;top:60px;width:120px">
<script>
b.onclick = e => {{ document.body.style.background = '#00f';
  say('click:' + e.isTrusted + ':' + navigator.userActivation.isActive) }};
i.onkeydown = e => say('key:' + e.key + ':' + e.code + ':' + e.isTrusted);
i.onfocus = () => say('focus');
</script></body>"#
        );
        let mut h = Host::new(&doc, 200., 100.);
        h.pixel("the red page", 150, 80, |r, g, b| {
            r > 200 && g < 50 && b < 50
        });
        h.click(50., 25.);
        assert_eq!(h.said("click:"), "click:true:true");
        h.pixel("the blue page", 150, 80, |r, _, b| b > 200 && r < 50);

        h.click(40., 70.);
        // WebKit queues mouse and key events apart: type once the click has focused the field.
        h.said("focus");
        for k in ["h", "i", "!"] {
            h.page.input(&json!({"type": "key", "key": k}));
        }
        assert_eq!(h.said("key:h"), "key:h:KeyH:true");
        h.page.input(&json!({"type": "key", "key": "Backspace"}));
        assert_eq!(h.said("key:Backspace"), "key:Backspace:Backspace:true");
        h.page
            .input(&json!({"type": "key", "key": "a", "ctrl": true, "selectAll": true}));
        h.page
            .input(&json!({"type": "key", "key": "c", "ctrl": true, "copy": true}));
        let PageEvent::Reply(Value::String(copied)) = h.next(
            "the copy",
            |e| matches!(e, PageEvent::Reply(Value::String(s)) if s.starts_with('\u{1}')),
        ) else {
            unreachable!()
        };
        assert_eq!(&copied[1..], "hi", "typed, and Backspace removed the !");
        println!("wpe trusted input: ok");
    }

    /// The IME's preedit and commit reach the field as composition events.
    pub fn ime() {
        let doc = format!(
            r#"<body style="margin:0">{SAY}<input id=i style="width:150px">
<script>
i.onfocus = () => say('focus');
for (const t of ['compositionstart', 'compositionend'])
  i.addEventListener(t, e => say(t + ':' + e.data + ':' + e.isTrusted));
i.oninput = e => say('input:' + i.value + ':' + e.isComposing);
</script></body>"#
        );
        let mut h = Host::new(&doc, 200., 50.);
        h.next("a frame", |e| matches!(e, PageEvent::Frame { .. }));
        h.click(20., 10.);
        h.said("focus");
        h.page.ime("に", 1, false);
        assert_eq!(h.said("compositionstart"), "compositionstart::true");
        assert_eq!(h.said("input:"), "input:に:true");
        h.page.ime("日本", 0, true);
        // The last input is still part of the composition, which then ends.
        assert_eq!(h.said("input:日本"), "input:日本:true");
        assert_eq!(h.said("compositionend"), "compositionend:日本:true");
        println!("wpe ime: ok");
    }

    /// `:hover` follows the pointer natively, and a `<select>` asks the host for its menu.
    pub fn hover_and_select() {
        let doc = format!(
            r#"<style>body{{margin:0}} #h{{width:100px;height:40px;background:#f00}} #h:hover{{background:#0f0}}</style>{SAY}
<div id=h></div>
<select id=s style="position:absolute;left:0;top:60px"><option>a</option><option>b</option><option>c</option></select>
<script>s.onchange = e => say('change:' + s.value + ':' + e.isTrusted)</script>"#
        );
        let mut h = Host::new(&doc, 200., 100.);
        h.pixel("the unhovered box", 50, 20, |r, g, _| r > 200 && g < 50);
        h.page.input(&json!({"type": "move", "x": 50, "y": 20}));
        h.pixel("the hovered box", 50, 20, |r, g, _| g > 200 && r < 50);
        h.page.input(&json!({"type": "leave"}));
        h.pixel("the box after leaving", 50, 20, |r, g, _| r > 200 && g < 50);

        h.click(20., 70.);
        let PageEvent::Reply(menu) = h.next(
            "the select menu",
            |e| matches!(e, PageEvent::Reply(v) if v.get("select").is_some()),
        ) else {
            unreachable!()
        };
        assert_eq!(menu["select"]["options"].as_array().map(Vec::len), Some(3));
        assert_eq!(menu["select"]["index"], 0);
        h.page.input(&json!({"type": "pick", "index": 2}));
        assert_eq!(h.said("change:"), "change:c:true");
        println!("wpe hover and select: ok");
    }

    /// A CSS animation and a canvas drawn on `requestAnimationFrame`: frames delivered per
    /// second, and the page's own rate.
    pub fn animation_rate() {
        let doc = format!(
            r#"<style>body{{margin:0}} @keyframes m{{from{{transform:translateX(0)}}to{{transform:translateX(300px)}}}}
#a{{width:50px;height:50px;background:#00f;animation:m 1s linear infinite alternate}}</style>{SAY}
<div id=a></div><canvas id=c width=400 height=200></canvas>
<script>
const g = c.getContext('2d'); let n = 0, t0 = 0;
function tick(t) {{ if (!t0) t0 = t; n++; g.fillStyle = `hsl(${{n % 360}},80%,50%)`; g.fillRect(0, 0, 400, 200);
  if (t - t0 >= 3000) {{ say('raf:' + (n * 1000 / (t - t0)).toFixed(1)); n = 0; t0 = t }}
  requestAnimationFrame(tick) }}
requestAnimationFrame(tick);
</script>"#
        );
        let (w, hgt) = (800., 600.);
        let mut h = Host::new(&doc, w, hgt);
        h.next("a frame", |e| matches!(e, PageEvent::Frame { .. }));
        let start = Instant::now();
        let mut frames = 0;
        while start.elapsed() < Duration::from_secs(3) {
            match h.rx.try_recv() {
                Ok(PageEvent::Frame { .. }) => frames += 1,
                Ok(_) => {}
                Err(_) => std::thread::sleep(Duration::from_millis(1)),
            }
        }
        let fps = frames as f64 / start.elapsed().as_secs_f64();
        let raf = h.said("raf:");
        println!("wpe animation: {fps:.1} frames/s delivered at {w}x{hgt}, page {raf}/s");
        assert!(fps > 30., "{fps:.1} frames/s");
    }

    /// An https iframe loads (network allowing); the page cannot navigate away or open a popup.
    pub fn navigation() {
        // A main-frame navigation, even a refused one, stops subframe loads: try it once the
        // iframe has loaded.
        let doc = recur_content::preview::html_preview_srcdoc(&format!(
            r#"<style>body{{margin:0;background:#f00}} iframe{{border:0;position:absolute;left:0;top:50px;width:300px;height:150px}}</style>{SAY}
<iframe src="https://example.com/" onload="setTimeout(away, 500)"></iframe>
<script>
say('popup:' + window.open('https://example.com/'));
function away() {{ location.href = 'https://example.com/away'; say('tried');
  setTimeout(() => say('still:' + location.href), 2000) }}
</script>"#
        ));
        let mut h = Host::new(&doc, 300., 200.);
        assert_eq!(h.said("popup:"), "popup:null");
        if std::env::var_os("WPE_NETWORK").is_none_or(|v| v != "0") {
            // example.com's page is light: anything but the body's red shows it loaded.
            h.pixel("the https iframe", 150, 150, |r, g, b| {
                r > 200 && g > 200 && b > 200
            });
        }
        h.said("tried");
        assert_eq!(h.said("still:"), "still:about:blank");
        println!("wpe navigation: ok");
    }
}
