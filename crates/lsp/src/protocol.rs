//! Transport-level LSP client rules: answers to server requests the editor
//! client leaves unhandled, `initialize` rewrites, and the Vue ↔ TypeScript
//! `tsserver/request` bridge messages. Pure JSON-in, JSON-out, so the editor's
//! LSP client applies them around [`crate::Manager::write`] and the
//! `lsp.message` stream (parsed once, off the UI thread).

use serde_json::{Map, Value, json};

/// What to do with a message from the server before the editor client sees
/// it.
#[derive(Debug, Clone, PartialEq)]
pub enum ServerRequest {
    /// Not a request this layer answers; deliver it to the client.
    Pass,
    /// Answer with this JSON-RPC response and swallow the request.
    Reply(Value),
    /// A Vue `tsserver/request`: reply `null` now, forward the parsed call to
    /// the workspace's TypeScript session (`workspace/executeCommand`
    /// `typescript.tsserverRequest`), then send [`vue_tsserver_response`] to
    /// the Vue session. `None` when the params do not parse (reply only).
    Tsserver {
        reply: Value,
        call: Option<TsserverRequest>,
    },
}

/// One Vue language-server → tsserver call.
#[derive(Debug, Clone, PartialEq)]
pub struct TsserverRequest {
    pub id: Value,
    pub command: String,
    pub payload: Value,
}

fn reply(id: &Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

/// Classifies a server → client message. `root_uri` is the session root
/// (for `workspace/workspaceFolders`).
pub fn handle_server_request(msg: &Value, root_uri: &str) -> ServerRequest {
    let Value::Object(msg) = msg else {
        return ServerRequest::Pass;
    };
    // Requests carry an id and a method; responses an id and a result or
    // error; notifications only a method.
    let id = match msg.get("id") {
        None | Some(Value::Null) => return ServerRequest::Pass,
        Some(id) => id.clone(),
    };
    let Some(method) = msg.get("method").and_then(Value::as_str) else {
        return ServerRequest::Pass;
    };
    match method {
        "workspace/configuration" => {
            let items = msg
                .get("params")
                .and_then(|p| p.get("items"))
                .and_then(Value::as_array)
                .map_or(&[][..], Vec::as_slice);
            // `null` reads as unset; some servers (taplo) then treat it as
            // include=[] and exclude every document. `{}` keeps defaults.
            // rust-analyzer checks into `target/rust-analyzer` so it never
            // holds the build lock the user's own cargo runs need.
            let config = items
                .iter()
                .map(|item| match item.get("section").and_then(Value::as_str) {
                    Some("rust-analyzer") => json!({"cargo": {"targetDir": true}}),
                    _ => json!({}),
                })
                .collect();
            ServerRequest::Reply(reply(&id, Value::Array(config)))
        }
        "workspace/workspaceFolders" => {
            let folders = if root_uri.is_empty() {
                json!([])
            } else {
                json!([{"uri": root_uri, "name": "workspace"}])
            };
            ServerRequest::Reply(reply(&id, folders))
        }
        // gopls dynamic registration: ack so the server does not stall.
        "client/registerCapability"
        | "client/unregisterCapability"
        | "window/workDoneProgress/create" => ServerRequest::Reply(reply(&id, Value::Null)),
        "tsserver/request" => ServerRequest::Tsserver {
            reply: reply(&id, Value::Null),
            call: parse_vue_tsserver_request(msg.get("params").unwrap_or(&Value::Null)),
        },
        _ => ServerRequest::Pass,
    }
}

/// `[[id, command, payload]]` or `[id, command, payload]`.
pub fn parse_vue_tsserver_request(params: &Value) -> Option<TsserverRequest> {
    let mut tuple = params;
    if let Some(a) = params.as_array()
        && a.len() == 1
        && a[0].is_array()
    {
        tuple = &a[0];
    }
    let t = tuple.as_array().filter(|t| t.len() >= 2)?;
    let command = t[1].as_str().filter(|c| !c.is_empty())?;
    Some(TsserverRequest {
        id: t[0].clone(),
        command: command.to_owned(),
        payload: t.get(2).cloned().unwrap_or(Value::Null),
    })
}

/// The `tsserver/response` notification for the Vue session.
pub fn vue_tsserver_response(id: &Value, body: Option<&Value>) -> Value {
    let body = body.cloned().unwrap_or(Value::Null);
    json!({"jsonrpc": "2.0", "method": "tsserver/response", "params": [[id, body]]})
}

/// The `typescript.tsserverRequest` result body: `r.body` when present,
/// else `r`.
pub fn tsserver_body(result: &Value) -> Value {
    match result.get("body") {
        Some(b) => b.clone(),
        None => result.clone(),
    }
}

fn initialize_params(message: &str) -> Option<(Map<String, Value>, Map<String, Value>)> {
    let Ok(Value::Object(msg)) = serde_json::from_str::<Value>(message) else {
        return None;
    };
    if msg.get("method").and_then(Value::as_str) != Some("initialize") {
        return None;
    }
    let params = msg.get("params")?.as_object()?.clone();
    Some((msg, params))
}

/// Adds `@vue/typescript-plugin` to a TypeScript `initialize` (once).
pub fn with_vue_ts_plugin(message: &str, plugin_path: &str) -> String {
    if plugin_path.is_empty() {
        return message.to_owned();
    }
    let Some((mut msg, mut params)) = initialize_params(message) else {
        return message.to_owned();
    };
    let mut init = params
        .get("initializationOptions")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let mut plugins = init
        .get("plugins")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let has = plugins
        .iter()
        .any(|p| p.get("name").and_then(Value::as_str) == Some("@vue/typescript-plugin"));
    if !has {
        plugins.push(json!({
            "name": "@vue/typescript-plugin",
            "location": plugin_path,
            "languages": ["vue"],
            "configNamespace": "typescript",
        }));
    }
    init.insert("plugins".into(), Value::Array(plugins));
    params.insert("initializationOptions".into(), Value::Object(init));
    msg.insert("params".into(), Value::Object(params));
    Value::Object(msg).to_string()
}

/// The folder for a root URI, named after its last path segment.
pub fn workspace_folder(root_uri: &str) -> Value {
    let name = url::Url::parse(root_uri)
        .ok()
        .and_then(|u| {
            u.path_segments()
                .and_then(|mut s| s.rfind(|p| !p.is_empty()).map(str::to_owned))
        })
        .map(|seg| {
            url::form_urlencoded::parse(format!("x={seg}").replace('+', "%2B").as_bytes())
                .next()
                .map(|(_, v)| v.into_owned())
                .unwrap_or(seg)
        })
        .unwrap_or_else(|| "workspace".into());
    json!({"uri": root_uri, "name": name})
}

/// Adds `workspaceFolders` (and `rootUri`) to `initialize` when missing:
/// the CodeMirror client sends only `rootUri`, and taplo (among others)
/// creates a workspace only from `workspaceFolders`.
pub fn with_workspace_folders(message: &str, root_uri: &str) -> String {
    if root_uri.is_empty() {
        return message.to_owned();
    }
    let Some((mut msg, mut params)) = initialize_params(message) else {
        return message.to_owned();
    };
    if params.get("workspaceFolders").is_none_or(Value::is_null) {
        params.insert(
            "workspaceFolders".into(),
            json!([workspace_folder(root_uri)]),
        );
    }
    if params.get("rootUri").is_none_or(Value::is_null) {
        params.insert("rootUri".into(), json!(root_uri));
    }
    msg.insert("params".into(), Value::Object(params));
    Value::Object(msg).to_string()
}

/// A client → server message's body with every rewrite applied, in order:
/// workspace folders, then the Vue plugin for a TypeScript session that has
/// one. Only `initialize` is rewritten.
pub fn rewrite_outgoing(
    message: &Value,
    root_uri: &str,
    server_key: &str,
    vue_plugin_path: &str,
) -> String {
    let body = message.to_string();
    if message.get("method").and_then(Value::as_str) != Some("initialize") {
        return body;
    }
    let next = with_workspace_folders(&body, root_uri);
    if server_key == "typescript-language-server" && !vue_plugin_path.is_empty() {
        return with_vue_ts_plugin(&next, vue_plugin_path);
    }
    next
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_nested_tsserver_request_and_builds_the_response() {
        let parsed =
            parse_vue_tsserver_request(&json!([[42, "completionInfo", {"file": "App.vue"}]]))
                .unwrap();
        assert_eq!(
            parsed,
            TsserverRequest {
                id: json!(42),
                command: "completionInfo".into(),
                payload: json!({"file": "App.vue"})
            }
        );
        assert_eq!(
            parse_vue_tsserver_request(&json!([7, "quickinfo", null])),
            Some(TsserverRequest {
                id: json!(7),
                command: "quickinfo".into(),
                payload: Value::Null
            })
        );
        assert_eq!(parse_vue_tsserver_request(&Value::Null), None);
        assert_eq!(
            vue_tsserver_response(&json!(42), Some(&json!({"entries": []}))),
            json!({"jsonrpc": "2.0", "method": "tsserver/response", "params": [[42, {"entries": []}]]})
        );
        assert_eq!(
            vue_tsserver_response(&json!(1), None)["params"],
            json!([[1, null]])
        );
    }

    #[test]
    fn injects_vue_plugin_into_typescript_initialize() {
        let raw = json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"rootUri": "file:///tmp/ws"}}).to_string();
        let path = "/data/bin/lsp/vue-language-server/node_modules/@vue/language-server";
        let next: Value = serde_json::from_str(&with_vue_ts_plugin(&raw, path)).unwrap();
        assert_eq!(
            next["params"]["initializationOptions"]["plugins"],
            json!([{"name": "@vue/typescript-plugin", "location": path, "languages": ["vue"], "configNamespace": "typescript"}])
        );
        let twice: Value =
            serde_json::from_str(&with_vue_ts_plugin(&next.to_string(), path)).unwrap();
        assert_eq!(
            twice["params"]["initializationOptions"]["plugins"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            with_vue_ts_plugin(r#"{"method":"initialized"}"#, "/p"),
            r#"{"method":"initialized"}"#
        );
    }

    #[test]
    fn server_requests() {
        let r = |raw: &str| {
            handle_server_request(
                &serde_json::from_str(raw).unwrap_or_default(),
                "file:///root/ws",
            )
        };
        assert_eq!(
            r(
                r#"{"jsonrpc":"2.0","id":3,"method":"workspace/configuration","params":{"items":[{},{}]}}"#
            ),
            ServerRequest::Reply(json!({"jsonrpc": "2.0", "id": 3, "result": [{}, {}]}))
        );
        assert_eq!(
            r(
                r#"{"id":4,"method":"workspace/configuration","params":{"items":[{"section":"rust-analyzer"},{"section":"gopls"}]}}"#
            ),
            ServerRequest::Reply(
                json!({"jsonrpc": "2.0", "id": 4, "result": [{"cargo": {"targetDir": true}}, {}]})
            )
        );
        assert_eq!(
            r(r#"{"id":"a","method":"workspace/workspaceFolders"}"#),
            ServerRequest::Reply(
                json!({"jsonrpc": "2.0", "id": "a", "result": [{"uri": "file:///root/ws", "name": "workspace"}]})
            )
        );
        for m in [
            "client/registerCapability",
            "client/unregisterCapability",
            "window/workDoneProgress/create",
        ] {
            assert_eq!(
                r(&json!({"id": 1, "method": m}).to_string()),
                ServerRequest::Reply(json!({"jsonrpc": "2.0", "id": 1, "result": null}))
            );
        }
        assert_eq!(
            r(r#"{"method":"textDocument/publishDiagnostics"}"#),
            ServerRequest::Pass
        );
        assert_eq!(r(r#"{"id":1,"result":{}}"#), ServerRequest::Pass);
        assert_eq!(r("not json"), ServerRequest::Pass);
        assert!(matches!(
            r(r#"{"id":9,"method":"tsserver/request","params":[[1,"quickinfo",{}]]}"#),
            ServerRequest::Tsserver { call: Some(_), .. }
        ));
        assert_eq!(tsserver_body(&json!({"body": [1]})), json!([1]));
        assert_eq!(tsserver_body(&json!(5)), json!(5));
    }

    #[test]
    fn initialize_gets_workspace_folders() {
        let raw = json!({"id": 1, "method": "initialize", "params": {}}).to_string();
        let next: Value =
            serde_json::from_str(&with_workspace_folders(&raw, "file:///Users/me/my%20proj"))
                .unwrap();
        assert_eq!(
            next["params"]["workspaceFolders"],
            json!([{"uri": "file:///Users/me/my%20proj", "name": "my proj"}])
        );
        assert_eq!(next["params"]["rootUri"], "file:///Users/me/my%20proj");
        let kept = json!({"id": 1, "method": "initialize", "params": {"workspaceFolders": [], "rootUri": "file:///x"}}).to_string();
        let next: Value =
            serde_json::from_str(&with_workspace_folders(&kept, "file:///y")).unwrap();
        assert_eq!(
            (
                next["params"]["workspaceFolders"].clone(),
                next["params"]["rootUri"].clone()
            ),
            (json!([]), json!("file:///x"))
        );
        assert_eq!(workspace_folder("file:///")["name"], "workspace");
        let out = rewrite_outgoing(
            &serde_json::from_str(&raw).unwrap(),
            "file:///r",
            "typescript-language-server",
            "/p",
        );
        let v: Value = serde_json::from_str(&out).unwrap();
        assert!(
            v["params"]["workspaceFolders"].is_array()
                && v["params"]["initializationOptions"]["plugins"].is_array()
        );
        assert_eq!(
            rewrite_outgoing(
                &json!({"method": "initialized"}),
                "file:///r",
                "typescript-language-server",
                "/p"
            ),
            r#"{"method":"initialized"}"#
        );
    }
}
