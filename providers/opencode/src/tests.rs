use super::*;

#[test]
fn maps_tool_names() {
    assert_eq!(canonical_tool("bash"), "Bash");
    assert_eq!(canonical_tool("edit"), "Edit");
    assert_eq!(canonical_tool("write"), "Edit");
    assert_eq!(canonical_tool("todowrite"), "TodoWrite");
    assert_eq!(canonical_tool("custom"), "custom");
}

#[test]
fn resume_handles_are_opencode_sessions() {
    let p = OpenCodeProvider::new(Settings::default());
    assert!(p.resume_handle_fits(""));
    assert!(p.resume_handle_fits("ses_abc123"));
    assert!(!p.resume_handle_fits("agent-123"));
}

#[test]
fn model_label_comes_from_the_source() {
    let settings = Settings {
        compatible_providers: vec![recur_domain::CompatibleProvider {
            id: "openrouter".into(),
            name: "OpenRouter".into(),
            models: vec![recur_domain::CompatibleModel {
                id: "openai/gpt-5".into(),
                label: "GPT-5".into(),
                ..Default::default()
            }],
            ..Default::default()
        }],
        ..Default::default()
    };
    let p = OpenCodeProvider::new(settings);
    assert_eq!(p.model_label("openrouter/openai/gpt-5"), "GPT-5");
    assert_eq!(p.model_label("openrouter/missing"), "missing");
    assert_eq!(p.model_label("missing/model"), "missing/model");
}

#[cfg(unix)]
#[tokio::test]
async fn runs_a_fake_opencode_and_parses_its_stream() {
    use std::os::unix::fs::PermissionsExt;
    use std::sync::{Arc, Mutex};

    use recur_agent::EventSink;
    use recur_agent::turn::{TurnContext, TurnRequest};
    use tokio_util::sync::CancellationToken;

    let dir = tempfile::tempdir().unwrap();
    let bin = dir.path().join("opencode");
    std::fs::write(
        &bin,
        "#!/bin/sh\n\
         echo '{\"type\":\"text\",\"timestamp\":1,\"sessionID\":\"ses_test\",\"part\":{\"id\":\"p1\",\"type\":\"text\",\"text\":\"Hello\"}}'\n\
         echo '{\"type\":\"step_finish\",\"timestamp\":2,\"sessionID\":\"ses_test\",\"part\":{\"id\":\"p2\",\"type\":\"step-finish\",\"tokens\":{\"input\":7,\"output\":2,\"cache\":{\"read\":1,\"write\":0}}}}'\n\
         exit 0\n",
    )
    .unwrap();
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();

    let settings = Settings {
        opencode_bin_path: bin.to_string_lossy().into_owned(),
        compatible_providers: vec![recur_domain::CompatibleProvider {
            id: "openrouter".into(),
            name: "OpenRouter".into(),
            base_url: "https://openrouter.ai/api".into(),
            models: vec![recur_domain::CompatibleModel {
                id: "gpt-test".into(),
                ..Default::default()
            }],
            ..Default::default()
        }],
        ..Default::default()
    };
    let events = Arc::new(Mutex::new(Vec::new()));
    let sink: Arc<dyn EventSink> = {
        let events = Arc::clone(&events);
        Arc::new(move |ev| events.lock().unwrap().push(ev))
    };
    let req = TurnRequest {
        model: "openrouter/gpt-test".into(),
        prompt: "do it".into(),
        worktree: dir.path().to_path_buf(),
        ..Default::default()
    };
    let provider = OpenCodeProvider::new(settings);
    let result = provider
        .run_turn(req, TurnContext::new(CancellationToken::new(), sink))
        .await
        .unwrap();
    assert_eq!(result.session_id, "ses_test");
    assert_eq!(result.text, "Hello");
    assert!(!result.is_error);
    assert_eq!(result.usage.input_tokens, 7);
    let seen = events.lock().unwrap();
    assert!(
        seen.iter()
            .any(|ev| matches!(ev, AgentEvent::Text { text, .. } if text == "Hello"))
    );
    assert!(seen.iter().any(|ev| matches!(ev, AgentEvent::TurnEnd(_))));
}

#[cfg(unix)]
#[tokio::test]
async fn runs_opencode_in_the_worktree() {
    use std::os::unix::fs::PermissionsExt;
    use std::sync::Arc;

    use recur_agent::EventSink;
    use recur_agent::turn::{TurnContext, TurnRequest};
    use tokio_util::sync::CancellationToken;

    let bin_dir = tempfile::tempdir().unwrap();
    let worktree = tempfile::tempdir().unwrap();
    let seen = bin_dir.path().join("seen");
    let bin = bin_dir.path().join("opencode");
    std::fs::write(
        &bin,
        format!(
            "#!/bin/sh\n\
             echo \"pwd=$PWD\" > '{seen}'\n\
             echo \"args=$*\" >> '{seen}'\n\
             echo '{{\"type\":\"text\",\"timestamp\":1,\"sessionID\":\"ses_dir\",\"part\":{{\"id\":\"p1\",\"type\":\"text\",\"text\":\"ok\"}}}}'\n",
            seen = seen.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    let settings = Settings {
        opencode_bin_path: bin.to_string_lossy().into_owned(),
        compatible_providers: vec![recur_domain::CompatibleProvider {
            id: "deepseek".into(),
            base_url: "https://api.deepseek.com".into(),
            models: vec![recur_domain::CompatibleModel {
                id: "deepseek-chat".into(),
                ..Default::default()
            }],
            ..Default::default()
        }],
        ..Default::default()
    };
    let sink: Arc<dyn EventSink> = Arc::new(|_| {});
    OpenCodeProvider::new(settings)
        .run_turn(
            TurnRequest {
                model: "deepseek/deepseek-chat".into(),
                prompt: "hi".into(),
                worktree: worktree.path().to_path_buf(),
                ..Default::default()
            },
            TurnContext::new(CancellationToken::new(), sink),
        )
        .await
        .unwrap();
    let seen = std::fs::read_to_string(seen).unwrap();
    let wt = worktree.path().display().to_string();
    assert!(seen.contains(&format!("pwd={wt}\n")), "{seen}");
    assert!(seen.contains(&format!("--dir {wt}")), "{seen}");
}
