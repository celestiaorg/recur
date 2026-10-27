//! The mermaid.js host: mermaid 11.16.1 (the build the web UI ships, vendored under
//! `assets/mermaid`) in a hidden WKWebView (WebKitGTK on Linux). Mermaid lays diagrams out from DOM text
//! measurement, so it runs in WebKit itself and measures exactly as the app's webview did; the
//! diagram it returns is drawn natively. The page loads nothing (CSP `default-src 'none'`),
//! keeps no data (non-persistent store) and is never on screen.

/// The page mermaid runs in: the app's web fonts (the files `frontend/index.html` loads, as
/// recur-ui bundles them) and `body` font, which mermaid's text measurement inherits. The page
/// is the WKWebView frame's 1024×768 in CSS, so layouts that read the container (gantt) see
/// the same width where the webview has no size of its own (WebKitGTK, never allocated).
pub fn page() -> String {
    use base64::Engine as _;
    let mut faces = String::new();
    for (family, weight, bytes) in recur_ui::fonts::FACES {
        faces.push_str(&format!(
            "@font-face{{font-family:'{family}';font-style:normal;font-weight:{weight};font-display:block;src:url(data:font/ttf;base64,{}) format('truetype');}}",
            base64::engine::general_purpose::STANDARD.encode(bytes)
        ));
    }
    format!(
        r#"<!DOCTYPE html><html><head><meta charset="utf-8"><meta http-equiv="Content-Security-Policy" content="default-src 'none'; script-src 'unsafe-eval'; style-src 'unsafe-inline'; img-src data:; font-src data:"><style>{faces}*{{box-sizing:border-box}}html,body{{margin:0;width:1024px;height:768px;overflow:hidden}}body{{font-family:'Inter',system-ui,sans-serif;font-size:13px;line-height:1.45}}</style></head><body><div id="m"></div></body></html>"#
    )
}

/// Injected after mermaid: `recur(op)` renders a source or measures a sanitized SVG.
pub const HOST_JS: &str = r#"
(function () {
  function unclip(root) {
    var svg = root.querySelector('svg');
    if (!svg) return;
    svg.setAttribute('overflow', 'visible');
    svg.querySelectorAll('g, foreignObject, .label, .node, .cluster, .edgeLabel, .nodeLabel').forEach(function (n) {
      n.setAttribute('overflow', 'visible');
      if (n.style) n.style.overflow = 'visible';
    });
  }
  // Paint WebKit resolved, inlined so the native rasterizer needs no selector support.
  var PAINT = ['fill', 'fill-opacity', 'fill-rule', 'stroke', 'stroke-width', 'stroke-opacity', 'stroke-dasharray',
    'stroke-dashoffset', 'stroke-linecap', 'stroke-linejoin', 'stroke-miterlimit', 'opacity', 'display', 'visibility'];
  function collapse(s) { return s.replace(/[\n\r\t]/g, ' ').replace(/ +/g, ' '); }
  function measure(html) {
    var host = document.getElementById('m');
    host.innerHTML = html;
    unclip(host);
    var svg = host.querySelector('svg');
    if (!svg) return { error: 'no svg' };
    var vb = svg.viewBox && svg.viewBox.baseVal;
    var w = vb && vb.width > 0 ? vb.width : parseFloat(svg.getAttribute('width')) || 0;
    var h = vb && vb.height > 0 ? vb.height : parseFloat(svg.getAttribute('height')) || 0;
    var ox = vb && vb.width > 0 ? vb.x : 0, oy = vb && vb.height > 0 ? vb.y : 0;
    svg.removeAttribute('style');
    svg.setAttribute('width', String(w));
    svg.setAttribute('height', String(h));
    svg.style.width = w + 'px';
    svg.style.height = h + 'px';
    svg.style.maxWidth = 'none';
    var inv = svg.getScreenCTM().inverse();
    var texts = svg.querySelectorAll('text');
    var runs = [], raster = [];
    for (var ti = 0; ti < texts.length; ti++) {
      var t = texts[ti];
      var m = inv.multiply(t.getScreenCTM());
      if (Math.abs(m.b) > 1e-6 || Math.abs(m.c) > 1e-6 || m.a <= 0 || m.d <= 0 || Math.abs(m.a - m.d) > 1e-6) { raster.push(ti); continue; }
      var leaves = [];
      (function walk(el) {
        var own = false;
        for (var c = el.firstChild; c; c = c.nextSibling) {
          if (c.nodeType === 3 && c.data.length) own = true;
          if (c.nodeType === 1) walk(c);
        }
        if (own) leaves.push(el);
      })(t);
      var ok = true, mine = [];
      for (var li = 0; li < leaves.length && ok; li++) {
        var el = leaves[li];
        var cs = getComputedStyle(el);
        if (cs.display === 'none' || cs.visibility === 'hidden') continue;
        var text = '';
        for (var c = el.firstChild; c; c = c.nextSibling) if (c.nodeType === 3) text += c.data;
        var n = el.getNumberOfChars();
        if (!n) continue;
        var trimmed = collapse(text);
        if (li === 0) trimmed = trimmed.replace(/^ /, '');
        if (li === leaves.length - 1) trimmed = trimmed.replace(/ $/, '');
        if (trimmed.length !== n) { ok = false; break; }
        var p = el.getStartPositionOfChar(0);
        var end = el.getEndPositionOfChar(n - 1);
        var op = 1;
        for (var e = el; e && e !== svg; e = e.parentElement) op *= parseFloat(getComputedStyle(e).opacity);
        mine.push({
          text: trimmed, x: p.x * m.a + m.e, y: p.y * m.d + m.f,
          size: parseFloat(cs.fontSize) * m.a, family: cs.fontFamily, weight: parseFloat(cs.fontWeight) || 400,
          italic: cs.fontStyle === 'italic' || cs.fontStyle === 'oblique', fill: cs.fill,
          fillOpacity: parseFloat(cs.fillOpacity), opacity: op,
          spacing: cs.letterSpacing === 'normal' ? 0 : parseFloat(cs.letterSpacing) || 0,
          decoration: cs.textDecorationLine, advance: (end.x - p.x) * m.a, text_index: ti
        });
      }
      if (ok) runs = runs.concat(mine); else raster.push(ti);
    }
    var els = svg.querySelectorAll('*');
    var styles = Array.prototype.map.call(els, function (el) {
      var cs = getComputedStyle(el);
      return PAINT.map(function (p) { return cs.getPropertyValue(p); });
    });
    for (var ei = 0; ei < els.length; ei++) {
      for (var pi = 0; pi < PAINT.length; pi++) els[ei].style.setProperty(PAINT[pi], styles[ei][pi]);
    }
    return { width: w, height: h, x: ox, y: oy, runs: runs, raster: raster, texts: texts.length, svg: svg.outerHTML };
  }
  window.recur = async function (op) {
    await document.fonts.ready;
    await Promise.all(Array.from(document.fonts).map(function (f) { return f.load().catch(function () {}); }));
    if (op.op === 'render') {
      mermaid.initialize(op.config);
      try {
        var out = await mermaid.render(op.id, op.source);
        return { svg: out.svg };
      } catch (e) {
        var stale = document.getElementById('d' + op.id);
        if (stale) stale.remove();
        return { error: (e && e.message) ? e.message : 'Mermaid render failed' };
      }
    }
    if (op.op === 'measure') return measure(op.svg);
    return { error: 'unknown op' };
  };
})();
"#;

pub static HOST: crate::webhost::Host = crate::webhost::Host {
    name: "Mermaid",
    #[cfg(target_os = "macos")]
    make: mac::make,
    #[cfg(target_os = "linux")]
    make: gtk::make,
};

#[cfg(any(target_os = "macos", target_os = "linux"))]
const MERMAID_JS: &str = include_str!("../../assets/mermaid/mermaid.min.js");

#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
mod mac {
    use objc2::rc::Retained;
    use objc2::{MainThreadMarker, MainThreadOnly};
    use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};
    use objc2_web_kit::{
        WKUserScript, WKUserScriptInjectionTime, WKWebView, WKWebViewConfiguration,
        WKWebsiteDataStore,
    };

    pub fn make(mtm: MainThreadMarker) -> Retained<WKWebView> {
        // SAFETY: WebKit objects are created and used on the main thread (`mtm`).
        unsafe {
            let config = WKWebViewConfiguration::new(mtm);
            config.setWebsiteDataStore(&WKWebsiteDataStore::nonPersistentDataStore(mtm));
            let source = NSString::from_str(&format!("{}\n{}", super::MERMAID_JS, super::HOST_JS));
            let script = WKUserScript::initWithSource_injectionTime_forMainFrameOnly(
                WKUserScript::alloc(mtm),
                &source,
                WKUserScriptInjectionTime::AtDocumentEnd,
                true,
            );
            config.userContentController().addUserScript(&script);
            let frame = NSRect::new(NSPoint::new(0., 0.), NSSize::new(1024., 768.));
            let web = WKWebView::initWithFrame_configuration(WKWebView::alloc(mtm), frame, &config);
            web.loadHTMLString_baseURL(&NSString::from_str(&super::page()), None);
            web
        }
    }
}

#[cfg(target_os = "linux")]
mod gtk {
    use webkit6::prelude::*;
    use webkit6::{
        NetworkSession, UserContentInjectedFrames, UserContentManager, UserScript,
        UserScriptInjectionTime, WebView,
    };

    pub fn make() -> WebView {
        let scripts = UserContentManager::new();
        scripts.add_script(&UserScript::new(
            &format!("{}\n{}", super::MERMAID_JS, super::HOST_JS),
            UserContentInjectedFrames::TopFrame,
            UserScriptInjectionTime::End,
            &[],
            &[],
        ));
        let web = WebView::builder()
            .network_session(&NetworkSession::new_ephemeral())
            .user_content_manager(&scripts)
            .build();
        web.load_html(&super::page(), None);
        web
    }
}
