use std::io::Write as _;
use std::path::Path;

use recur_domain::{Project, effective_runs};

use super::{FILE_HEADER, File, FileRun, REL_PATH, invalidate, repo_root};
use crate::tomledit::{Document, Value, array_tables, encode_string, key_value, multiline_field};

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

fn run_body(r: &FileRun) -> String {
    key_value("name", &encode_string(&r.name)) + &key_value("cmd", &multiline_field(&r.cmd))
}

fn marshal_new(p: &Project) -> String {
    let f = write_shape(p);
    let mut out = String::from(FILE_HEADER);
    out += &key_value("setup_cmd", &multiline_field(&f.setup_cmd));
    out += &key_value("teardown_cmd", &multiline_field(&f.teardown_cmd));
    out += &key_value("app_url", &encode_string(&f.app_url));
    out += &key_value("env", &multiline_field(&f.env));
    out += &key_value("copy_files", &multiline_field(&f.copy_files));
    let bodies: Vec<String> = f.runs.iter().map(run_body).collect();
    out += &array_tables("run", &bodies, true);
    out
}

fn merge_project_file(existing: &[u8], p: &Project) -> Result<String, String> {
    let text = std::str::from_utf8(existing).map_err(|_| "toml: invalid UTF-8".to_owned())?;
    let mut d = Document::parse(text).map_err(|e| e.to_string())?;
    let shape = write_shape(p);
    set_string(&mut d, "setup_cmd", &shape.setup_cmd)?;
    set_string(&mut d, "teardown_cmd", &shape.teardown_cmd)?;
    set_string(&mut d, "app_url", &shape.app_url)?;
    set_string(&mut d, "env", &shape.env)?;
    set_string(&mut d, "copy_files", &shape.copy_files)?;
    merge_runs(&mut d, &shape.runs)?;
    d.delete(&["dev_cmd"]);
    Ok(d.to_string())
}

fn append_section(doc: &str, extra: &str) -> String {
    if extra.is_empty() {
        return doc.to_owned();
    }
    format!("{}\n\n{extra}", doc.trim_end_matches('\n'))
}

fn set_string(d: &mut Document, key: &str, value: &str) -> Result<(), String> {
    match d.get(&[key]) {
        Some(toml::Value::String(cur)) if cur == value => return Ok(()),
        None if value.is_empty() => return Ok(()),
        _ => {}
    }
    d.set(&[key], &Value::Raw(multiline_field(value)))
        .map_err(|e| e.to_string())
}

fn merge_runs(d: &mut Document, runs: &[FileRun]) -> Result<(), String> {
    let n = existing_run_count(d);
    if runs.is_empty() {
        if n > 0 {
            d.delete(&["run"]);
        }
        return Ok(());
    }
    if n == 0 {
        // Setting run.0.… would create [run.0] tables, which overlay cannot
        // read as [[run]]. Append real array-of-tables blocks instead.
        let bodies: Vec<String> = runs.iter().map(run_body).collect();
        let blocks = array_tables("run", &bodies, false);
        *d =
            Document::parse(&append_section(&d.to_string(), &blocks)).map_err(|e| e.to_string())?;
        return Ok(());
    }
    for (i, r) in runs.iter().enumerate() {
        let idx = i.to_string();
        set_run_field(d, &idx, "name", &r.name)?;
        set_run_field(d, &idx, "cmd", &r.cmd)?;
    }
    for i in (runs.len()..n).rev() {
        d.delete(&["run", &i.to_string()]);
    }
    Ok(())
}

fn existing_run_count(d: &mut Document) -> usize {
    match d.get(&["run"]) {
        None => 0,
        Some(toml::Value::Array(arr)) => arr.len(),
        Some(_) => {
            d.delete(&["run"]);
            0
        }
    }
}

fn set_run_field(d: &mut Document, idx: &str, field: &str, value: &str) -> Result<(), String> {
    let path = ["run", idx, field];
    if let Some(toml::Value::String(cur)) = d.get(&path)
        && cur == value
    {
        return Ok(());
    }
    let v = if field == "cmd" {
        Value::Raw(multiline_field(value))
    } else {
        Value::Str(value.to_owned())
    };
    d.set(&path, &v).map_err(|e| e.to_string())
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
