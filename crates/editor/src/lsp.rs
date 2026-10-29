//! Language server UI hooks: the [`LspClient`] trait the editor drives
//! (implemented by the LSP port) and the editor-side logic of
//! `@codemirror/lsp-client` as Recur's `lsp.ts` configures it: hover
//! tooltips, signature help, formatting (Shift-Alt-f), rename (F2), jump to
//! definition (F12 / Mod-click), the completion source, and server
//! diagnostics mapped through unsynced edits.
//!
//! Positions on the wire are LSP `{line, character}` with UTF-16
//! characters; the editor converts to and from its byte offsets.

use std::rc::Rc;

use futures::future::LocalBoxFuture;
use serde_json::{Value, json};

use crate::autocomplete::{
    Completion, CompletionApply, CompletionContext, CompletionResult, CompletionSource, Doc,
    SourceResult, ValidFor,
};
use crate::change::{Change, ChangeDesc, ChangeSet, MapMode};
use crate::state::{Dispatch, EditorState, Transaction, TransactionSpec};
use crate::text::Text;

/// A failed request (`{code, message}` or a transport error).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LspError {
    pub code: Option<i64>,
    pub message: String,
}

impl LspError {
    /// JSON-RPC `RequestCancelled`.
    pub fn is_cancelled(&self) -> bool {
        self.code == Some(-32800)
    }
}

pub type LspFuture<T> = LocalBoxFuture<'static, Result<T, LspError>>;

/// An in-flight request: its id (for `$/cancelRequest`) and the response.
pub struct LspRequest {
    pub id: u64,
    pub response: LspFuture<Value>,
}

/// What the editor needs from a language server session. The LSP port owns
/// the transport, initialization, document versions and `didChange`
/// batching; the editor reports its edits and issues requests.
pub trait LspClient {
    /// `hasCapability`: `None` before initialization.
    fn has_capability(&self, name: &str) -> Option<bool>;
    /// The server's capabilities (`serverCapabilities`).
    fn server_capabilities(&self) -> Option<Value>;
    /// Send a request.
    fn request(&self, method: &str, params: Value) -> LspRequest;
    /// `$/cancelRequest` for a request id.
    fn cancel_request(&self, id: u64);
    /// Push pending document changes to the server (`client.sync()`).
    fn sync(&self);
    /// The editor opened `uri` with this text (`workspace.openFile`).
    fn open_file(&self, uri: &str, language_id: &str, text: &Text);
    /// The editor closed `uri`.
    fn close_file(&self, uri: &str);
    /// The editor changed `uri` (unsynced until the next `sync`).
    fn file_changed(&self, uri: &str, prev: &Text, changes: &ChangeSet, next: &Text);
    /// Show another file for a cross-file jump (`workspace.displayFile`);
    /// the host moves the caret there with [`LspJump`].
    fn display_file(&self, uri: &str, target: LspJump) -> LspFuture<bool>;
    /// Apply a `WorkspaceEdit` across files (`applyWorkspaceEdit`); false
    /// when some file could not be loaded.
    fn apply_workspace_edit(&self, edit: Value) -> LspFuture<bool>;
}

/// Where a cross-file jump lands, in LSP coordinates of the target file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LspJump {
    pub line: u32,
    pub character: u32,
}

/// `toPosition`: byte offset to `{line, character}` (UTF-16).
pub fn to_position(doc: &Text, pos: usize) -> Value {
    let line = doc.line_at(pos);
    json!({
        "line": line.number - 1,
        "character": doc.byte_to_utf16(pos) - doc.byte_to_utf16(line.from),
    })
}

fn pos_fields(pos: &Value) -> Option<(i64, i64)> {
    Some((pos.get("line")?.as_i64()?, pos.get("character")?.as_i64()?))
}

/// `contentChangesFor`: a `didChange` body for `changes` from `start` to
/// `doc`. Full text when the server syncs fully or the document is under
/// 1024 UTF-16 units; otherwise one ranged event per change, last first,
/// all in `start`'s coordinates.
pub fn content_changes_for(
    doc: &Text,
    start: &Text,
    changes: &ChangeSet,
    incremental: bool,
) -> Vec<Value> {
    if !incremental || doc.byte_to_utf16(doc.len()) < 1024 {
        return vec![json!({ "text": doc.to_string() })];
    }
    let mut events = Vec::new();
    changes.iter_changes(false, |from_a, to_a, _, _, inserted| {
        events.push(json!({
            "range": { "start": to_position(start, from_a), "end": to_position(start, to_a) },
            "text": inserted.to_string(),
        }));
    });
    events.reverse();
    events
}

/// `fromPosition`: `{line, character}` to a byte offset (clamped to the
/// document like CodeMirror's `doc.line()` would throw past it).
pub fn from_position(doc: &Text, pos: &Value) -> Option<usize> {
    let (line, ch) = pos_fields(pos)?;
    let n = usize::try_from(line + 1).ok()?.clamp(1, doc.lines());
    let l = doc.line(n);
    let base = doc.byte_to_utf16(l.from);
    Some(doc.utf16_to_byte(base + usize::try_from(ch).ok()?))
}

/// `fromPositionChecked`: `None` outside the document.
pub fn from_position_checked(doc: &Text, pos: &Value) -> Option<usize> {
    let (line, ch) = pos_fields(pos)?;
    if line < 0 || line as usize >= doc.lines() {
        return None;
    }
    let l = doc.line(line as usize + 1);
    let len16 = doc.byte_to_utf16(l.to) - doc.byte_to_utf16(l.from);
    if ch < 0 || ch as usize > len16 {
        return None;
    }
    Some(doc.utf16_to_byte(doc.byte_to_utf16(l.from) + ch as usize))
}

/// Server documentation as the editor renders it (`docToHTML`):
/// `MarkupContent` keeps its kind, bare strings use `default_kind`.
pub fn doc_of(value: &Value, default_markdown: bool) -> Doc {
    match value {
        Value::String(s) => Doc {
            markdown: default_markdown,
            value: s.clone(),
        },
        Value::Object(o) => Doc {
            markdown: o.get("kind").and_then(Value::as_str) != Some("plaintext"),
            value: o
                .get("value")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
        },
        _ => Doc {
            markdown: false,
            value: String::new(),
        },
    }
}

/// A rendered hover section: markdown, or code in a language.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HoverPart {
    Doc(Doc),
    /// `MarkedString {language, value}`: shown as code (no Lezer
    /// highlighting in Recur, so escaped text).
    Code {
        language: String,
        value: String,
    },
}

/// `renderTooltipContent`.
pub fn hover_parts(contents: &Value) -> Vec<HoverPart> {
    let code = |v: &Value| match v {
        Value::String(s) => HoverPart::Doc(Doc {
            markdown: true,
            value: s.clone(),
        }),
        Value::Object(o) if o.contains_key("language") => HoverPart::Code {
            language: o
                .get("language")
                .and_then(Value::as_str)
                .unwrap_or("")
                .into(),
            value: o.get("value").and_then(Value::as_str).unwrap_or("").into(),
        },
        other => HoverPart::Doc(doc_of(other, false)),
    };
    match contents {
        Value::Array(items) => items.iter().map(code).collect(),
        Value::String(_) => vec![code(contents)],
        Value::Object(o) if o.contains_key("language") => vec![code(contents)],
        other => vec![HoverPart::Doc(doc_of(other, false))],
    }
}

/// A hover result anchored to a range.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HoverInfo {
    pub from: usize,
    pub to: usize,
    pub parts: Vec<HoverPart>,
}

/// Signature help state (`SignatureState`).
#[derive(Clone, Debug, PartialEq)]
pub struct SignatureState {
    pub data: Value,
    pub active: usize,
    pub pos: usize,
}

/// One rendered signature: label with the active parameter span (byte
/// range of the label), counter and docs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignatureView {
    pub label: String,
    pub active_param: Option<(usize, usize)>,
    /// `"2/3"` when there are several signatures.
    pub counter: Option<String>,
    pub doc: Option<Doc>,
}

fn utf16_range_to_bytes(s: &str, from: usize, to: usize) -> (usize, usize) {
    let conv = |u: usize| {
        let mut acc = 0;
        for (b, c) in s.char_indices() {
            if acc >= u {
                return b;
            }
            acc += c.len_utf16();
        }
        s.len()
    };
    (conv(from), conv(to))
}

impl SignatureState {
    /// `drawSignatureTooltip`.
    pub fn view(&self) -> Option<SignatureView> {
        let sigs = self.data.get("signatures")?.as_array()?;
        let sig = sigs.get(self.active)?;
        let label = sig.get("label")?.as_str()?.to_string();
        let active_n = sig
            .get("activeParameter")
            .or_else(|| self.data.get("activeParameter"))
            .and_then(Value::as_u64);
        let param =
            active_n.and_then(|n| sig.get("parameters")?.as_array()?.get(n as usize).cloned());
        let mut active_param = None;
        if let Some(p) = param {
            match p.get("label") {
                Some(Value::Array(a)) if a.len() == 2 => {
                    let (f, t) = (
                        a[0].as_u64().unwrap_or(0) as usize,
                        a[1].as_u64().unwrap_or(0) as usize,
                    );
                    if t > 0 {
                        // A server can send a reversed range; `slice` makes it empty.
                        let (f, t) = utf16_range_to_bytes(&label, f, t);
                        active_param = Some((f, t.max(f)));
                    }
                }
                Some(Value::String(s)) => {
                    if let Some(i) = label.find(s.as_str())
                        && !s.is_empty()
                    {
                        active_param = Some((i, i + s.len()));
                    }
                }
                _ => {}
            }
        }
        Some(SignatureView {
            counter: (sigs.len() > 1).then(|| format!("{}/{}", self.active + 1, sigs.len())),
            doc: sig.get("documentation").map(|d| doc_of(d, false)),
            label,
            active_param,
        })
    }

    fn same_signatures(a: &Value, b: &Value) -> bool {
        let (Some(x), Some(y)) = (
            a.get("signatures").and_then(Value::as_array),
            b.get("signatures").and_then(Value::as_array),
        ) else {
            return false;
        };
        x.len() == y.len()
            && x.iter()
                .zip(y)
                .all(|(p, q)| p.get("label") == q.get("label"))
    }

    fn same_active_param(a: &Value, b: &Value, active: usize) -> bool {
        let param = |v: &Value| {
            v.get("signatures")
                .and_then(|s| s.get(active))
                .and_then(|s| s.get("activeParameter"))
                .or_else(|| v.get("activeParameter"))
                .cloned()
        };
        param(a) == param(b)
    }
}

/// Completion item kinds to icon types (`kindToType`).
pub fn kind_to_type(kind: u64) -> Option<&'static str> {
    Some(match kind {
        1 => "text",
        2 => "method",
        3 => "function",
        4 | 7 | 22 => "class",
        5 | 10 => "property",
        6 => "variable",
        8 => "interface",
        9 => "namespace",
        11 | 14 => "keyword",
        12 | 13 | 16 | 20 | 21 => "constant",
        25 => "type",
        _ => return None,
    })
}

/// `lspToSnippet`: drop backslashes before `$}\`, `$1` to `${1}`.
pub fn lsp_to_snippet(text: &str) -> String {
    let b = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\' && matches!(b.get(i + 1), Some(b'$') | Some(b'}') | Some(b'\\')) {
            out.push(b[i + 1] as char);
            i += 2;
            continue;
        }
        if b[i] == b'$' {
            let n = b[i + 1..].iter().take_while(|c| c.is_ascii_digit()).count();
            if n > 0 {
                out.push_str(&format!("${{{}}}", &text[i + 1..i + 1 + n]));
                i += 1 + n;
                continue;
            }
        }
        let ch = text[i..].chars().next().unwrap_or('$');
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

fn item_text(item: &Value) -> String {
    item.get("textEdit")
        .and_then(|e| e.get("newText"))
        .or_else(|| item.get("textEditText"))
        .or_else(|| item.get("insertText"))
        .or_else(|| item.get("label"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

/// `prefixRegexp`: `validFor` covering non-word prefixes of the items.
pub fn prefix_regexp(items: &[Value]) -> String {
    let step = items.len().div_ceil(50).max(1);
    let mut prefixes: Vec<String> = Vec::new();
    let mut i = 0;
    while i < items.len() {
        let text = item_text(&items[i]);
        let starts_word = text
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_');
        if !starts_word {
            let prefix: String = text
                .chars()
                .take_while(|c| !(c.is_ascii_alphanumeric() || *c == '_'))
                .collect();
            if !prefixes.contains(&prefix) {
                prefixes.push(prefix);
            }
        }
        i += step;
    }
    if prefixes.is_empty() {
        return "^\\w*$".into();
    }
    let escaped: Vec<String> = prefixes
        .iter()
        .map(|p| {
            p.chars()
                .map(|c| {
                    if c.is_alphanumeric() || c == '_' || c.is_whitespace() {
                        c.to_string()
                    } else {
                        format!("\\{c}")
                    }
                })
                .collect()
        })
        .collect();
    format!("^(?:{})?\\w*$", escaped.join("|"))
}

fn trigger_kind(client: &dyn LspClient, ch: &str) -> Option<&'static str> {
    let triggers = client.server_capabilities().and_then(|c| {
        c.get("completionProvider")?
            .get("triggerCharacters")?
            .as_array()
            .cloned()
    });
    if triggers.is_some_and(|t| t.iter().any(|x| x.as_str() == Some(ch))) {
        return Some("triggerCharacter");
    }
    ch.chars()
        .next()
        .filter(|c| c.is_ascii_alphabetic() || *c == '_')
        .map(|_| "identifier")
}

/// An edit to apply after a completion (`additionalTextEdits`).
#[derive(Clone, Debug, PartialEq, Eq)]
struct ExtraEdit {
    from: usize,
    to: usize,
    text: String,
}

fn apply_edits_fn(
    edits: Vec<ExtraEdit>,
    text: String,
    mapped: Option<ChangeDesc>,
) -> crate::autocomplete::ApplyFn {
    Rc::new(move |d: &mut dyn Dispatch, _c: &Completion, from, to| {
        let base = crate::autocomplete::insert_completion_text(d.state(), &text, from, to);
        let mut changes = Vec::new();
        for e in &edits {
            let (mut f, mut t) = (e.from, e.to);
            if let Some(m) = &mapped {
                if m.touches_range(f, t) != crate::change::Touch::No {
                    continue;
                }
                let len = t - f;
                f = m.map_pos(f, 1);
                t = f + len;
            }
            changes.push(Change::new(f, t, e.text.clone()));
        }
        let state = d.state().clone();
        let tr = state.update_all(vec![base, TransactionSpec::default().changes(changes)]);
        d.dispatch(TransactionSpec {
            changes: crate::state::ChangesInput::Set(tr.changes.clone()),
            selection: tr.selection.clone(),
            effects: tr.effects.clone(),
            annotations: tr.annotations.clone(),
            scroll_into_view: tr.scroll_into_view,
            ..Default::default()
        });
    })
}

/// `serverCompletionSource`.
pub struct LspCompletionSource {
    pub client: Rc<dyn LspClient>,
    pub uri: String,
}

impl CompletionSource for LspCompletionSource {
    fn complete(&self, cx: &CompletionContext) -> SourceResult {
        let client = self.client.clone();
        let state = &cx.state;
        let trigger_char =
            state.slice_doc(state.doc.clamp_to_char(cx.pos.saturating_sub(1)), cx.pos);
        let reason = if cx.explicit {
            Some("invoked")
        } else {
            trigger_kind(client.as_ref(), &trigger_char)
        };
        let Some(reason) = reason else {
            return SourceResult::Ready(None);
        };
        if client.has_capability("completionProvider") == Some(false) {
            return SourceResult::Ready(None);
        }
        client.sync();
        let context = if reason == "triggerCharacter" {
            json!({ "triggerKind": 2, "triggerCharacter": trigger_char })
        } else {
            json!({ "triggerKind": 1 })
        };
        let params = json!({
            "position": to_position(&state.doc, cx.pos),
            "textDocument": { "uri": self.uri },
            "context": context,
        });
        let req = client.request("textDocument/completion", params);
        let id = req.id;
        let abort_client = client.clone();
        cx.abort
            .add_listener(move || abort_client.cancel_request(id), false);
        let doc = state.doc.clone();
        let pos = cx.pos;
        let word = state.word_at(pos);
        SourceResult::Pending(Box::pin(async move {
            let result = match req.response.await {
                Ok(v) => v,
                Err(e) if e.is_cancelled() => return Ok(None),
                Err(e) => return Err(e.message),
            };
            if result.is_null() {
                return Ok(None);
            }
            let (items, defaults, incomplete) = match &result {
                Value::Array(a) => (a.clone(), None, false),
                other => (
                    other
                        .get("items")
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_default(),
                    other.get("itemDefaults").cloned(),
                    other
                        .get("isIncomplete")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                ),
            };
            let (from, to) = result_range(&doc, pos, word, &items, defaults.as_ref());
            let default_commit = defaults
                .as_ref()
                .and_then(|d| d.get("commitCharacters"))
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|c| c.as_str().map(str::to_string))
                        .collect::<Vec<_>>()
                });
            let mut options = Vec::new();
            for item in &items {
                let text = item_text(item);
                let label = item
                    .get("label")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let mut option = Completion {
                    label: item
                        .get("filterText")
                        .and_then(Value::as_str)
                        .map_or(label.clone(), str::to_string),
                    display_label: Some(label),
                    kind: item
                        .get("kind")
                        .and_then(Value::as_u64)
                        .and_then(kind_to_type)
                        .map(str::to_string),
                    ..Completion::default()
                };
                let commit: Option<Vec<String>> = item
                    .get("commitCharacters")
                    .and_then(Value::as_array)
                    .map(|a| {
                        a.iter()
                            .filter_map(|c| c.as_str().map(str::to_string))
                            .collect()
                    });
                if commit.is_some() && commit != default_commit {
                    option.commit_characters = commit;
                }
                option.detail = item
                    .get("detail")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                option.sort_text = item
                    .get("sortText")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                let format = item
                    .get("insertTextFormat")
                    .or_else(|| defaults.as_ref().and_then(|d| d.get("insertTextFormat")))
                    .and_then(Value::as_u64);
                if format == Some(2) {
                    let template = lsp_to_snippet(&text);
                    option.apply = Some(CompletionApply::Fn(Rc::new(
                        move |d: &mut dyn Dispatch, _c: &Completion, from, to| {
                            crate::snippet::apply_template(d, &template, from, to, true);
                        },
                    )));
                } else if let Some(extra) =
                    item.get("additionalTextEdits").and_then(Value::as_array)
                {
                    let edits: Vec<ExtraEdit> = extra
                        .iter()
                        .filter_map(|e| {
                            let range = e.get("range")?;
                            Some(ExtraEdit {
                                from: from_position_checked(&doc, range.get("start")?)?,
                                to: from_position_checked(&doc, range.get("end")?)?,
                                text: e.get("newText")?.as_str()?.to_string(),
                            })
                        })
                        .collect();
                    option.apply = Some(CompletionApply::Fn(apply_edits_fn(
                        edits,
                        text.clone(),
                        None,
                    )));
                } else if option.label != text {
                    option.apply = Some(CompletionApply::Text(text.clone()));
                }
                if let Some(d) = item.get("documentation") {
                    option.info = Some(doc_of(d, false));
                }
                options.push(option);
            }
            let mut out = CompletionResult::new(from, options);
            out.to = Some(to);
            out.commit_characters = default_commit;
            if !incomplete {
                out.valid_for = ValidFor::regex(&prefix_regexp(&items), "");
            }
            Ok(Some(out))
        }))
    }
}

/// `completionResultRange`.
fn result_range(
    doc: &Text,
    pos: usize,
    word: Option<crate::selection::SelectionRange>,
    items: &[Value],
    defaults: Option<&Value>,
) -> (usize, usize) {
    if items.is_empty() {
        return (pos, pos);
    }
    let range = defaults
        .and_then(|d| d.get("editRange"))
        .map(|r| r.get("insert").unwrap_or(r).clone())
        .or_else(|| {
            items[0].get("textEdit").map(|e| {
                e.get("range")
                    .or_else(|| e.get("insert"))
                    .cloned()
                    .unwrap_or(Value::Null)
            })
        });
    let Some(range) = range.filter(|r| !r.is_null()) else {
        return word.map_or((pos, pos), |w| (w.from, w.to));
    };
    let line = doc.line_at(pos);
    let ch = |p: Option<&Value>| {
        p.and_then(|p| p.get("character"))
            .and_then(Value::as_u64)
            .unwrap_or(0) as usize
    };
    let base = doc.byte_to_utf16(line.from);
    (
        doc.utf16_to_byte(base + ch(range.get("start"))),
        doc.utf16_to_byte(base + ch(range.get("end"))),
    )
}

/// One editor's connection to a language server (`LSPPlugin`): the uri,
/// the synced document and changes made since the last sync.
pub struct LspAttachment {
    pub client: Rc<dyn LspClient>,
    pub uri: String,
    pub language_id: String,
    pub synced_doc: Text,
    pub unsynced: ChangeSet,
    pub signature: Option<SignatureState>,
    /// The request whose response may still be shown (`activeRequest`).
    pub sig_request: Option<(u64, usize)>,
}

impl LspAttachment {
    pub fn new(
        client: Rc<dyn LspClient>,
        uri: &str,
        language_id: &str,
        doc: &Text,
    ) -> LspAttachment {
        client.open_file(uri, language_id, doc);
        LspAttachment {
            client,
            uri: uri.to_string(),
            language_id: language_id.to_string(),
            synced_doc: doc.clone(),
            unsynced: ChangeSet::empty(doc.len()),
            signature: None,
            sig_request: None,
        }
    }

    pub(crate) fn document_changed(&mut self, prev: &Text, changes: &ChangeSet, next: &Text) {
        self.unsynced = self.unsynced.compose(changes);
        self.client.file_changed(&self.uri, prev, changes, next);
    }

    /// `client.sync()`; the editor's unsynced changes are now synced.
    pub fn sync(&mut self, doc: &Text) {
        self.client.sync();
        self.synced_doc = doc.clone();
        self.unsynced = ChangeSet::empty(doc.len());
    }

    /// Map a server position (in the synced document) to the current one
    /// (`serverDiagnostics`).
    pub fn map_synced(&self, pos: &Value) -> Option<usize> {
        let p = from_position(&self.synced_doc, pos)?;
        Some(self.unsynced.map_pos(p, -1))
    }

    /// The signature tooltip follows the document.
    pub(crate) fn after_transaction(&mut self, tr: &Transaction, _state: &EditorState) {
        if let Some(sig) = self.signature.as_mut()
            && tr.doc_changed()
        {
            sig.pos = tr.changes.map_pos(sig.pos, -1);
        }
        if let Some((id, pos)) = self.sig_request.as_mut() {
            if tr.selection.is_some() {
                let _ = id;
                self.sig_request = None;
            } else if tr.doc_changed() {
                *pos = tr.changes.map_pos(*pos, -1);
            }
        }
    }

    /// Signature help trigger after an update: a typed trigger character
    /// (or retrigger character while open), else a delayed content-change
    /// request when the selection moved with the tooltip open.
    pub fn signature_trigger(&self, tr: &Transaction) -> Option<SignatureTrigger> {
        let caps = self.client.server_capabilities()?;
        let conf = caps.get("signatureHelpProvider")?;
        let mut trigger = None;
        if tr.doc_changed() && tr.is_user_event("input.type") {
            let mut triggers: Vec<String> = conf
                .get("triggerCharacters")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|c| c.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            if self.signature.is_some() {
                triggers.extend(
                    conf.get("retriggerCharacters")
                        .and_then(Value::as_array)
                        .map(|a| {
                            a.iter()
                                .filter_map(|c| c.as_str().map(str::to_string))
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default(),
                );
            }
            tr.changes.iter_changes(false, |_, _, _, _, ins| {
                for ch in &triggers {
                    if !ins.is_empty() && ins.contains(ch.as_str()) {
                        trigger = Some(ch.clone());
                    }
                }
            });
        }
        if let Some(ch) = trigger {
            return Some(SignatureTrigger::Character(ch));
        }
        if self.signature.is_some() && tr.selection.is_some() {
            return Some(SignatureTrigger::ContentChange);
        }
        None
    }

    /// Request signature help (`startRequest`).
    pub fn signature_request(&mut self, state: &EditorState, context: Value) -> Option<LspRequest> {
        if self.client.has_capability("signatureHelpProvider") == Some(false) {
            return None;
        }
        self.client.sync();
        self.synced_doc = state.doc.clone();
        self.unsynced = ChangeSet::empty(state.doc.len());
        let pos = state.selection.main().head();
        let req = self.client.request(
            "textDocument/signatureHelp",
            json!({
                "context": context,
                "position": to_position(&state.doc, pos),
                "textDocument": { "uri": self.uri },
            }),
        );
        self.sig_request = Some((req.id, pos));
        Some(req)
    }

    /// A signature response arrived; returns whether the tooltip changed.
    pub fn signature_response(
        &mut self,
        id: u64,
        result: Option<Value>,
        content_change: bool,
    ) -> bool {
        let Some((rid, pos)) = self.sig_request else {
            return false;
        };
        if rid != id {
            return false;
        }
        let has = result
            .as_ref()
            .and_then(|r| r.get("signatures")?.as_array().map(|a| !a.is_empty()))
            .unwrap_or(false);
        if let (true, Some(result)) = (has, result) {
            let cur = self.signature.as_ref();
            let same = cur.is_some_and(|c| SignatureState::same_signatures(&c.data, &result));
            let active = if same && content_change {
                cur.map_or(0, |c| c.active)
            } else {
                result
                    .get("activeSignature")
                    .and_then(Value::as_u64)
                    .unwrap_or(0) as usize
            };
            if same
                && cur.is_some_and(|c| SignatureState::same_active_param(&c.data, &result, active))
            {
                return false;
            }
            let p = if same {
                cur.map_or(pos, |c| c.pos)
            } else {
                pos
            };
            self.signature = Some(SignatureState {
                data: result,
                active,
                pos: p,
            });
            return true;
        }
        if self.signature.is_some() {
            self.signature = None;
            return true;
        }
        false
    }

    /// `nextSignature` / `prevSignature`; `None` without a tooltip.
    pub fn step_signature(&mut self, forward: bool) -> Option<bool> {
        let sig = self.signature.as_mut()?;
        let n = sig
            .data
            .get("signatures")
            .and_then(Value::as_array)
            .map_or(0, Vec::len);
        if forward && sig.active + 1 < n {
            sig.active += 1;
        } else if !forward && sig.active > 0 {
            sig.active -= 1;
        }
        Some(true)
    }

    /// `textDocument/hover` at `pos`.
    pub fn hover_request(&mut self, state: &EditorState, pos: usize) -> Option<LspRequest> {
        if self.client.has_capability("hoverProvider") == Some(false) {
            return None;
        }
        self.sync(&state.doc);
        Some(self.client.request(
            "textDocument/hover",
            json!({ "position": to_position(&state.doc, pos), "textDocument": { "uri": self.uri } }),
        ))
    }

    /// The hover tooltip for a response.
    pub fn hover_info(doc: &Text, pos: usize, result: &Value) -> Option<HoverInfo> {
        if result.is_null() {
            return None;
        }
        let range = result.get("range");
        let from = range
            .and_then(|r| r.get("start"))
            .and_then(|p| from_position(doc, p))
            .unwrap_or(pos);
        let to = range
            .and_then(|r| r.get("end"))
            .and_then(|p| from_position(doc, p))
            .unwrap_or(pos);
        Some(HoverInfo {
            from,
            to,
            parts: hover_parts(result.get("contents")?),
        })
    }

    /// `textDocument/formatting` with the editor's indent unit.
    pub fn format_request(&mut self, state: &EditorState) -> LspRequest {
        self.sync(&state.doc);
        let unit = state.indent_unit_width();
        self.client.request(
            "textDocument/formatting",
            json!({
                "options": { "tabSize": unit, "insertSpaces": !state.indent_unit().contains('\t') },
                "textDocument": { "uri": self.uri },
            }),
        )
    }

    /// Changes for a formatting response, mapped through edits made while
    /// it was in flight (`since`); `None` when one of them was touched.
    pub fn format_changes(
        start_doc: &Text,
        since: &ChangeDesc,
        response: &Value,
    ) -> Option<Vec<Change>> {
        let edits = response.as_array()?;
        let mut changes = Vec::new();
        for e in edits {
            let range = e.get("range")?;
            let mut from = from_position(start_doc, range.get("start")?)?;
            let mut to = from_position(start_doc, range.get("end")?)?;
            if !since.is_empty() {
                if since.touches_range(from, to) != crate::change::Touch::No {
                    return None;
                }
                from = since.map_pos(from, 1);
                to = since.map_pos(to, -1);
            }
            changes.push(Change::new(
                from,
                to,
                e.get("newText")?.as_str()?.to_string(),
            ));
        }
        Some(changes)
    }

    /// `textDocument/rename` for the word at the cursor.
    pub fn rename_request(&mut self, state: &EditorState, new_name: &str) -> Option<LspRequest> {
        let word = state.word_at(state.selection.main().head())?;
        if self.client.has_capability("renameProvider") != Some(true) {
            return None;
        }
        self.sync(&state.doc);
        Some(self.client.request(
            "textDocument/rename",
            json!({
                "newName": new_name,
                "position": to_position(&state.doc, word.from),
                "textDocument": { "uri": self.uri },
            }),
        ))
    }

    /// `textDocument/definition` at the cursor; `None` when unsupported.
    pub fn definition_request(&mut self, state: &EditorState) -> Option<LspRequest> {
        if self.client.has_capability("definitionProvider") == Some(false) {
            return None;
        }
        self.sync(&state.doc);
        Some(self.client.request(
            "textDocument/definition",
            json!({
                "textDocument": { "uri": self.uri },
                "position": to_position(&state.doc, state.selection.main().head()),
            }),
        ))
    }

    /// The first location of a definition response: uri and start.
    pub fn first_location(response: &Value) -> Option<(String, Value)> {
        let loc = match response {
            Value::Array(a) => a.first()?.clone(),
            Value::Null => return None,
            other => other.clone(),
        };
        let uri = loc
            .get("uri")
            .or_else(|| loc.get("targetUri"))?
            .as_str()?
            .to_string();
        let range = loc
            .get("range")
            .or_else(|| loc.get("targetSelectionRange"))?;
        Some((uri, range.get("start")?.clone()))
    }
}

impl Drop for LspAttachment {
    fn drop(&mut self) {
        self.client.close_file(&self.uri);
    }
}

/// What triggered a signature request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SignatureTrigger {
    Character(String),
    ContentChange,
}

/// Delay before a content-change signature request (`250ms`).
pub const SIGNATURE_RETRIGGER_MS: u64 = 250;
/// Hover delay (`hoverTime` default).
pub const HOVER_TIME_MS: u64 = 300;
/// Automatic sync after edits (`autoSyncClients`).
pub const AUTO_SYNC_MS: u64 = 500;
/// Hover tooltip placement inset from the scroller (`TOOLTIP_EDGE_MARGIN`).
pub const TOOLTIP_EDGE_MARGIN: f32 = 12.0;

/// `MapMode` re-export for diagnostics mapping.
pub fn map_through(changes: &ChangeDesc, pos: usize) -> Option<usize> {
    changes.map_pos_mode(pos, -1, MapMode::Simple)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positions_are_utf16() {
        let doc = Text::of("aé😀b\nx");
        let p = to_position(&doc, 7);
        assert_eq!(p, json!({"line": 0, "character": 4}));
        assert_eq!(
            from_position(&doc, &json!({"line": 0, "character": 4})),
            Some(7)
        );
        assert_eq!(
            from_position_checked(&doc, &json!({"line": 0, "character": 6})),
            None
        );
        assert_eq!(
            from_position(&doc, &json!({"line": 1, "character": 1})),
            Some(10)
        );
    }

    #[test]
    fn snippets_and_prefixes() {
        assert_eq!(lsp_to_snippet("fmt.Println($1)$0"), "fmt.Println(${1})${0}");
        assert_eq!(lsp_to_snippet("a\\$b\\}"), "a$b}");
        assert_eq!(prefix_regexp(&[json!({"label": "foo"})]), "^\\w*$");
        assert_eq!(prefix_regexp(&[json!({"label": "@bar"})]), "^(?:\\@)?\\w*$");
        assert_eq!(kind_to_type(3), Some("function"));
        assert_eq!(kind_to_type(99), None);
    }

    #[test]
    fn hover_and_signature_rendering() {
        let parts = hover_parts(&json!([{"language": "go", "value": "func f()"}, "docs"]));
        assert_eq!(parts.len(), 2);
        assert!(matches!(parts[0], HoverPart::Code { .. }));
        let sig = SignatureState {
            data: json!({"signatures": [{"label": "f(a int, b int)", "parameters": [{"label": "a int"}, {"label": [9, 14]}]}], "activeParameter": 1}),
            active: 0,
            pos: 0,
        };
        let v = sig.view().unwrap();
        assert_eq!(v.active_param, Some((9, 14)));
        assert_eq!(v.counter, None);
        let reversed = SignatureState {
            data: json!({"signatures": [{"label": "f(\u{e9}, b)", "parameters": [{"label": [6, 2]}]}], "activeParameter": 0}),
            active: 0,
            pos: 0,
        };
        let (f, t) = reversed.view().unwrap().active_param.unwrap();
        assert!(f <= t && "f(\u{e9}, b)".is_char_boundary(f) && "f(\u{e9}, b)".is_char_boundary(t));
    }

    #[test]
    fn format_changes_map_and_skip_touched() {
        let doc = Text::of("a\nbb");
        let resp = json!([{"range": {"start": {"line": 1, "character": 0}, "end": {"line": 1, "character": 2}}, "newText": "B"}]);
        let none = ChangeDesc::default();
        assert_eq!(
            LspAttachment::format_changes(&doc, &none, &resp),
            Some(vec![Change::new(2, 4, "B")])
        );
        let typed = ChangeSet::of(&[Change::insert(0, "x")], 4).desc();
        assert_eq!(
            LspAttachment::format_changes(&doc, &typed, &resp),
            Some(vec![Change::new(3, 5, "B")])
        );
        let touched = ChangeSet::of(&[Change::insert(3, "x")], 4).desc();
        assert_eq!(LspAttachment::format_changes(&doc, &touched, &resp), None);
    }

    #[test]
    fn content_changes_full_or_ranged() {
        let small = Text::of("ab");
        let cs = ChangeSet::of(&[Change::insert(1, "x")], 2);
        let next = cs.apply(&small);
        assert_eq!(
            content_changes_for(&next, &small, &cs, true),
            vec![json!({"text": "axb"})]
        );
        let big_src = format!("{}\nend", "x".repeat(1100));
        let big = Text::of(&big_src);
        let cs = ChangeSet::of(
            &[Change::insert(0, "a"), Change::new(1101, 1104, "END")],
            big.len(),
        );
        let next = cs.apply(&big);
        assert_eq!(
            content_changes_for(&next, &big, &cs, true),
            vec![
                json!({"range": {"start": {"line": 1, "character": 0}, "end": {"line": 1, "character": 3}}, "text": "END"}),
                json!({"range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 0}}, "text": "a"}),
            ]
        );
        assert_eq!(content_changes_for(&next, &big, &cs, false).len(), 1);
    }
}
