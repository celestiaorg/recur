// Observatory preview kinds — dual-nature files that support Code + Preview.

export type PreviewKind = 'markdown' | 'svg' | 'html' | 'mermaid'

const PREVIEW_BY_EXT: Record<string, PreviewKind> = {
  '.md': 'markdown',
  '.mdx': 'markdown',
  '.markdown': 'markdown',
  '.svg': 'svg',
  '.html': 'html',
  '.htm': 'html',
  '.mmd': 'mermaid',
  '.mermaid': 'mermaid',
}

export function filePreviewFindActive(opts: {
  showPreview: boolean
  shellActive?: boolean
  mainSurface: string
  termOverlaying: boolean
  editorHidden: boolean
}): boolean {
  return !!(opts.showPreview && opts.shellActive && opts.mainSurface === 'editor' && !opts.termOverlaying && !opts.editorHidden)
}

/** Returns the preview renderer kind for a path, or null when none. */
export function previewKindForPath(path: string): PreviewKind | null {
  const base = path.split(/[/\\]/).pop() ?? path
  const i = base.lastIndexOf('.')
  if (i < 0) return null
  return PREVIEW_BY_EXT[base.slice(i).toLowerCase()] ?? null
}

/** Build a data-URL for an SVG source string (safe in <img>; scripts won't run). */
export function svgDataUrl(src: string): string {
  // Prefer base64 so XML special chars / unicode don't break the URL.
  try {
    const b64 = btoa(unescape(encodeURIComponent(src)))
    return `data:image/svg+xml;base64,${b64}`
  } catch {
    return `data:image/svg+xml;charset=utf-8,${encodeURIComponent(src)}`
  }
}

/**
 * CSP for HTML Preview srcdoc (D-html-preview-scripts).
 * Opens https:/wss: so CDN libraries and public APIs work, like claude.ai artifacts or CodePen.
 * The app's own origin (wails:// on macOS/Linux, http://wails.localhost on Windows) and
 * Recur's http://localhost servers stay unreachable to fetch/subresources because only
 * https:/wss:/data:/blob: are listed.
 * Does not block iframe self-navigation (`location` / meta-refresh), accepted residual risk.
 * Combined with iframe sandbox="allow-scripts allow-forms" (no allow-same-origin),
 * so the page gets an opaque origin and cannot touch the parent DOM or its storage.
 */
export const HTML_PREVIEW_CSP =
  "default-src https: data: blob:; script-src https: data: blob: 'unsafe-inline' 'unsafe-eval'; " +
  "style-src https: data: blob: 'unsafe-inline'; connect-src https: wss: data: blob:; " +
  "form-action 'none'; base-uri 'none'; object-src 'none'"

const CSP_META = `<meta http-equiv="Content-Security-Policy" content="${HTML_PREVIEW_CSP}">`

/** Opaque-origin iframe → parent. Parent accepts only messages from that iframe's window. */
export const PREVIEW_IFRAME_SCROLL = 'recur-preview-scroll'
export const PREVIEW_IFRAME_RESTORE = 'recur-preview-restore'
export const PREVIEW_IFRAME_FIND = 'recur-preview-find'
export const PREVIEW_IFRAME_FIND_RESULT = 'recur-preview-find-result'
export const PREVIEW_IFRAME_FIND_KEY = 'recur-preview-find-key'

const HTML_PREVIEW_FIND_STYLE =
  '<style>::highlight(preview-find){background-color:#e6cc7788}' +
  '::highlight(preview-find-current){background-color:#e6a817;color:#111}</style>'

// simplify: opaque origins throw on localStorage/sessionStorage, so pages get in-memory
// stand-ins that reset on every reload. Persist per file via postMessage if anyone needs it.
export const HTML_PREVIEW_STORAGE_SHIM =
  `try{localStorage}catch(_){(function(){var mk=function(){var d=new Map();return{get length(){return d.size},` +
  `key:function(i){var k=Array.from(d.keys())[i];return k===undefined?null:k},` +
  `getItem:function(k){k=String(k);return d.has(k)?d.get(k):null},setItem:function(k,v){d.set(String(k),String(v))},` +
  `removeItem:function(k){d.delete(String(k))},clear:function(){d.clear()}}};` +
  `Object.defineProperty(window,'localStorage',{value:mk(),configurable:true});` +
  `Object.defineProperty(window,'sessionStorage',{value:mk(),configurable:true})})()}`

export const HTML_PREVIEW_SCROLL_SCRIPT =
  `(function(){var T='${PREVIEW_IFRAME_SCROLL}',R='${PREVIEW_IFRAME_RESTORE}',` +
  `F='${PREVIEW_IFRAME_FIND}',FR='${PREVIEW_IFRAME_FIND_RESULT}',K='${PREVIEW_IFRAME_FIND_KEY}';` +
  `function se(){return document.scrollingElement||document.documentElement}` +
  `function send(){var s=se();parent.postMessage({type:T,top:s.scrollTop,left:s.scrollLeft},'*')}` +
  `document.addEventListener('scroll',send,{passive:true,capture:true});` +
  `var matches=[],idx=-1;` +
  `function skip(n){var p=n.parentElement,e,s;if(!n.data||!p)return true;` +
  `if(p.tagName==='SCRIPT'||p.tagName==='STYLE'||p.tagName==='NOSCRIPT')return true;` +
  `for(e=p;e;e=e.parentElement){if(e.hidden)return true;s=getComputedStyle(e);if(s.display==='none'||s.visibility==='hidden')return true}return false}` +
  `function inl(el){var d=getComputedStyle(el).display;return !d||d==='inline'||d==='inline-block'||d==='inline-flex'||d==='inline-grid'||d==='contents'}` +
  `function blk(n){for(var e=n.parentElement;e&&e!==document.body;e=e.parentElement)if(!inl(e))return e;return document.body}` +
  `function paint(){if(!CSS.highlights)return;var rest=[],cur;for(var i=0;i<matches.length;i++){if(i===idx)cur=matches[i];else rest.push(matches[i])}` +
  `if(rest.length)CSS.highlights.set('preview-find',new Highlight(...rest));else CSS.highlights.delete('preview-find');` +
  `if(cur)CSS.highlights.set('preview-find-current',new Highlight(cur));else CSS.highlights.delete('preview-find-current')}` +
  `function reveal(){if(idx<0||!matches[idx])return;try{var r=matches[idx],n=r.startContainer,el=n.nodeType===1?n:n.parentElement,box,p,cs,v,s,oy,ox;` +
  `if(!el)return;box=r.getBoundingClientRect();if(!box.height&&!box.width)return;s=se();` +
  `for(p=el;p&&p!==document.documentElement;p=p.parentElement){if(p===s)continue;cs=getComputedStyle(p);v=p.getBoundingClientRect();` +
  `oy=cs.overflowY||cs.overflow;ox=cs.overflowX||cs.overflow;` +
  `if((oy==='auto'||oy==='scroll')&&p.scrollHeight>p.clientHeight)p.scrollTop+=(box.top+box.bottom)/2-(v.top+v.bottom)/2;` +
  `if((ox==='auto'||ox==='scroll')&&p.scrollWidth>p.clientWidth)p.scrollLeft+=(box.left+box.right)/2-(v.left+v.right)/2;` +
  `box=r.getBoundingClientRect()}` +
  `s.scrollTop+=(box.top+box.bottom)/2-innerHeight/2;s.scrollLeft+=(box.left+box.right)/2-innerWidth/2}catch(e){}}` +
  `function report(ok){parent.postMessage({type:FR,total:matches.length,index:idx<0?0:idx+1,valid:ok!==false},'*')}` +
  `function wc(s,i){var c=s[i];return!!c&&/[\\p{L}\\p{N}_]/u.test(c)}` +
  `function wwok(s,a,b){return !wc(s,a-1)&&!wc(s,b)}` +
  `function collect(q,cs,re,ww){matches=[];idx=-1;if(!q){paint();report(true);return}` +
  `var rx;try{rx=new RegExp(re?q:q.replace(/[.*+?^${'$'}{}()|[\\]\\\\]/g,'\\\\$&'),cs?'g':'gi')}catch(e){paint();report(false);return}` +
  `var hay='',pcs=[],pb=null,br=false,w=document.createTreeWalker(document.body,NodeFilter.SHOW_ELEMENT|NodeFilter.SHOW_TEXT),n,cap=999,i,p,pe,sn,so,en,eo,a,b,m;` +
  `while(n=w.nextNode()){if(n.nodeType===1){if(n.tagName==='BR')br=true;continue}if(skip(n))continue;` +
  `p=blk(n);if(hay&&(br||p!==pb))hay+='\\n';br=false;pb=p;pcs.push({n:n,a:hay.length});hay+=n.data}` +
  `rx.lastIndex=0;while(m=rx.exec(hay)){a=m.index;b=a+m[0].length;if(a===b){rx.lastIndex=a+1;continue}` +
  `if(ww&&!wwok(hay,a,b))continue;sn=so=en=eo=null;` +
  `for(i=0;i<pcs.length;i++){p=pcs[i];pe=p.a+p.n.data.length;if(sn==null&&a>=p.a&&a<pe){sn=p.n;so=a-p.a}if(b>p.a&&b<=pe){en=p.n;eo=b-p.a}}` +
  `if(!sn||!en)continue;p=new Range();p.setStart(sn,so);p.setEnd(en,eo);matches.push(p);if(matches.length>=cap)break}` +
  `idx=matches.length?0:-1;paint();reveal();report(true)}` +
  `function step(d){if(!matches.length)return;idx=(idx+d+matches.length)%matches.length;paint();reveal();report()}` +
  `window.addEventListener('message',function(e){if(!e.data)return;` +
  `if(e.data.type===R){var s=se();s.scrollTop=e.data.top||0;s.scrollLeft=e.data.left||0;return}` +
  `if(e.data.type!==F)return;if(e.data.op==='query')collect(e.data.search||'',!!e.data.caseSensitive,!!e.data.regexp,!!e.data.wholeWord);` +
  `else if(e.data.op==='next')step(1);else if(e.data.op==='prev')step(-1);` +
  `else if(e.data.op==='clear'){matches=[];idx=-1;paint();report()}});` +
  `document.addEventListener('keydown',function(e){var mod=e.metaKey||e.ctrlKey,key=e.key;` +
  `if(mod&&!e.altKey&&(key==='f'||key==='F')&&!e.shiftKey){e.preventDefault();parent.postMessage({type:K,key:'f'},'*')}` +
  `else if(mod&&!e.altKey&&(key==='g'||key==='G')){e.preventDefault();parent.postMessage({type:K,key:e.shiftKey?'G':'g'},'*')}` +
  `else if(key==='F3'){e.preventDefault();parent.postMessage({type:K,key:e.shiftKey?'G':'g'},'*')}` +
  `else if(key==='Escape')parent.postMessage({type:K,key:'Escape'},'*')},true)})();`

/**
 * Prepare HTML for FilePreview srcdoc.
 * Always wrap in a fresh document shell so the CSP meta is parsed first and cannot be
 * bypassed by splicing into attacker-controlled `<head>`/`<html>` markup.
 */
export function htmlPreviewSrcdoc(html: string): string {
  const src = html ?? ''
  return `<!DOCTYPE html><html><head>${CSP_META}${HTML_PREVIEW_FIND_STYLE}<script>${HTML_PREVIEW_STORAGE_SHIM}${HTML_PREVIEW_SCROLL_SCRIPT}</script></head><body>${src}</body></html>`
}
