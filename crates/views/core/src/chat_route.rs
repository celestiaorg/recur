//! Inbox / palette route targets (`lib/chat-route.ts`).

/// `chat:<workspaceId>:<code>` (workspace ids may contain dots).
pub fn parse_chat_route_target(target: &str) -> Option<(String, String)> {
    let rest = target.strip_prefix("chat:")?;
    let (workspace_id, code) = rest.split_once(':').unwrap_or((rest, ""));
    (!workspace_id.is_empty()).then(|| (workspace_id.to_string(), code.to_string()))
}

/// An Observatory panel a workspace target can open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RoutePanel {
    Plan,
    Findings,
    Review,
}

impl RoutePanel {
    pub fn as_str(self) -> &'static str {
        match self {
            RoutePanel::Plan => "plan",
            RoutePanel::Findings => "findings",
            RoutePanel::Review => "review",
        }
    }
}

/// `workspace:<id>` / `loop:<id>` with an optional `:plan` / `:findings` / `:review` panel.
pub fn parse_workspace_route_target(target: &str) -> Option<(String, Option<RoutePanel>)> {
    let t = target.trim();
    let (kind, rest) = t.split_once(':')?;
    if kind != "workspace" && kind != "loop" {
        return None;
    }
    let rest = rest.trim();
    if rest.is_empty() {
        return None;
    }
    let Some((workspace_id, panel)) = rest.split_once(':') else {
        return Some((rest.to_string(), None));
    };
    if workspace_id.is_empty() {
        return None;
    }
    let panel = match panel {
        "plan" => Some(RoutePanel::Plan),
        "findings" => Some(RoutePanel::Findings),
        "review" => Some(RoutePanel::Review),
        _ => None,
    };
    Some((workspace_id.to_string(), panel))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chat(t: &str) -> Option<(String, String)> {
        parse_chat_route_target(t)
    }

    fn pair(w: &str, c: &str) -> Option<(String, String)> {
        Some((w.into(), c.into()))
    }

    #[test]
    fn chat_targets() {
        assert_eq!(
            chat("chat:demo.golden-burrito:PA2"),
            pair("demo.golden-burrito", "PA2")
        );
        assert_eq!(chat("chat:LOOP-2218:MA"), pair("LOOP-2218", "MA"));
        assert_eq!(chat("workspace:LOOP-2218"), None);
        assert_eq!(chat("chat:"), None);
        assert_eq!(chat("pr:482"), None);
    }

    #[test]
    fn workspace_targets() {
        let p = |t: &str| parse_workspace_route_target(t);
        assert_eq!(p("workspace:LOOP-2218"), Some(("LOOP-2218".into(), None)));
        assert_eq!(
            p("workspace:recur.zesty-quiche:plan"),
            Some(("recur.zesty-quiche".into(), Some(RoutePanel::Plan)))
        );
        assert_eq!(
            p("loop:W1:findings"),
            Some(("W1".into(), Some(RoutePanel::Findings)))
        );
        assert_eq!(
            p("workspace:W1:review"),
            Some(("W1".into(), Some(RoutePanel::Review)))
        );
        assert_eq!(p("chat:W1:PA"), None);
        assert_eq!(p("workspace:"), None);
        assert_eq!(p("pr:12"), None);
    }
}
