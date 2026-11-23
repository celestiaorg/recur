//! browser_screenshot's renderer, which the desktop installs on the engine: the URL is the
//! one asked for, else the workspace browser's page, else the workspace's start URL.

use gpui_kit::{App, Task};
use recur_content::browser::{AVAILABLE, normalize_url};
use recur_content::browser_shot;
use recur_engine::browser::{BROWSER_UNAVAILABLE, BrowserCapture, BrowserShot, BrowserShots};

pub fn capture_browser(req: BrowserCapture, cx: &mut App) -> Task<Result<BrowserShots, String>> {
    if !AVAILABLE {
        return Task::ready(Err(BROWSER_UNAVAILABLE.into()));
    }
    let url = if req.url.is_empty() {
        let shown = crate::browser::existing_browser(&req.workspace_id, cx)
            .map(|b| b.read(cx).current_url(cx))
            .unwrap_or_default();
        [shown, req.start_url]
            .iter()
            .map(|u| normalize_url(u))
            .find(|u| !u.is_empty())
    } else {
        Some(normalize_url(&req.url)).filter(|u| !u.is_empty())
    };
    let Some(url) = url else {
        return Task::ready(Err(if req.url.is_empty() {
            "no URL to load: pass url, open a page in the workspace's Browser, or set the \
project's app URL"
                .into()
        } else {
            format!("{:?} is not a URL", req.url)
        }));
    };
    let shots = browser_shot::capture(url.clone(), req.widths, req.blocked_ports, req.out_dir, cx);
    cx.spawn(async move |_| {
        let page = shots.await?;
        Ok(BrowserShots {
            url,
            shots: page
                .shots
                .into_iter()
                .map(|s| BrowserShot {
                    width: s.width,
                    height: s.height,
                    path: s
                        .path
                        .map(|p| p.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                    error: s.error,
                })
                .collect(),
            console_errors: page.console_errors,
            exceptions: page.exceptions,
            failed_requests: page.failed_requests,
        })
    })
}
