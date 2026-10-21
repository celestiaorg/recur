//! The I/O half of `lib/promptActions.ts` the chat uses: the `[[prompt]]` catalog cache, arg
//! completion and running a prompt. The pure rules (slash lookup, arg slots, `/` menu gates,
//! tab routing, context, failure wording) are `recur_engine::view::prompts`.

use std::rc::Rc;
use std::time::Duration;

use futures::FutureExt as _;
use futures::future::{LocalBoxFuture, Shared};
use gpui_kit::{App, Global, Task};
use recur_bundles::{PromptAction, PromptArgSpec, PromptRunContext, normalize_prompt_display};
use recur_domain::Settings;
use recur_engine::view::prompts::prompt_enabled;
use recur_services::Services;
use recur_services::api::{
    PromptActionInfo, PromptCompleteHitDto, PromptCompleteRequestDto, PromptCompleteResultDto,
    PromptContextDto, PromptResultDto,
};

const DEFAULT_PROMPT_TIMEOUT: Duration = Duration::from_millis(5_000);
const COMPLETE_TIMEOUT: Duration = Duration::from_millis(4_000);

/// Runs a compiled TypeScript prompt module (`runPromptModule` in the macro worker):
/// `(code, ctx, chat code, prompt name, timeout)`.
pub type PromptModuleRunner = Rc<
    dyn Fn(String, PromptRunContext, String, String, Duration) -> LocalBoxFuture<'static, Result<PromptResultDto, String>>,
>;

/// Runs a compiled TypeScript complete module (`runCompleteModule`); the runner adds the
/// workspace's LSP diagnostics.
pub type CompleteModuleRunner = Rc<
    dyn Fn(String, PromptCompleteRequestDto, Duration) -> LocalBoxFuture<'static, Result<PromptCompleteResultDto, String>>,
>;

type CatalogLoad = Shared<Task<Rc<Vec<PromptActionInfo>>>>;

/// The catalog (`cached` / `cacheErr`) keyed on `AppState::bundles_epoch`, so a bundle
/// reload refetches as `reloadBundles` did with `listPromptActionsUnfiltered(true)`.
#[derive(Default)]
pub struct PromptCatalog {
    cached: Option<(u64, Rc<Vec<PromptActionInfo>>)>,
    inflight: Option<(u64, CatalogLoad)>,
    error: Option<String>,
    prompt_runner: Option<PromptModuleRunner>,
    complete_runner: Option<CompleteModuleRunner>,
}

impl Global for PromptCatalog {}

impl PromptCatalog {
    fn global(cx: &mut App) -> &mut Self {
        cx.default_global::<Self>()
    }

    pub fn set_runners(
        prompt: Option<PromptModuleRunner>,
        complete: Option<CompleteModuleRunner>,
        cx: &mut App,
    ) {
        let g = Self::global(cx);
        g.prompt_runner = prompt;
        g.complete_runner = complete;
    }

    /// The last load's failure (`promptActionsCacheError`).
    pub fn error(cx: &App) -> Option<String> {
        cx.try_global::<Self>().and_then(|g| g.error.clone())
    }

    /// The cached catalog for `epoch` without loading.
    pub fn peek(epoch: u64, cx: &App) -> Option<Rc<Vec<PromptActionInfo>>> {
        let g = cx.try_global::<Self>()?;
        g.cached.as_ref().filter(|(e, _)| *e == epoch).map(|(_, l)| l.clone())
    }

    /// `listPromptActionsUnfiltered`: every discovered prompt including disabled ones. A
    /// failure is not cached, so the next call retries.
    pub fn load(services: &Services, epoch: u64, cx: &mut App) -> CatalogLoad {
        if let Some(list) = Self::peek(epoch, cx) {
            return Task::ready(list).shared();
        }
        if let Some((e, t)) = &Self::global(cx).inflight
            && *e == epoch
        {
            return t.clone();
        }
        let services = services.clone();
        let task = cx
            .spawn(async move |cx| {
                let res = services.bundle.list_prompt_actions().await;
                cx.update(|cx| {
                    let g = Self::global(cx);
                    if g.inflight.as_ref().is_some_and(|(e, _)| *e == epoch) {
                        g.inflight = None;
                    }
                    match res {
                        Ok(list) => {
                            let list = Rc::new(list);
                            g.error = None;
                            g.cached = Some((epoch, list.clone()));
                            list
                        }
                        Err(err) => {
                            g.error = Some(err.to_string());
                            Rc::new(Vec::new())
                        }
                    }
                })
            })
            .shared();
        Self::global(cx).inflight = Some((epoch, task.clone()));
        task
    }
}

/// `filterEnabledPrompts` over the services DTO.
pub fn enabled_prompts(settings: Option<&Settings>, list: &[PromptActionInfo]) -> Vec<PromptActionInfo> {
    list.iter()
        .filter(|p| settings.is_none_or(|s| prompt_enabled(s, &p.id)))
        .cloned()
        .collect()
}

/// The manifest fields the engine's prompt rules read, from the catalog DTO.
pub fn prompt_action(info: &PromptActionInfo) -> PromptAction {
    PromptAction {
        id: info.id.clone(),
        name: info.name.clone(),
        description: info.description.clone(),
        slash: info.slash.clone(),
        mode: info.mode.clone(),
        loop_id: info.r#loop.clone(),
        timeout_ms: info.timeout_ms,
        fresh_session: info.fresh_session,
        tab_group: info.tab_group.clone(),
        interpreter: info.interpreter.clone(),
        takes_args: info.takes_args,
        arg_placeholder: info.arg_placeholder.clone(),
        args: info
            .args
            .iter()
            .map(|a| PromptArgSpec {
                name: a.name.clone(),
                placeholder: a.placeholder.clone(),
                optional: a.optional,
            })
            .collect(),
        complete_interpreter: info.complete_interpreter.clone(),
        bundle_name: info.bundle_name.clone(),
        source_root: info.source_root.clone(),
        ..PromptAction::default()
    }
}

/// Races `fut` against `timer` (`raceAbort`); dropping the returned future cancels.
async fn with_timeout<T>(
    fut: impl Future<Output = Result<T, String>>,
    timer: impl Future<Output = ()>,
) -> Result<T, String> {
    futures::select_biased! {
        r = std::pin::pin!(fut.fuse()) => r,
        _ = std::pin::pin!(timer.fuse()) => Err("prompt timed out".into()),
    }
}

fn normalize_hits(res: PromptCompleteResultDto) -> PromptCompleteResultDto {
    PromptCompleteResultDto {
        hits: Some(
            res.hits
                .unwrap_or_default()
                .into_iter()
                .map(|h| PromptCompleteHitDto {
                    label: if h.label.is_empty() { h.insert.clone() } else { h.label },
                    ..h
                })
                .collect(),
        ),
        message: res.message,
    }
}

/// `completePromptArgs`: one prompt-arg complete script; empty when the prompt has none.
pub fn complete_prompt_args(
    info: &PromptActionInfo,
    req: PromptCompleteRequestDto,
    services: &Services,
    cx: &mut App,
) -> Task<Result<PromptCompleteResultDto, String>> {
    if !info.has_complete {
        return Task::ready(Ok(PromptCompleteResultDto::default()));
    }
    let services = services.clone();
    let id = info.id.clone();
    let timer = cx.background_executor().timer(COMPLETE_TIMEOUT);
    if info.complete_interpreter == "typescript" {
        let runner = cx.try_global::<PromptCatalog>().and_then(|g| g.complete_runner.clone());
        return cx.spawn(async move |_| {
            let code = services
                .bundle
                .compiled_complete_module(&id)
                .await
                .map_err(|e| e.to_string())?;
            let runner = runner.ok_or("TypeScript complete scripts need the macro runner")?;
            runner(code, req, COMPLETE_TIMEOUT).await.map(normalize_hits)
        });
    }
    cx.spawn(async move |_| {
        let run = async {
            services
                .bundle
                .run_prompt_complete(&id, &req)
                .await
                .map_err(|e| e.to_string())
        };
        with_timeout(run, timer).await.map(normalize_hits)
    })
}

/// `toDTO`: the run context the backend's `RunPromptAction` takes.
pub fn prompt_context_dto(ctx: &PromptRunContext, chat_code: &str, timeout: Duration) -> PromptContextDto {
    PromptContextDto {
        workspace_id: ctx.workspace_id.clone(),
        project_id: ctx.project_id.clone(),
        worktree: ctx.worktree.clone(),
        port: ctx.port,
        selected_text: ctx.selected_text.clone(),
        document: ctx.document.clone(),
        current_line: ctx.current_line.clone(),
        line_number: ctx.line_number,
        column: ctx.column,
        current_word: ctx.current_word.clone(),
        filepath: ctx.file_path.clone(),
        filename: ctx.file_name.clone(),
        scope: ctx.scope.clone(),
        clipboard: ctx.clipboard.clone(),
        tab_size: ctx.tab_size,
        soft_tabs: ctx.soft_tabs,
        work_dir: ctx.worktree.clone(),
        timeout_ms: timeout.as_millis() as i64,
        surface: ctx.surface.clone(),
        chat_code: chat_code.to_string(),
        args: ctx.args.clone(),
        params: ctx.params.clone(),
    }
}

/// `runPromptAction`: runs one prompt; does not send chat.
pub fn run_prompt_action(
    info: &PromptActionInfo,
    ctx: PromptRunContext,
    chat_code: &str,
    services: &Services,
    cx: &mut App,
) -> Task<Result<PromptResultDto, String>> {
    let timeout = if info.timeout_ms > 0 {
        Duration::from_millis(info.timeout_ms as u64)
    } else {
        DEFAULT_PROMPT_TIMEOUT
    };
    let services = services.clone();
    let (id, name, chat_code) = (info.id.clone(), info.name.clone(), chat_code.to_string());
    if info.interpreter == "typescript" {
        let runner = cx.try_global::<PromptCatalog>().and_then(|g| g.prompt_runner.clone());
        return cx.spawn(async move |_| {
            let code = services
                .bundle
                .compiled_prompt_module(&id)
                .await
                .map_err(|e| e.to_string())?;
            let runner = runner.ok_or("TypeScript prompts need the macro runner")?;
            runner(code, ctx, chat_code, name, timeout).await
        });
    }
    cx.spawn(async move |_| {
        let res = services
            .bundle
            .run_prompt_action(&id, &prompt_context_dto(&ctx, &chat_code, timeout))
            .await
            .map_err(|e| e.to_string())?;
        Ok(PromptResultDto {
            display: normalize_prompt_display(&res.display, &name),
            ..res
        })
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use gpui::TestAppContext;
    use recur_services::mock::{MockBridge, MockOptions, Reply};
    use serde_json::json;

    use super::*;

    #[gpui::test]
    async fn caches_per_bundle_epoch_and_retries_failures(cx: &mut TestAppContext) {
        let calls = Arc::new(AtomicUsize::new(0));
        let c = calls.clone();
        let bridge = MockBridge::fresh(MockOptions::default().resolver(
            "ListPromptActions",
            move |_, _, _| match c.fetch_add(1, Ordering::SeqCst) {
                0 => Reply::error("boom"),
                _ => Reply::value(json!([{"id": "a", "slash": "commit", "name": "Commit"}])),
            },
        ));
        let services = bridge.services();
        let failed = cx.update(|cx| PromptCatalog::load(&services, 0, cx)).await;
        assert!(failed.is_empty());
        assert_eq!(cx.update(|cx| PromptCatalog::error(cx)).as_deref(), Some("boom"));
        let list = cx.update(|cx| PromptCatalog::load(&services, 0, cx)).await;
        assert_eq!(list[0].slash, "commit");
        let _ = cx.update(|cx| PromptCatalog::load(&services, 0, cx)).await;
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        let _ = cx.update(|cx| PromptCatalog::load(&services, 1, cx)).await;
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }

    #[gpui::test]
    async fn completes_with_label_fallback(cx: &mut TestAppContext) {
        let bridge = MockBridge::fresh(MockOptions::default().resolver(
            "RunPromptComplete",
            |_, _, _| Reply::value(json!({"hits": [{"insert": "ENG-1"}], "message": "m"})),
        ));
        let services = bridge.services();
        let info = PromptActionInfo {
            id: "lin".into(),
            has_complete: true,
            ..Default::default()
        };
        let res = cx
            .update(|cx| complete_prompt_args(&info, PromptCompleteRequestDto::default(), &services, cx))
            .await
            .unwrap();
        let hits = res.hits.unwrap();
        assert_eq!((hits[0].insert.as_str(), hits[0].label.as_str()), ("ENG-1", "ENG-1"));
        assert_eq!(res.message, "m");
        let none = PromptActionInfo::default();
        let empty = cx
            .update(|cx| complete_prompt_args(&none, PromptCompleteRequestDto::default(), &services, cx))
            .await
            .unwrap();
        assert!(empty.hits.is_none());
    }

    #[gpui::test]
    async fn shebang_run_normalizes_display(cx: &mut TestAppContext) {
        let bridge = MockBridge::fresh(MockOptions::default().resolver(
            "RunPromptAction",
            |args, _, _| {
                assert_eq!(args[1]["chatCode"], "PA2");
                assert_eq!(args[1]["timeoutMs"], 5000);
                Reply::value(json!({"prompt": "go", "display": "  two\nlines "}))
            },
        ));
        let services = bridge.services();
        let info = PromptActionInfo {
            id: "c".into(),
            name: "Commit".into(),
            interpreter: "bash".into(),
            ..Default::default()
        };
        let res = cx
            .update(|cx| run_prompt_action(&info, PromptRunContext::default(), "PA2", &services, cx))
            .await
            .unwrap();
        assert_eq!((res.prompt.as_str(), res.display.as_str()), ("go", "two lines"));
    }
}
