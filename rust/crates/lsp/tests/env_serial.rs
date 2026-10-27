//! Go `managed_test.go` cases that set `RECUR_LSP_*` overrides. They run in
//! their own test binary, serialized by [`ENV_LOCK`].

static ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

struct EnvGuard(&'static str);

#[allow(unsafe_code)]
fn set_env(key: &'static str, value: &str) -> EnvGuard {
    // SAFETY: every test in this binary holds ENV_LOCK while it runs.
    unsafe { std::env::set_var(key, value) };
    EnvGuard(key)
}

impl Drop for EnvGuard {
    #[allow(unsafe_code)]
    fn drop(&mut self) {
        // SAFETY: still under ENV_LOCK.
        unsafe { std::env::remove_var(self.0) };
    }
}

fn fake(dir: &std::path::Path, name: &str) -> String {
    let p = dir.join(name);
    recur_exec::write_executable(&p, "#!/bin/sh\n", 0o755).unwrap();
    p.to_string_lossy().into_owned()
}

#[tokio::test]
async fn env_overrides_win() {
    let _lock = ENV_LOCK.lock().await;
    let dir = tempfile::tempdir().unwrap();
    for (env, id) in [
        (recur_lsp::managed::ENV_GOPLS, "gopls"),
        (recur_lsp::managed::ENV_RUFF, "ruff"),
        (
            recur_lsp::managed::ENV_TYPESCRIPT,
            "typescript-language-server",
        ),
    ] {
        let bin = fake(dir.path(), id);
        let _g = set_env(env, &bin);
        let data = tempfile::tempdir().unwrap();
        let got = recur_lsp::managed::ensure(&data.path().to_string_lossy(), id)
            .await
            .unwrap();
        assert_eq!(got, bin, "{id}");
    }
}

#[tokio::test]
async fn non_executable_override_is_ignored() {
    let _lock = ENV_LOCK.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("gopls");
    std::fs::write(&p, "x").unwrap();
    let _g = set_env(recur_lsp::managed::ENV_GOPLS, &p.to_string_lossy());
    let data = tempfile::tempdir().unwrap();
    let data = data.path().to_string_lossy().into_owned();
    let bin = recur_lsp::managed::managed_bin_dir(&data, "gopls").join("gopls");
    std::fs::create_dir_all(bin.parent().unwrap()).unwrap();
    fake(bin.parent().unwrap(), "gopls");
    recur_lsp::managed::write_managed_version(&data, "gopls", recur_lsp::managed::GOPLS_RELEASE)
        .unwrap();
    assert_eq!(
        recur_lsp::managed::ensure(&data, "gopls").await.unwrap(),
        bin.to_string_lossy()
    );
}
