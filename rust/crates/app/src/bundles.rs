//! Navigate → Reload Bundles (`lib/bundle-reload.ts`): re-read installed bundles without a
//! restart. Views holding bundle-derived state (grammars, snippets, prompt actions) rebuild
//! off `AppState::bundles_epoch`.

use gpui_kit::{App, Entity};
use recur_ui::widgets::ToastKind;
use recur_view_core::{AppState, bridge_error_message, toast};

pub fn reload_bundles(state: &Entity<AppState>, cx: &mut App) {
    let services = state.read(cx).services().clone();
    let state = state.clone();
    cx.spawn(async move |cx| {
        let res = async {
            services.bundle.invalidate_catalog().await?;
            state.update(cx, |s, _| s.invalidate_command_cache());
            let skipped = services
                .bundle
                .list_skipped_bundles()
                .await
                .unwrap_or_default();
            let catalogs = state.update(cx, |s, cx| s.load_theme_catalogs(cx));
            catalogs.await;
            state.update(cx, |s, cx| {
                s.bundles_epoch += 1;
                cx.notify();
            });
            Ok::<_, recur_services::Error>(skipped.len())
        }
        .await;
        cx.update(|cx| match res {
            Ok(0) => toast(ToastKind::Success, "Bundles reloaded", cx),
            Ok(n) => toast(
                ToastKind::Info,
                format!("Bundles reloaded, {n} skipped (see Settings → Bundles)"),
                cx,
            ),
            Err(err) => toast(
                ToastKind::Error,
                format!("Reload bundles failed: {}", bridge_error_message(&err)),
                cx,
            ),
        });
    })
    .detach();
}
