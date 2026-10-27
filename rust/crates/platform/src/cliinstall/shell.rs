//! Login-shell PATH probe.

use std::io::Read;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// Wraps `$PATH` so rc-file greetings (oh-my-zsh, nvm) cannot be mistaken for
/// the value. Same idea VS Code and GitHub Desktop use.
const PATH_MARK: &str = "__RECUR_PATH_MARK__";

/// Bounds the probe: a blocking read in .zshrc must not stall launch.
const SHELL_PATH_TIMEOUT: Duration = Duration::from_secs(5);

/// The login shell's PATH, probed once per process; empty when it fails.
pub(super) fn capture_login_path() -> String {
    static CAPTURED: OnceLock<String> = OnceLock::new();
    CAPTURED
        .get_or_init(|| read_shell_path(&user_login_shell(), SHELL_PATH_TIMEOUT))
        .clone()
}

fn user_login_shell() -> String {
    login_shell_from(std::env::var("SHELL").ok().as_deref(), passwd_shell)
}

fn login_shell_from(shell_env: Option<&str>, passwd: impl FnOnce() -> String) -> String {
    if let Some(sh) = shell_env.map(str::trim).filter(|s| !s.is_empty()) {
        return sh.to_string();
    }
    let sh = passwd();
    let sh = sh.trim();
    if !sh.is_empty() {
        return sh.to_string();
    }
    if cfg!(target_os = "macos") {
        "/bin/zsh".to_string()
    } else {
        "/bin/sh".to_string()
    }
}

fn passwd_shell() -> String {
    if cfg!(target_os = "macos") {
        let sh = darwin_id_shell();
        if !sh.is_empty() {
            return sh;
        }
    }
    shell_from_passwd_file(Path::new("/etc/passwd"), rustix::process::getuid().as_raw())
}

/// getpwuid for macOS without libc: `id -P` prints the Directory Services
/// passwd line, whose last field is the login shell.
fn darwin_id_shell() -> String {
    let Some(out) =
        crate::run_with_timeout(Command::new("id").arg("-P"), Duration::from_secs(1), false)
    else {
        return String::new();
    };
    if !out.success {
        return String::new();
    }
    let text = String::from_utf8_lossy(&out.output);
    let fields: Vec<&str> = text.trim().split(':').collect();
    if fields.len() < 10 {
        return String::new();
    }
    fields[fields.len() - 1].to_string()
}

fn shell_from_passwd_file(path: &Path, uid: u32) -> String {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return String::new();
    };
    let want = uid.to_string();
    raw.split('\n')
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| l.split(':').collect::<Vec<_>>())
        .find(|f| f.len() >= 7 && f[2] == want)
        .map(|f| f[6].to_string())
        .unwrap_or_default()
}

fn shell_path_args(shell: &str) -> Vec<String> {
    let name = Path::new(shell)
        .file_name()
        .map(|n| n.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    match name.trim_start_matches('-') {
        "fish" => vec![
            "-l".into(),
            "-i".into(),
            "-c".into(),
            format!("printf '{PATH_MARK}%s{PATH_MARK}' (string join : $PATH)"),
        ],
        _ => vec![
            "-ilc".into(),
            format!("printf '{PATH_MARK}%s{PATH_MARK}' \"$PATH\""),
        ],
    }
}

fn read_shell_path(shell: &str, timeout: Duration) -> String {
    if shell.trim().is_empty() {
        return String::new();
    }
    let timeout = if timeout.is_zero() {
        SHELL_PATH_TIMEOUT
    } else {
        timeout
    };
    // stdin closed so rc files cannot block on a prompt; own process group so
    // a timeout kills everything the rc files started.
    let spawned = Command::new(shell)
        .args(shell_path_args(shell))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn();
    let Ok(mut child) = spawned else {
        return String::new();
    };
    let Some(mut stdout) = child.stdout.take() else {
        return String::new();
    };
    let reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stdout.read_to_end(&mut buf);
        buf
    });
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            Ok(None) => {
                if let Some(pgid) = i32::try_from(child.id())
                    .ok()
                    .and_then(rustix::process::Pid::from_raw)
                {
                    let _ =
                        rustix::process::kill_process_group(pgid, rustix::process::Signal::KILL);
                }
                let _ = child.wait();
                break None;
            }
            Err(_) => break None,
        }
    };
    let out = reader.join().unwrap_or_default();
    match status {
        Some(s) if s.success() => {
            parse_path_marks(&String::from_utf8_lossy(&out)).unwrap_or_default()
        }
        _ => String::new(),
    }
}

fn parse_path_marks(out: &str) -> Option<String> {
    let parts: Vec<&str> = out.split(PATH_MARK).collect();
    if parts.len() < 3 {
        return None;
    }
    let p = parts[1].trim();
    if p.is_empty() || p.contains(['\n', '\r']) {
        return None;
    }
    Some(p.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_marks() {
        let junk = format!(
            "oh-my-zsh loading...\n{PATH_MARK}/opt/homebrew/bin:/Users/me/.local/bin{PATH_MARK}\n"
        );
        assert_eq!(
            parse_path_marks(&junk).as_deref(),
            Some("/opt/homebrew/bin:/Users/me/.local/bin")
        );
        assert_eq!(parse_path_marks("no markers here /usr/bin"), None);
        assert_eq!(parse_path_marks(&format!("{PATH_MARK}{PATH_MARK}")), None);
        assert_eq!(
            parse_path_marks(&format!("{PATH_MARK}/bin\n/hack{PATH_MARK}")),
            None
        );
    }

    #[test]
    fn args_per_shell() {
        let zsh = shell_path_args("/bin/zsh");
        assert_eq!(zsh.len(), 2);
        assert_eq!(zsh[0], "-ilc");
        let fish = shell_path_args("/opt/homebrew/bin/fish");
        assert_eq!(fish[..3], ["-l", "-i", "-c"]);
        assert!(fish[3].contains("string join"));
        assert_eq!(shell_path_args("-zsh")[0], "-ilc");
    }

    #[test]
    fn login_shell_prefers_shell_env() {
        assert_eq!(
            login_shell_from(Some("/opt/homebrew/bin/fish"), || panic!(
                "passwd not consulted"
            )),
            "/opt/homebrew/bin/fish"
        );
        assert_eq!(
            login_shell_from(Some("  "), || "/bin/bash".into()),
            "/bin/bash"
        );
        let fallback = login_shell_from(None, String::new);
        assert!(fallback == "/bin/zsh" || fallback == "/bin/sh");
    }

    #[test]
    fn passwd_file_lookup() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("passwd");
        std::fs::write(
            &path,
            "# comment\nnobody:*:-2:-2:Unprivileged User:/var/empty:/usr/bin/false\n\
             josh:*:501:20:Josh:/Users/josh:/bin/zsh\n",
        )
        .expect("write");
        assert_eq!(shell_from_passwd_file(&path, 501), "/bin/zsh");
        assert_eq!(shell_from_passwd_file(&path, 999), "");
    }

    fn fake_shell(dir: &Path, body: &str) -> String {
        let p = dir.join("shell");
        recur_exec::write_executable(&p, body, 0o755).expect("write");
        p.to_string_lossy().into_owned()
    }

    #[test]
    fn extracts_path_from_junk() {
        let dir = tempfile::tempdir().expect("tempdir");
        let shell = fake_shell(
            dir.path(),
            r#"#!/bin/sh
echo "nvm: loading..."
cmd=""
while [ $# -gt 0 ]; do
  case "$1" in
    -ilc|-c) shift; cmd=$1 ;;
    -l|-i) ;;
    *) if [ -z "$cmd" ]; then cmd=$1; fi ;;
  esac
  shift
done
PATH=/captured/bin:/usr/bin
export PATH
eval "$cmd"
"#,
        );
        assert_eq!(
            read_shell_path(&shell, Duration::from_secs(30)),
            "/captured/bin:/usr/bin"
        );
    }

    #[test]
    fn times_out() {
        let dir = tempfile::tempdir().expect("tempdir");
        let shell = fake_shell(dir.path(), "#!/bin/sh\nsleep 5\n");
        let start = Instant::now();
        assert_eq!(read_shell_path(&shell, Duration::from_millis(200)), "");
        assert!(start.elapsed() < Duration::from_secs(3));
    }

    #[test]
    fn missing_binary() {
        let dir = tempfile::tempdir().expect("tempdir");
        let missing = dir.path().join("nope");
        assert_eq!(
            read_shell_path(&missing.to_string_lossy(), Duration::from_secs(1)),
            ""
        );
    }
}
