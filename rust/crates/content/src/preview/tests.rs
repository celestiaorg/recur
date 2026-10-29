//! `preview.test.ts`.

use super::*;

#[test]
fn maps_html_and_htm_to_html() {
    assert_eq!(preview_kind_for_path("demo.html"), Some(PreviewKind::Html));
    assert_eq!(preview_kind_for_path("/x/Y.HTM"), Some(PreviewKind::Html));
    assert_eq!(
        preview_kind_for_path("notes.MD"),
        Some(PreviewKind::Markdown)
    );
    assert_eq!(
        preview_kind_for_path("flow.mmd"),
        Some(PreviewKind::Mermaid)
    );
    assert_eq!(preview_kind_for_path("Makefile"), None);
}

#[test]
fn find_is_on_only_for_a_visible_editor_preview() {
    assert!(file_preview_find_active(true, true, "editor", false, false));
    assert!(!file_preview_find_active(
        false, true, "editor", false, false
    ));
    assert!(!file_preview_find_active(
        true, false, "editor", false, false
    ));
    assert!(!file_preview_find_active(
        true, true, "review", false, false
    ));
    assert!(!file_preview_find_active(true, true, "editor", true, false));
    // Off after Hide editor even when preview state is otherwise live.
    assert!(!file_preview_find_active(true, true, "editor", false, true));
}

#[test]
fn csp_allows_https_but_never_the_app_origin_or_localhost() {
    assert!(
        HTML_PREVIEW_CSP.contains("script-src https: data: blob: 'unsafe-inline' 'unsafe-eval'")
    );
    assert!(HTML_PREVIEW_CSP.contains("style-src https: data: blob: 'unsafe-inline'"));
    assert!(HTML_PREVIEW_CSP.contains("connect-src https: wss: data: blob:"));
    assert!(HTML_PREVIEW_CSP.contains("form-action 'none'"));
    for bad in ["http:", "ws:", "wails", "localhost", "*"] {
        assert!(!HTML_PREVIEW_CSP.contains(bad), "{bad}");
    }
}

fn shell_prefix() -> String {
    format!(
        "<!DOCTYPE html><html><head><meta http-equiv=\"Content-Security-Policy\" content=\"{HTML_PREVIEW_CSP}\"><style>::highlight(preview-find){{background-color:#e6cc7788}}::highlight(preview-find-current){{background-color:#e6a817;color:#111}}</style><script>{HTML_PREVIEW_STORAGE_SHIM}{HTML_PREVIEW_SCROLL_SCRIPT}</script></head><body>"
    )
}

const SUFFIX: &str = "</body></html>";

#[test]
fn srcdoc_matches_the_frontend_byte_for_byte() {
    assert_eq!(
        html_preview_srcdoc("<b>x</b>"),
        r#"<!DOCTYPE html><html><head><meta http-equiv="Content-Security-Policy" content="default-src https: data: blob:; script-src https: data: blob: 'unsafe-inline' 'unsafe-eval'; style-src https: data: blob: 'unsafe-inline'; connect-src https: wss: data: blob:; form-action 'none'; base-uri 'none'; object-src 'none'"><style>::highlight(preview-find){background-color:#e6cc7788}::highlight(preview-find-current){background-color:#e6a817;color:#111}</style><script>try{localStorage}catch(_){(function(){var mk=function(){var d=new Map();return{get length(){return d.size},key:function(i){var k=Array.from(d.keys())[i];return k===undefined?null:k},getItem:function(k){k=String(k);return d.has(k)?d.get(k):null},setItem:function(k,v){d.set(String(k),String(v))},removeItem:function(k){d.delete(String(k))},clear:function(){d.clear()}}};Object.defineProperty(window,'localStorage',{value:mk(),configurable:true});Object.defineProperty(window,'sessionStorage',{value:mk(),configurable:true})})()}(function(){var T='recur-preview-scroll',R='recur-preview-restore',F='recur-preview-find',FR='recur-preview-find-result',K='recur-preview-find-key';function se(){return document.scrollingElement||document.documentElement}function send(){var s=se();parent.postMessage({type:T,top:s.scrollTop,left:s.scrollLeft},'*')}document.addEventListener('scroll',send,{passive:true,capture:true});var matches=[],idx=-1;function skip(n){var p=n.parentElement,e,s;if(!n.data||!p)return true;if(p.tagName==='SCRIPT'||p.tagName==='STYLE'||p.tagName==='NOSCRIPT')return true;for(e=p;e;e=e.parentElement){if(e.hidden)return true;s=getComputedStyle(e);if(s.display==='none'||s.visibility==='hidden')return true}return false}function inl(el){var d=getComputedStyle(el).display;return !d||d==='inline'||d==='inline-block'||d==='inline-flex'||d==='inline-grid'||d==='contents'}function blk(n){for(var e=n.parentElement;e&&e!==document.body;e=e.parentElement)if(!inl(e))return e;return document.body}function paint(){if(!CSS.highlights)return;var rest=[],cur;for(var i=0;i<matches.length;i++){if(i===idx)cur=matches[i];else rest.push(matches[i])}if(rest.length)CSS.highlights.set('preview-find',new Highlight(...rest));else CSS.highlights.delete('preview-find');if(cur)CSS.highlights.set('preview-find-current',new Highlight(cur));else CSS.highlights.delete('preview-find-current')}function reveal(){if(idx<0||!matches[idx])return;try{var r=matches[idx],n=r.startContainer,el=n.nodeType===1?n:n.parentElement,box,p,cs,v,s,oy,ox;if(!el)return;box=r.getBoundingClientRect();if(!box.height&&!box.width)return;s=se();for(p=el;p&&p!==document.documentElement;p=p.parentElement){if(p===s)continue;cs=getComputedStyle(p);v=p.getBoundingClientRect();oy=cs.overflowY||cs.overflow;ox=cs.overflowX||cs.overflow;if((oy==='auto'||oy==='scroll')&&p.scrollHeight>p.clientHeight)p.scrollTop+=(box.top+box.bottom)/2-(v.top+v.bottom)/2;if((ox==='auto'||ox==='scroll')&&p.scrollWidth>p.clientWidth)p.scrollLeft+=(box.left+box.right)/2-(v.left+v.right)/2;box=r.getBoundingClientRect()}s.scrollTop+=(box.top+box.bottom)/2-innerHeight/2;s.scrollLeft+=(box.left+box.right)/2-innerWidth/2}catch(e){}}function report(ok){parent.postMessage({type:FR,total:matches.length,index:idx<0?0:idx+1,valid:ok!==false},'*')}function wc(s,i){var c=s[i];return!!c&&/[\p{L}\p{N}_]/u.test(c)}function wwok(s,a,b){return !wc(s,a-1)&&!wc(s,b)}function collect(q,cs,re,ww){matches=[];idx=-1;if(!q){paint();report(true);return}var rx;try{rx=new RegExp(re?q:q.replace(/[.*+?^${}()|[\]\\]/g,'\\$&'),cs?'g':'gi')}catch(e){paint();report(false);return}var hay='',pcs=[],pb=null,br=false,w=document.createTreeWalker(document.body,NodeFilter.SHOW_ELEMENT|NodeFilter.SHOW_TEXT),n,cap=999,i,p,pe,sn,so,en,eo,a,b,m;while(n=w.nextNode()){if(n.nodeType===1){if(n.tagName==='BR')br=true;continue}if(skip(n))continue;p=blk(n);if(hay&&(br||p!==pb))hay+='\n';br=false;pb=p;pcs.push({n:n,a:hay.length});hay+=n.data}rx.lastIndex=0;while(m=rx.exec(hay)){a=m.index;b=a+m[0].length;if(a===b){rx.lastIndex=a+1;continue}if(ww&&!wwok(hay,a,b))continue;sn=so=en=eo=null;for(i=0;i<pcs.length;i++){p=pcs[i];pe=p.a+p.n.data.length;if(sn==null&&a>=p.a&&a<pe){sn=p.n;so=a-p.a}if(b>p.a&&b<=pe){en=p.n;eo=b-p.a}}if(!sn||!en)continue;p=new Range();p.setStart(sn,so);p.setEnd(en,eo);matches.push(p);if(matches.length>=cap)break}idx=matches.length?0:-1;paint();reveal();report(true)}function step(d){if(!matches.length)return;idx=(idx+d+matches.length)%matches.length;paint();reveal();report()}window.addEventListener('message',function(e){if(!e.data)return;if(e.data.type===R){var s=se();s.scrollTop=e.data.top||0;s.scrollLeft=e.data.left||0;return}if(e.data.type!==F)return;if(e.data.op==='query')collect(e.data.search||'',!!e.data.caseSensitive,!!e.data.regexp,!!e.data.wholeWord);else if(e.data.op==='next')step(1);else if(e.data.op==='prev')step(-1);else if(e.data.op==='clear'){matches=[];idx=-1;paint();report()}});document.addEventListener('keydown',function(e){var mod=e.metaKey||e.ctrlKey,key=e.key;if(mod&&!e.altKey&&(key==='f'||key==='F')&&!e.shiftKey){e.preventDefault();parent.postMessage({type:K,key:'f'},'*')}else if(mod&&!e.altKey&&(key==='g'||key==='G')){e.preventDefault();parent.postMessage({type:K,key:e.shiftKey?'G':'g'},'*')}else if(key==='F3'){e.preventDefault();parent.postMessage({type:K,key:e.shiftKey?'G':'g'},'*')}else if(key==='Escape')parent.postMessage({type:K,key:'Escape'},'*')},true)})();</script></head><body><b>x</b></body></html>"#
    );
    assert_eq!(
        svg_data_url("<svg>é</svg>"),
        r#"data:image/svg+xml;base64,PHN2Zz7DqTwvc3ZnPg=="#
    );
}

#[test]
fn always_wraps_content_in_a_csp_first_shell() {
    let frag = "<button id=\"b\">Go</button><script>b.onclick=()=>{}</script>";
    let out = html_preview_srcdoc(frag);
    assert_eq!(out, format!("{}{frag}{SUFFIX}", shell_prefix()));
    for needle in [
        PREVIEW_IFRAME_SCROLL,
        PREVIEW_IFRAME_FIND,
        PREVIEW_IFRAME_FIND_KEY,
        "e.data.regexp",
        "e.data.wholeWord",
        "e.hidden",
        "overflowX",
        "SHOW_ELEMENT",
    ] {
        assert!(out.contains(needle), "{needle}");
    }
}

#[test]
fn wraps_full_documents_without_splicing_into_their_head() {
    let src =
        "<!DOCTYPE html><html><head><title>T</title></head><body><script>1</script></body></html>";
    let out = html_preview_srcdoc(src);
    assert!(out.starts_with(&shell_prefix()));
    assert!(out.ends_with(SUFFIX));
    assert!(out.contains("<title>T</title>"));
    assert!(out.contains("<script>1</script>"));
    assert_eq!(out.matches("Content-Security-Policy").count(), 1);
}

#[test]
fn keeps_an_author_csp_and_wraps_decoys_verbatim() {
    let src = "<html><head><meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'self'\"></head><body>x</body></html>";
    let out = html_preview_srcdoc(src);
    assert!(out.starts_with(&shell_prefix()));
    assert_eq!(out.matches("Content-Security-Policy").count(), 2);
    assert_eq!(
        html_preview_srcdoc(""),
        format!("{}{SUFFIX}", shell_prefix())
    );
    for decoy in [
        "<!--<head>--><script>fetch('https://evil')</script>",
        "<header></header><head></head><script>1</script>",
        "<head data-x=\">\"></head><script>1</script>",
        "<script>fetch('https://evil')</script><head></head>",
    ] {
        assert_eq!(
            html_preview_srcdoc(decoy),
            format!("{}{decoy}{SUFFIX}", shell_prefix())
        );
    }
}

#[test]
fn the_find_escape_in_the_script_escapes_regex_metacharacters() {
    // `q.replace(/[.*+?^${}()|[\]\\]/g,'\\$&')`, applied through the same regex engine.
    let start = HTML_PREVIEW_SCROLL_SCRIPT.find("q.replace(/").unwrap() + "q.replace(/".len();
    let end = HTML_PREVIEW_SCROLL_SCRIPT[start..].find("/g,").unwrap() + start;
    let re = crate::js::Re::new(&HTML_PREVIEW_SCROLL_SCRIPT[start..end], "g");
    assert_eq!(re.replace("a+b", "\\$&"), "a\\+b");
    assert_eq!(re.replace("hello", "\\$&"), "hello");
}

struct PreviewHost(gpui_kit::Entity<view::FilePreview>);

impl gpui_kit::Render for PreviewHost {
    fn render(
        &mut self,
        _: &mut gpui_kit::Window,
        _: &mut gpui_kit::Context<Self>,
    ) -> impl gpui_kit::IntoElement {
        use gpui_kit::{ParentElement as _, Styled as _};
        gpui_kit::div()
            .size_full()
            .flex()
            .flex_col()
            .child(self.0.clone())
    }
}

/// A long document of nested lists (a 224 KB checklist of them) lays its text
/// out a few times a frame, not once per pass of every list above it: a whole-window refresh
/// took 1s a frame with that file open.
#[gpui::test]
fn a_long_markdown_preview_measures_each_text_a_few_times(cx: &mut gpui::TestAppContext) {
    use gpui_kit::AppContext as _;
    cx.update(|cx| {
        gpui_kit::init(cx);
        recur_ui::init(cx);
    });
    let mut md = String::new();
    for s in 0..6 {
        md += &format!("## Section {s}\n\n");
        for i in 0..6 {
            md += &format!("- [x] **Item {i}.** Some words about the item that wrap.\n");
            for j in 0..3 {
                md += &format!("  - Nested {j} with `code` and more words.\n");
                md += "    - Deeper still.\n";
            }
        }
        md += "\n";
    }
    let (_host, cx) = cx.add_window_view(|window, cx| {
        let scroll = std::rc::Rc::new(std::cell::RefCell::new(scroll::PreviewScrollStore::new()));
        let fp = cx.new(|cx| {
            let mut p =
                view::FilePreview::new(PreviewKind::Markdown, "", "DOC.md", scroll, window, cx);
            p.set_content(md, window, cx);
            p
        });
        PreviewHost(fp)
    });
    cx.run_until_parked();
    recur_ui::guard::count_layouts(true);
    recur_ui::guard::take_measure_count();
    cx.update(|window, cx| {
        window.refresh();
        window.draw(cx).clear(cx);
    });
    let texts: u32 = recur_ui::guard::take_layout_counts().values().sum();
    let measures = recur_ui::guard::take_measure_count();
    recur_ui::guard::count_layouts(false);
    assert!(texts > 200, "{texts} texts");
    assert!(
        measures <= 16 * texts,
        "{measures} measures for {texts} texts"
    );
}
