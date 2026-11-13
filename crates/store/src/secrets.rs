//! Where [`Secrets`] live instead of recur.db: the OS credential store
//! (macOS Keychain via `/usr/bin/security`, Secret Service via
//! `secret-tool` on Linux), falling back to a `0600` `secrets.json` beside
//! the database when that store is missing or fails.
//!
//! The Keychain is driven through `security` so no item ACL is tied to the
//! Recur binary (no prompt after every rebuild or update) and values reach
//! it on stdin, never argv.

use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
#[cfg(not(test))]
use std::process::{Command, Stdio};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use parking_lot::Mutex;
use recur_domain::Secrets;

#[cfg(not(test))]
const SERVICE: &str = "Recur";
/// `security -i` reads at most 4096 bytes per command line.
const CHUNK: usize = 3000;
/// Ends every chunk that has a successor at `<account>#<n>`.
const MORE: char = '~';

pub(crate) struct SecretVault {
    /// The OS store account (the data dir); `None` keeps secrets in `file`.
    account: Option<String>,
    file: PathBuf,
    /// The last value written, so unrelated settings saves skip the store.
    /// `None` after a failed load: saving then would overwrite secrets this
    /// session never saw.
    saved: Mutex<Option<Secrets>>,
}

impl SecretVault {
    pub fn new(db_path: &Path, use_os_store: bool) -> Self {
        let dir = db_path.parent().unwrap_or(Path::new("."));
        Self {
            account: use_os_store.then(|| dir.display().to_string()),
            file: dir.join("secrets.json"),
            saved: Mutex::new(Some(Secrets::default())),
        }
    }

    /// The fallback file when present (a later OS write removes it, so it is
    /// never stale), else the OS store.
    pub fn load(&self) -> io::Result<Secrets> {
        let r = self.read();
        *self.saved.lock() = r.as_ref().ok().cloned();
        r
    }

    fn read(&self) -> io::Result<Secrets> {
        let json = match std::fs::read_to_string(&self.file) {
            Ok(s) => Some(s),
            Err(e) if e.kind() == io::ErrorKind::NotFound => match &self.account {
                Some(a) => os_get(a)?,
                None => None,
            },
            Err(e) => return Err(e),
        };
        match json {
            Some(j) => serde_json::from_str(&j).map_err(io::Error::other),
            None => Ok(Secrets::default()),
        }
    }

    pub fn save(&self, s: &Secrets) -> io::Result<()> {
        let mut saved = self.saved.lock();
        match &*saved {
            None => return Err(io::Error::other("secrets failed to load")),
            Some(prev) if prev == s => return Ok(()),
            Some(_) => {}
        }
        let json = serde_json::to_string(s).map_err(io::Error::other)?;
        let os = match &self.account {
            Some(a) => os_set(a, &json),
            None => Err(io::ErrorKind::Unsupported.into()),
        };
        match os {
            Ok(()) => match std::fs::remove_file(&self.file) {
                Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
                _ => {}
            },
            Err(e) => {
                if self.account.is_some() {
                    tracing::warn!(
                        "credential store unavailable ({e}); keeping secrets in {}",
                        self.file.display()
                    );
                }
                write_private(&self.file, &json)?;
            }
        }
        *saved = Some(s.clone());
        Ok(())
    }
}

fn write_private(path: &Path, data: &str) -> io::Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
    tmp.write_all(data.as_bytes())?;
    tmp.as_file().sync_all()?;
    tmp.persist(path).map_err(|e| e.error)?;
    Ok(())
}

fn chunk_account(account: &str, i: usize) -> String {
    if i == 0 {
        account.to_owned()
    } else {
        format!("{account}#{i}")
    }
}

/// Reads the chunked base64 value; `None` when no item exists.
fn os_get(account: &str) -> io::Result<Option<String>> {
    let mut b64 = String::new();
    for i in 0.. {
        let Some(part) = keyring::get(&chunk_account(account, i))? else {
            return if i == 0 {
                Ok(None)
            } else {
                Err(io::Error::other("credential store item is missing a chunk"))
            };
        };
        match part.strip_suffix(MORE) {
            Some(p) => b64.push_str(p),
            None => {
                b64.push_str(&part);
                break;
            }
        }
    }
    let bytes = B64.decode(b64.trim()).map_err(io::Error::other)?;
    String::from_utf8(bytes).map(Some).map_err(io::Error::other)
}

/// Writes tail chunks first so the head never points at an unwritten one.
/// Stale chunks past a shrunk value are never read.
fn os_set(account: &str, json: &str) -> io::Result<()> {
    let b64 = B64.encode(json);
    let parts: Vec<&str> = b64
        .as_bytes()
        .chunks(CHUNK)
        .map(|c| std::str::from_utf8(c).unwrap_or_default())
        .collect();
    for (i, p) in parts.iter().enumerate().rev() {
        let value = if i + 1 < parts.len() {
            format!("{p}{MORE}")
        } else {
            (*p).to_owned()
        };
        keyring::set(&chunk_account(account, i), &value)?;
    }
    Ok(())
}

#[cfg(not(test))]
fn run_with_stdin(mut cmd: Command, input: &str) -> io::Result<()> {
    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()?;
    child
        .stdin
        .take()
        .ok_or_else(|| io::Error::other("no stdin"))?
        .write_all(input.as_bytes())?;
    let out = child.wait_with_output()?;
    let err = String::from_utf8_lossy(&out.stderr);
    if !out.status.success() || !err.trim().is_empty() {
        return Err(io::Error::other(format!("{cmd:?}: {}", err.trim())));
    }
    Ok(())
}

#[cfg(all(not(test), target_os = "macos"))]
mod keyring {
    use super::*;

    const SECURITY: &str = "/usr/bin/security";
    /// `security`'s "item not found" exit status.
    const NOT_FOUND: i32 = 44;

    pub fn get(account: &str) -> io::Result<Option<String>> {
        let out = Command::new(SECURITY)
            .args(["find-generic-password", "-s", SERVICE, "-a", account, "-w"])
            .output()?;
        match out.status.code() {
            Some(0) => Ok(Some(String::from_utf8_lossy(&out.stdout).trim().to_owned())),
            Some(NOT_FOUND) => Ok(None),
            _ => Err(io::Error::other(
                String::from_utf8_lossy(&out.stderr).trim().to_owned(),
            )),
        }
    }

    /// The value (base64 plus `~`) needs no quoting; the account is quoted
    /// for the `-i` tokenizer.
    pub fn set(account: &str, value: &str) -> io::Result<()> {
        if account.contains(['"', '\\', '\n']) {
            return Err(io::Error::other("unsupported data dir path"));
        }
        let mut cmd = Command::new(SECURITY);
        cmd.arg("-i");
        run_with_stdin(
            cmd,
            &format!("add-generic-password -U -s {SERVICE} -a \"{account}\" -w {value}\n"),
        )
    }
}

#[cfg(all(not(test), target_os = "linux"))]
mod keyring {
    use super::*;

    /// A missing `secret-tool` reads as no item; saving then falls back to
    /// the file.
    pub fn get(account: &str) -> io::Result<Option<String>> {
        let out = match Command::new("secret-tool")
            .args(["lookup", "service", SERVICE, "account", account])
            .output()
        {
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            r => r?,
        };
        let err = String::from_utf8_lossy(&out.stderr);
        if !err.trim().is_empty() {
            return Err(io::Error::other(err.trim().to_owned()));
        }
        Ok(out
            .status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned()))
    }

    pub fn set(account: &str, value: &str) -> io::Result<()> {
        let mut cmd = Command::new("secret-tool");
        cmd.args([
            "store", "--label", SERVICE, "service", SERVICE, "account", account,
        ]);
        run_with_stdin(cmd, value)
    }
}

#[cfg(all(not(test), not(any(target_os = "macos", target_os = "linux"))))]
mod keyring {
    use super::*;

    pub fn get(_: &str) -> io::Result<Option<String>> {
        Err(io::ErrorKind::Unsupported.into())
    }

    pub fn set(_: &str, _: &str) -> io::Result<()> {
        Err(io::ErrorKind::Unsupported.into())
    }
}

#[cfg(test)]
mod keyring {
    use std::collections::BTreeMap;
    use std::io;

    use parking_lot::Mutex;

    /// Chunk account → value; an account holding [`FAIL`] errors.
    pub static ITEMS: Mutex<BTreeMap<String, String>> = Mutex::new(BTreeMap::new());
    pub const FAIL: &str = "!fail";

    fn check(account: &str) -> io::Result<()> {
        let base = account.split('#').next().unwrap_or(account);
        match ITEMS.lock().get(base).map(String::as_str) {
            Some(FAIL) => Err(io::Error::other("store unavailable")),
            _ => Ok(()),
        }
    }

    pub fn get(account: &str) -> io::Result<Option<String>> {
        check(account)?;
        Ok(ITEMS.lock().get(account).cloned())
    }

    pub fn set(account: &str, value: &str) -> io::Result<()> {
        check(account)?;
        ITEMS.lock().insert(account.to_owned(), value.to_owned());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vault(dir: &Path) -> (SecretVault, String) {
        let v = SecretVault::new(&dir.join("recur.db"), true);
        let a = v.account.clone().unwrap();
        (v, a)
    }

    fn big() -> Secrets {
        Secrets {
            codex_api_key: "c".repeat(5000),
            linear_api_key: "lin".into(),
            ..Default::default()
        }
    }

    #[test]
    fn os_store_round_trips_chunked_values() {
        let dir = tempfile::tempdir().unwrap();
        let (v, a) = vault(dir.path());
        assert_eq!(v.load().unwrap(), Secrets::default());
        v.save(&big()).unwrap();
        {
            let items = keyring::ITEMS.lock();
            assert!(items[&a].ends_with(MORE) && items[&a].len() == CHUNK + 1);
            assert!(items[&format!("{a}#2")].len() < CHUNK);
        }
        assert!(!v.file.exists());
        assert_eq!(
            SecretVault::new(&dir.path().join("recur.db"), true)
                .load()
                .unwrap(),
            big()
        );

        keyring::ITEMS.lock().remove(&format!("{a}#1"));
        assert!(vault(dir.path()).0.load().is_err());
    }

    #[test]
    fn file_fallback_and_handoff() {
        let dir = tempfile::tempdir().unwrap();
        let (v, a) = vault(dir.path());
        v.load().unwrap();
        keyring::ITEMS
            .lock()
            .insert(a.clone(), keyring::FAIL.into());
        v.save(&big()).unwrap();
        assert!(v.file.exists());

        keyring::ITEMS.lock().remove(&a);
        let (v, _) = vault(dir.path());
        assert_eq!(v.load().unwrap(), big(), "the file wins while present");
        let newer = Secrets {
            codex_api_key: "new".into(),
            ..Default::default()
        };
        v.save(&newer).unwrap();
        assert!(!v.file.exists());
        assert_eq!(vault(dir.path()).0.load().unwrap(), newer);
    }

    #[test]
    fn failed_load_blocks_saves() {
        let dir = tempfile::tempdir().unwrap();
        let (v, a) = vault(dir.path());
        keyring::ITEMS.lock().insert(a, keyring::FAIL.into());
        assert!(v.load().is_err());
        assert!(v.save(&big()).is_err());
        assert!(!v.file.exists());
    }
}
