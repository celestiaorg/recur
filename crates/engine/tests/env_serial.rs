//! The `/skill` cases that need `$HOME` (user-scope skills, Go
//! `skill_mentions_test.go`). They run in their own test binary, serialized
//! by [`ENV_LOCK`].

use std::path::Path;
use std::sync::Mutex;

use recur_engine::composer::skills::wrap_activated_skills;

static ENV_LOCK: Mutex<()> = Mutex::new(());

struct EnvGuard {
    old: Option<std::ffi::OsString>,
}

#[allow(unsafe_code)]
fn set_home(value: &Path) -> EnvGuard {
    let old = std::env::var_os("HOME");
    // SAFETY: every test in this binary holds ENV_LOCK while it runs.
    unsafe { std::env::set_var("HOME", value) };
    EnvGuard { old }
}

impl Drop for EnvGuard {
    #[allow(unsafe_code)]
    fn drop(&mut self) {
        // SAFETY: still under ENV_LOCK (guards drop before the lock).
        unsafe {
            match &self.old {
                Some(v) => std::env::set_var("HOME", v),
                None => std::env::remove_var("HOME"),
            }
        }
    }
}

fn write_skill(root: &Path, scope_dir: &str, name: &str, body: &str) {
    let dir = root.join(scope_dir).join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("SKILL.md"), body).unwrap();
}

#[test]
fn workspace_skill_shadows_user_skill() {
    let _lock = ENV_LOCK.lock().unwrap();
    let home = tempfile::tempdir().unwrap();
    let wt = tempfile::tempdir().unwrap();
    let _env = set_home(home.path());
    write_skill(
        home.path(),
        ".claude/skills",
        "design-audit",
        "---\nname: design-audit\ndescription: user copy\n---\nUSER BODY\n",
    );
    write_skill(
        wt.path(),
        ".claude/skills",
        "design-audit",
        "---\nname: design-audit\ndescription: workspace copy\n---\nWORKSPACE BODY\n",
    );
    let out = wrap_activated_skills("/design-audit", &wt.path().to_string_lossy(), None);
    assert!(out.contains("WORKSPACE BODY"), "{out}");
    assert!(!out.contains("USER BODY"), "{out}");
}

#[test]
fn user_scope_skills_activate_and_respect_invocability() {
    let _lock = ENV_LOCK.lock().unwrap();
    let home = tempfile::tempdir().unwrap();
    let wt = tempfile::tempdir().unwrap();
    let _env = set_home(home.path());
    let w = wt.path().to_string_lossy();
    write_skill(
        home.path(),
        ".agents/skills",
        "go-code-review",
        "---\nname: go-code-review\ndescription: Review Go changes.\n---\n# Go review\n\nCheck error handling.\n",
    );
    write_skill(
        home.path(),
        ".agents/skills",
        "create-rule",
        "---\nname: create-rule\ndescription: Make a rule\ndisable-model-invocation: true\n---\n# Create\n",
    );
    write_skill(
        home.path(),
        ".agents/skills",
        "hidden",
        "---\nname: hidden\ndescription: model only\nuser-invocable: false\n---\n# Hidden\n",
    );
    let out = wrap_activated_skills("/go-code-review look at internal/engine", &w, None);
    assert!(out.contains("<skill_content name=\"go-code-review\">"));
    assert!(out.contains("Check error handling."));
    assert!(out.ends_with("\n\nlook at internal/engine"));
    assert!(
        wrap_activated_skills("/create-rule do it", &w, None)
            .contains("<skill_content name=\"create-rule\">"),
        "user slash still activates a model-disabled skill"
    );
    assert_eq!(
        wrap_activated_skills("/hidden please", &w, None),
        "/hidden please"
    );
}
