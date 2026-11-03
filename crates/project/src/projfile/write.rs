use std::io::Write as _;
use std::path::Path;

use recur_domain::{Project, effective_runs};

use super::{FILE_HEADER, File, FileRun, REL_PATH, invalidate, repo_root};
use toml_edit::{ArrayOfTables, DocumentMut, Item, Table, value};

/// Why [`write`] could not save project.toml.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct WriteError(String);

fn write_shape(p: &Project) -> File {
    File {
        setup_cmd: p.setup_cmd.clone(),
        teardown_cmd: p.teardown_cmd.clone(),
        app_url: p.app_url.clone(),
        env: p.env.clone(),
        copy_files: p.copy_files.clone(),
        runs: effective_runs(p)
            .into_iter()
            .map(|r| FileRun {
                name: r.name,
                cmd: r.cmd,
            })
            .collect(),
    }
}

/// Creates `.recur/project.toml` from p's run-config. When a parseable file
/// already exists, values are patched in place so comments, unknown keys and
/// layout stay. A missing or malformed file is rewritten from scratch with
/// the standard header.
pub fn write(p: &Project) -> Result<(), WriteError> {
    let root = repo_root(&p.repo_path);
    if root.is_empty() {
        return Err(WriteError(format!("write {REL_PATH}: empty repo path")));
    }
    match std::fs::metadata(&root) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(WriteError(format!(
                "write {REL_PATH}: repo path {root:?} does not exist"
            )));
        }
        Err(e) => {
            return Err(WriteError(format!("write {REL_PATH}: stat repo path: {e}")));
        }
        Ok(st) if !st.is_dir() => {
            return Err(WriteError(format!(
                "write {REL_PATH}: repo path {root:?} is not a directory"
            )));
        }
        Ok(_) => {}
    }
    let dir = crate::home::join(&root, ".recur");
    std::fs::create_dir_all(&dir).map_err(|e| WriteError(format!("create {dir}: {e}")))?;
    let path = crate::home::join(&dir, "project.toml");
    let data = encode_project_file(&path, p)?;
    write_atomic(&path, &data)
}

pub(crate) fn encode_project_file(path: &str, p: &Project) -> Result<String, WriteError> {
    let existing = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(marshal_new(p)),
        Err(e) => return Err(WriteError(format!("read {REL_PATH}: {e}"))),
    };
    match merge_project_file(&existing, p) {
        Ok(merged) => Ok(merged),
        Err(err) => {
            tracing::warn!("recur: {REL_PATH}: rewrite after parse/merge error: {err}");
            Ok(marshal_new(p))
        }
    }
}

const KEYS: [&str; 5] = ["setup_cmd", "teardown_cmd", "app_url", "env", "copy_files"];

fn fields(f: &File) -> [&str; 5] {
    [
        &f.setup_cmd,
        &f.teardown_cmd,
        &f.app_url,
        &f.env,
        &f.copy_files,
    ]
}

fn run_table(r: &FileRun) -> Table {
    let mut t = Table::new();
    t["name"] = value(r.name.as_str());
    t["cmd"] = value(r.cmd.as_str());
    t
}

fn marshal_new(p: &Project) -> String {
    let f = write_shape(p);
    let mut d = DocumentMut::new();
    for (k, v) in KEYS.into_iter().zip(fields(&f)) {
        d[k] = value(v);
    }
    if !f.runs.is_empty() {
        let mut runs = ArrayOfTables::new();
        for r in &f.runs {
            runs.push(run_table(r));
        }
        d["run"] = Item::ArrayOfTables(runs);
    }
    format!("{FILE_HEADER}{d}")
}

fn merge_project_file(existing: &[u8], p: &Project) -> Result<String, String> {
    let text = std::str::from_utf8(existing).map_err(|_| "toml: invalid UTF-8".to_owned())?;
    let mut d: DocumentMut = text
        .parse()
        .map_err(|e: toml_edit::TomlError| e.to_string())?;
    let f = write_shape(p);
    for (k, v) in KEYS.into_iter().zip(fields(&f)) {
        set_string(&mut d, k, v)?;
    }
    merge_runs(&mut d, &f.runs)?;
    d.remove("dev_cmd");
    Ok(d.to_string())
}

/// Sets `key` to `v`, keeping its comments, unless it already holds `v` or is absent and
/// `v` is empty. A table under `key` is an error, so the caller rewrites the file.
fn set_string(t: &mut Table, key: &str, v: &str) -> Result<(), String> {
    match t.get_mut(key) {
        Some(Item::Value(cur)) if cur.as_str() == Some(v) => {}
        Some(Item::Value(cur)) => {
            let decor = cur.decor().clone();
            *cur = v.into();
            *cur.decor_mut() = decor;
        }
        Some(Item::None) | None if v.is_empty() => {}
        Some(Item::None) | None => t[key] = value(v),
        Some(_) => return Err(format!("{key}: not a string")),
    }
    Ok(())
}

/// Patches the `[[run]]` tables in place (their other keys and comments stay), appends new
/// ones and drops the extra. A `run` that is not an array of tables (`[run.0]`) is replaced.
fn merge_runs(d: &mut DocumentMut, runs: &[FileRun]) -> Result<(), String> {
    if runs.is_empty() || !d.get("run").is_some_and(Item::is_array_of_tables) {
        d.remove("run");
    }
    if runs.is_empty() {
        return Ok(());
    }
    let Some(tables) = d
        .entry("run")
        .or_insert(Item::ArrayOfTables(ArrayOfTables::new()))
        .as_array_of_tables_mut()
    else {
        return Ok(());
    };
    for (i, r) in runs.iter().enumerate() {
        match tables.get_mut(i) {
            Some(t) => {
                set_string(t, "name", &r.name)?;
                set_string(t, "cmd", &r.cmd)?;
            }
            None => tables.push(run_table(r)),
        }
    }
    while tables.len() > runs.len() {
        tables.remove(tables.len() - 1);
    }
    Ok(())
}

fn write_atomic(path: &str, data: &str) -> Result<(), WriteError> {
    invalidate(path);
    let err = |e: &dyn std::fmt::Display| WriteError(format!("write {REL_PATH}: {e}"));
    let dir = Path::new(path).parent().unwrap_or(Path::new("."));
    let mut tmp = tempfile::Builder::new()
        .prefix(".project.toml.")
        .tempfile_in(dir)
        .map_err(|e| err(&e))?;
    tmp.write_all(data.as_bytes()).map_err(|e| err(&e))?;
    tmp.persist(path).map_err(|e| err(&e.error))?;
    Ok(())
}
