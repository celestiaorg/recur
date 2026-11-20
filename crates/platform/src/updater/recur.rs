//! Recur's updater wiring: a signed-only endpoint
//! provider, the pinned public key, and the background check schedule.

use std::sync::Arc;
use std::time::Duration;

use futures::future::BoxFuture;

use super::{
    CheckRequest, Config, EndpointConfig, EndpointProvider, Host, Provider, ProviderError, Release,
    State, Updater, UserAction,
};

/// Pins the key release manifests are signed with (`UPDATER_SIGNING_KEY`).
const UPDATER_PUBLIC_KEY: &[u8] = include_bytes!("../../../../build/updater.key.pub");

/// Rejects releases whose manifest carries no signature. The updater only
/// checks a signature that is present, so without this an unsigned manifest
/// would install on its digest alone.
pub struct SignedOnly<P>(pub P);

#[derive(Debug, thiserror::Error)]
#[error("updater: release manifest is not signed")]
struct Unsigned;

impl<P: Provider> Provider for SignedOnly<P> {
    fn name(&self) -> &str {
        self.0.name()
    }

    fn check<'a>(
        &'a self,
        req: &'a CheckRequest,
    ) -> BoxFuture<'a, Result<Option<Release>, ProviderError>> {
        Box::pin(async move {
            let rel = self.0.check(req).await?;
            if let Some(r) = &rel
                && r.verification
                    .as_ref()
                    .is_none_or(|v| v.signature.is_empty())
            {
                return Err(Box::new(Unsigned) as ProviderError);
            }
            Ok(rel)
        })
    }

    fn download<'a>(
        &'a self,
        rel: &'a Release,
        dst: &'a mut (dyn std::io::Write + Send),
        on_progress: &'a mut (dyn FnMut(i64, i64) + Send),
    ) -> BoxFuture<'a, Result<(), ProviderError>> {
        self.0.download(rel, dst, on_progress)
    }
}

/// The updater for release app builds.
pub struct RecurUpdater {
    updater: Arc<Updater>,
    schedule: tokio::task::JoinHandle<()>,
}

impl RecurUpdater {
    /// Wires the headless updater and reports whether it did: `None` unless
    /// this is a release build (`version` and `manifest_url` stamped at build
    /// time) running from an app bundle. Starts the background schedule on
    /// the current tokio runtime: first check after a minute, then every six
    /// hours. The UI renders every [`super::UpdaterEvent`] as a toast.
    pub fn install(version: &str, manifest_url: &str, host: Arc<dyn Host>) -> Option<Self> {
        Self::install_with(version, manifest_url, host, crate::running_in_app_bundle())
    }

    fn install_with(
        version: &str,
        manifest_url: &str,
        host: Arc<dyn Host>,
        in_bundle: bool,
    ) -> Option<Self> {
        if version.is_empty() || manifest_url.is_empty() || !in_bundle {
            return None;
        }
        let updater = match Self::configure(version, manifest_url, host) {
            Ok(u) => u,
            Err(e) => {
                tracing::warn!("updater: {e}");
                return None;
            }
        };
        let schedule = tokio::spawn({
            let updater = Arc::clone(&updater);
            async move {
                let mut delay = Duration::from_secs(60);
                loop {
                    tokio::time::sleep(delay).await;
                    check(&updater).await;
                    delay = Duration::from_secs(6 * 60 * 60);
                }
            }
        });
        Some(Self { updater, schedule })
    }

    fn configure(
        version: &str,
        manifest_url: &str,
        host: Arc<dyn Host>,
    ) -> Result<Arc<Updater>, String> {
        // No total timeout: it would also bound the ~50 MB body on slow links.
        let client = recur_http::client(|b| b.connect_timeout(Duration::from_secs(30)));
        let endpoint = EndpointProvider::new(EndpointConfig {
            url: manifest_url.to_string(),
            client: Some(client),
            response_header_timeout: Some(Duration::from_secs(30)),
            ..EndpointConfig::default()
        })
        .map_err(|e| e.to_string())?;
        let updater = Updater::new(host);
        updater
            .init(Config {
                current_version: version.to_string(),
                providers: vec![Arc::new(SignedOnly(endpoint))],
                public_key: UPDATER_PUBLIC_KEY.to_vec(),
                helper_ready_timeout: Duration::ZERO,
                platform: String::new(),
                arch: String::new(),
            })
            .map_err(|e| e.to_string())?;
        Ok(updater)
    }

    /// The `updater.check` event (menu "Check for Updates…"): checks unless
    /// a flow is already under way.
    pub fn check_now(&self) {
        let updater = Arc::clone(&self.updater);
        self.updater.spawn(async move { check(&updater).await });
    }

    /// Forwards an `updater:user:*` event from the UI.
    pub fn user_action(&self, action: UserAction) {
        self.updater.user_action(action);
    }

    pub fn updater(&self) -> &Arc<Updater> {
        &self.updater
    }
}

impl Drop for RecurUpdater {
    fn drop(&mut self) {
        self.schedule.abort();
    }
}

/// From Ready the check compares against the staged version, so only a
/// newer release replaces it.
async fn check(updater: &Updater) {
    if matches!(
        updater.state(),
        State::Idle | State::UpToDate | State::Error | State::Ready
    ) {
        let _ = updater.check_and_install().await;
    }
}

#[cfg(test)]
mod tests {
    use super::super::Verification;
    use super::*;

    struct Fake(Option<Release>);

    impl Provider for Fake {
        fn name(&self) -> &str {
            "fake"
        }

        fn check<'a>(
            &'a self,
            _req: &'a CheckRequest,
        ) -> BoxFuture<'a, Result<Option<Release>, ProviderError>> {
            Box::pin(async move { Ok(self.0.clone()) })
        }

        fn download<'a>(
            &'a self,
            _rel: &'a Release,
            _dst: &'a mut (dyn std::io::Write + Send),
            _on_progress: &'a mut (dyn FnMut(i64, i64) + Send),
        ) -> BoxFuture<'a, Result<(), ProviderError>> {
            Box::pin(async { Ok(()) })
        }
    }

    fn rel(verification: Option<Verification>) -> Option<Release> {
        Some(Release {
            version: "2.0.0".into(),
            verification,
            ..Release::default()
        })
    }

    #[tokio::test]
    async fn signed_only_rejects_unsigned_releases() {
        let cases = [
            ("up to date", None, false),
            ("no verify", rel(None), true),
            (
                "digest only",
                rel(Some(Verification {
                    digest: vec![1],
                    ..Verification::default()
                })),
                true,
            ),
            (
                "signed",
                rel(Some(Verification {
                    signature: vec![1],
                    ..Verification::default()
                })),
                false,
            ),
        ];
        for (name, release, want_err) in cases {
            let got = SignedOnly(Fake(release.clone()))
                .check(&CheckRequest::default())
                .await;
            assert_eq!(got.is_err(), want_err, "{name}");
            if let Ok(r) = got {
                assert_eq!(r, release, "{name}: release passed through");
            }
        }
    }

    struct NullHost;

    impl Host for NullHost {
        fn emit(&self, _event: super::super::UpdaterEvent) {}
        fn quit(&self) {}
    }

    #[tokio::test]
    async fn install_gates_on_release_bundle_builds() {
        let host: Arc<dyn Host> = Arc::new(NullHost);
        assert!(
            RecurUpdater::install_with("", "https://u/m.json", Arc::clone(&host), true).is_none()
        );
        assert!(RecurUpdater::install_with("1.0.0", "", Arc::clone(&host), true).is_none());
        assert!(
            RecurUpdater::install_with("1.0.0", "https://u/m.json", Arc::clone(&host), false)
                .is_none()
        );
        let u =
            RecurUpdater::install_with("1.0.0", "https://u/m.json", host, true).expect("installed");
        assert_eq!(u.updater().state(), State::Idle);
        assert_eq!(u.updater().current_version(), "1.0.0");
    }
}
