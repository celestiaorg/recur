//! How an ACP agent is spawned: its argv, the environment it gets and loses,
//! and the switches that turn off the tools Recur replaces
//! ([`recur_domain::AcpAgent::blocked_tools`]).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use recur_domain::{AcpAgent, AcpToolBlock, PERMISSION_MODE_BYPASS};
use serde_json::{Map, Value, json};

/// One spawn of an agent.
#[derive(Debug, Clone, Default)]
pub struct Launch {
    pub bin: PathBuf,
    /// Arguments after the binary.
    pub args: Vec<String>,
    /// Variables set on the agent.
    pub env: Vec<(String, String)>,
    /// Variables removed from the agent (tracker secrets).
    pub scrub: Vec<String>,
}

impl Launch {
    /// The agent's own args with nothing blocked (tests, fake agents).
    pub fn plain(agent: &AcpAgent, bin: &Path) -> Self {
        Self {
            bin: bin.into(),
            args: agent.args.iter().map(|a| (*a).to_owned()).collect(),
            ..Default::default()
        }
    }

    /// The production launch. Antigravity gets a Recur-owned
    /// `<data_dir>/acp-home/<id>` gemini dir (sign-in stays in the system
    /// keychain) holding the deny hook; OpenCode gets inline config.
    /// `permission_mode` is the turn's Recur permission mode.
    pub fn new(
        agent: &AcpAgent,
        bin: &Path,
        data_dir: &Path,
        scrub: Vec<String>,
        permission_mode: &str,
    ) -> Result<Self, String> {
        let mut l = Self {
            scrub,
            ..Self::plain(agent, bin)
        };
        match agent.tool_block {
            AcpToolBlock::OpenCodeConfig => l.env.push((
                "OPENCODE_CONFIG_CONTENT".into(),
                opencode_config(agent, permission_mode).to_string(),
            )),
            AcpToolBlock::GeminiDirHook => {
                let home = data_dir.join("acp-home").join(agent.id);
                write_if_changed(
                    &home.join("config").join("hooks.json"),
                    &serde_json::to_string_pretty(&gemini_hooks(agent)).unwrap_or_default(),
                )?;
                l.args.push("--gemini-dir".into());
                l.args.push(home.to_string_lossy().into_owned());
            }
        }
        Ok(l)
    }
}

/// `{"permission": {"<tool>": "deny", …}}`. Outside bypass, edits, shell
/// and fetches are set to `ask` so each one reaches Recur's permission gate
/// (OpenCode's own default allows everything).
pub fn opencode_config(agent: &AcpAgent, permission_mode: &str) -> Value {
    let ask = ["edit", "bash", "webfetch"]
        .into_iter()
        .filter(|_| permission_mode != PERMISSION_MODE_BYPASS)
        .map(|t| (t.to_owned(), "ask".into()));
    let deny = agent
        .blocked_tools
        .iter()
        .map(|(tool, _)| ((*tool).to_owned(), "deny".into()));
    json!({ "permission": ask.chain(deny).collect::<Map<String, Value>>() })
}

/// One `PreToolUse` hook per blocked tool that denies the call and names
/// the Recur tool to use. Hook commands run through `sh -c`.
pub fn gemini_hooks(agent: &AcpAgent) -> Value {
    let rules: Vec<Value> = agent
        .blocked_tools
        .iter()
        .map(|(tool, instead)| {
            let out = json!({
                "decision": "deny",
                "reason": format!("Not available in Recur. Use {instead} instead."),
            });
            json!({
                "matcher": format!("^{tool}$"),
                "hooks": [{"type": "command", "command": format!("printf '%s' '{out}'")}],
            })
        })
        .collect();
    json!({ "recur": { "PreToolUse": rules } })
}

/// Replaces `path` atomically when its contents differ, so concurrent turns
/// never read a half-written file.
fn write_if_changed(path: &Path, body: &str) -> Result<(), String> {
    if std::fs::read_to_string(path).is_ok_and(|cur| cur == body) {
        return Ok(());
    }
    let err = |e: std::io::Error| format!("{}: {e}", path.display());
    let dir = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir).map_err(err)?;
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let tmp = dir.join(format!(
        ".hooks.json.{}.{}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::write(&tmp, body).map_err(err)?;
    std::fs::rename(&tmp, path).map_err(err)
}

#[cfg(test)]
mod tests {
    use recur_domain::{PERMISSION_MODE_ASK, PERMISSION_MODE_DENY, acp_agent};

    use super::*;

    #[test]
    fn opencode_denies_through_inline_config() {
        let oc = acp_agent("opencode").unwrap();
        let config = |mode| {
            let l = Launch::new(
                oc,
                Path::new("/bin/opencode"),
                Path::new("/nowhere"),
                vec![],
                mode,
            )
            .unwrap();
            assert_eq!(l.args, ["acp"]);
            let (k, v) = &l.env[0];
            assert_eq!(k, "OPENCODE_CONFIG_CONTENT");
            serde_json::from_str::<Value>(v).unwrap()["permission"].clone()
        };
        for mode in [
            PERMISSION_MODE_ASK,
            PERMISSION_MODE_DENY,
            PERMISSION_MODE_BYPASS,
        ] {
            let p = config(mode);
            for t in ["skill", "task", "todowrite", "question"] {
                assert_eq!(p[t], "deny", "{mode} {t}");
            }
            for t in ["edit", "bash", "webfetch"] {
                let want = if mode == PERMISSION_MODE_BYPASS {
                    Value::Null
                } else {
                    "ask".into()
                };
                assert_eq!(p[t], want, "{mode} {t}");
            }
        }
    }

    #[test]
    fn antigravity_gets_a_recur_home_with_the_deny_hook() {
        let dir = tempfile::tempdir().unwrap();
        let agy = acp_agent("antigravity").unwrap();
        let l = Launch::new(
            agy,
            Path::new("/bin/agy"),
            dir.path(),
            vec![],
            PERMISSION_MODE_ASK,
        )
        .unwrap();
        let home = dir.path().join("acp-home/antigravity");
        assert_eq!(l.args, ["acp", "--gemini-dir", &home.to_string_lossy()]);
        let hooks: Value =
            serde_json::from_str(&std::fs::read_to_string(home.join("config/hooks.json")).unwrap())
                .unwrap();
        let rules = hooks["recur"]["PreToolUse"].as_array().unwrap();
        assert_eq!(rules.len(), agy.blocked_tools.len());
        let schedule = rules.iter().find(|r| r["matcher"] == "^schedule$").unwrap();
        let cmd = schedule["hooks"][0]["command"].as_str().unwrap();
        let out = std::process::Command::new("sh")
            .arg("-c")
            .arg(cmd)
            .output()
            .unwrap();
        let out: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(out["decision"], "deny");
        assert_eq!(
            out["reason"],
            "Not available in Recur. Use mcp__recur__schedule_wakeup instead."
        );
        Launch::new(
            agy,
            Path::new("/bin/agy"),
            dir.path(),
            vec![],
            PERMISSION_MODE_ASK,
        )
        .unwrap();
    }
}
