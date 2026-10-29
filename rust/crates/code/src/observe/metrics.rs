use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::ops::ControlFlow;

use recur_domain::path;
use recur_domain::{DupClone, DupLoc, Hotspot, MetricSnapshot};

use crate::gostr::truncate_fields;
use crate::langs::is_source_file;
use crate::scan::scan_lines;
use crate::walk::{Flow, walk_dir};

/// Consecutive significant lines that form a clone fingerprint.
const DUP_WINDOW: usize = 5;
/// Normalized lines shorter than this are ignored.
const DUP_MIN_LINE_LEN: usize = 12;
const DUP_MAX_CLONES: usize = 24;
const DUP_MAX_OCCS: usize = 8;

/// One file's duplication fingerprint: the 1-based numbers of its
/// significant lines and a hash per line. No preview text: that would be a
/// second copy of the codebase in memory for a report that shows at most
/// 24 × 8 snippets, read back from disk instead.
struct FileSeq {
    path: String,
    line_nos: Vec<i64>,
    hashes: Vec<u64>,
}

#[derive(Clone)]
struct DupSite {
    path: String,
    /// Index into the file's sequence.
    start: usize,
}

/// Size, complexity and duplication proxies for the source under `root`.
pub fn collect_metrics(root: &str) -> MetricSnapshot {
    let mut m = MetricSnapshot::default();
    let mut seqs: Vec<FileSeq> = Vec::new();
    let _ = walk_dir::<()>(root, |path, d, err| {
        let (Some(d), None) = (d, err) else {
            return Ok(Flow::Continue);
        };
        if d.is_dir() {
            return Ok(if skip_dir(&d.name) {
                Flow::SkipDir
            } else {
                Flow::Continue
            });
        }
        if !is_source_file(&d.name) {
            return Ok(Flow::Continue);
        }
        let rel = path::rel(root, path).unwrap_or_default();
        let Ok(f) = File::open(path) else {
            return Ok(Flow::Continue);
        };
        m.files += 1;
        if is_test_file(&d.name) {
            m.test_files += 1;
        }
        let (mut loc, mut complexity, mut todos) = (0i64, 1i64, 0i64);
        let mut seq = FileSeq {
            path: rel,
            line_nos: Vec::new(),
            hashes: Vec::new(),
        };
        let _ = scan_lines::<()>(f, |line_no, raw| {
            let line = String::from_utf8_lossy(raw);
            let trimmed = line.trim();
            if trimmed.is_empty()
                || trimmed.starts_with("//")
                || trimmed.starts_with('#')
                || trimmed.starts_with('*')
            {
                return ControlFlow::Continue(());
            }
            loc += 1;
            complexity += branch_weight(trimmed);
            if ["TODO", "FIXME", "HACK"]
                .iter()
                .any(|t| trimmed.contains(t))
            {
                todos += 1;
            }
            let norm = normalize_line(raw);
            if norm.len() >= DUP_MIN_LINE_LEN && !is_noise_line(&norm) {
                seq.line_nos.push(line_no as i64);
                seq.hashes.push(fnv64a(norm.as_bytes()));
            }
            ControlFlow::Continue(())
        });
        m.loc += loc;
        m.todo_count += todos;
        m.max_complexity = m.max_complexity.max(complexity);
        let reason = if complexity >= 25 {
            "high complexity"
        } else if loc >= 400 {
            "large file"
        } else {
            ""
        };
        if !reason.is_empty() {
            m.hotspots.push(Hotspot {
                path: seq.path.clone(),
                loc,
                complexity,
                reason: reason.into(),
            });
        }
        if seq.hashes.len() >= DUP_WINDOW {
            seqs.push(seq);
        }
        Ok(Flow::Continue)
    });

    if m.files > 0 {
        m.avg_complexity = if m.hotspots.is_empty() {
            m.max_complexity as f64 * 0.35
        } else {
            m.hotspots.iter().map(|h| h.complexity).sum::<i64>() as f64 / m.hotspots.len() as f64
        };
    }
    let (clones, covered) = find_clones(root, &seqs);
    m.duplicates = clones;
    if m.loc > 0 {
        m.duplication = (covered as f64 / m.loc as f64).min(1.0);
    }
    m.hotspots.truncate(8);
    m
}

fn find_clones(root: &str, seqs: &[FileSeq]) -> (Vec<DupClone>, usize) {
    let mut groups: HashMap<u64, Vec<DupSite>> = HashMap::new();
    for seq in seqs {
        for i in 0..=seq.hashes.len() - DUP_WINDOW {
            groups
                .entry(window_hash(&seq.hashes[i..i + DUP_WINDOW]))
                .or_default()
                .push(DupSite {
                    path: seq.path.clone(),
                    start: i,
                });
        }
    }
    let mut raw: Vec<(Vec<DupSite>, usize)> = groups
        .into_values()
        .filter_map(|sites| {
            let uniq = dedupe_sites(sites);
            (uniq.len() >= 2 && has_distinct_clone_sites(&uniq)).then(|| {
                let score = uniq.len() * DUP_WINDOW;
                (uniq, score)
            })
        })
        .collect();
    let by_path: HashMap<&str, &FileSeq> = seqs.iter().map(|s| (s.path.as_str(), s)).collect();
    // Previews are not in memory, so ties break on the first site's location.
    raw.sort_by(|(a, sa), (b, sb)| {
        sb.cmp(sa)
            .then_with(|| a[0].path.cmp(&b[0].path))
            .then_with(|| a[0].start.cmp(&b[0].start))
    });

    let mut covered: HashSet<String> = HashSet::new();
    let mut clones = Vec::new();
    let mut seen_occ = HashSet::new();
    let mut previews = PreviewReader::new(root);
    for (sites, _) in &raw {
        if clones.len() >= DUP_MAX_CLONES {
            break;
        }
        let block_len = expand_clone(&by_path, sites);
        let mut occs = Vec::new();
        let mut sig_parts = Vec::new();
        for s in sites {
            let seq = by_path[s.path.as_str()];
            let start_line = seq.line_nos[s.start];
            let end_idx = (s.start + block_len - 1).min(seq.line_nos.len() - 1);
            occs.push(DupLoc {
                path: s.path.clone(),
                line: start_line,
                end: seq.line_nos[end_idx],
                text: previews.at(&s.path, start_line),
            });
            sig_parts.push(format!("{}:{start_line}", s.path));
            for i in s.start..=end_idx {
                covered.insert(format!("{}:{}", s.path, seq.line_nos[i]));
            }
            if occs.len() >= DUP_MAX_OCCS {
                break;
            }
        }
        sig_parts.sort();
        if !seen_occ.insert(format!("{}#{block_len}", sig_parts.join("|"))) {
            continue;
        }
        let first = &sites[0];
        let first_line = by_path[first.path.as_str()].line_nos[first.start];
        clones.push(DupClone {
            lines: block_len as i64,
            preview: previews.at(&first.path, first_line),
            occurrences: occs,
        });
    }
    (clones, covered.len())
}

/// Reads the source of reported lines on demand (memoized: a hot clone
/// reports a site twice, as occurrence text and clone preview).
struct PreviewReader<'a> {
    root: &'a str,
    cache: HashMap<String, String>,
}

impl<'a> PreviewReader<'a> {
    fn new(root: &'a str) -> Self {
        Self {
            root,
            cache: HashMap::new(),
        }
    }

    fn at(&mut self, rel: &str, line: i64) -> String {
        let key = format!("{rel}:{line}");
        if let Some(v) = self.cache.get(&key) {
            return v.clone();
        }
        let mut out = String::new();
        if let Ok(f) = File::open(path::join(&[self.root, rel])) {
            let _ = scan_lines(f, |n, raw| {
                if n as i64 == line {
                    out = truncate_fields(String::from_utf8_lossy(raw).trim(), 80);
                    return ControlFlow::Break(());
                }
                ControlFlow::Continue(())
            });
        }
        self.cache.insert(key, out.clone());
        out
    }
}

fn expand_clone(by_path: &HashMap<&str, &FileSeq>, sites: &[DupSite]) -> usize {
    let mut n = DUP_WINDOW;
    while n < 80 {
        let mut h = 0;
        let mut ok = true;
        for (i, st) in sites.iter().enumerate() {
            let seq = by_path[st.path.as_str()];
            let Some(&x) = seq.hashes.get(st.start + n) else {
                ok = false;
                break;
            };
            if i == 0 {
                h = x;
            } else if x != h {
                ok = false;
                break;
            }
        }
        if !ok {
            break;
        }
        n += 1;
    }
    n
}

fn dedupe_sites(sites: Vec<DupSite>) -> Vec<DupSite> {
    let mut seen = HashSet::new();
    sites
        .into_iter()
        .filter(|s| seen.insert((s.path.clone(), s.start)))
        .collect()
}

fn has_distinct_clone_sites(sites: &[DupSite]) -> bool {
    let paths: HashSet<&str> = sites.iter().map(|s| s.path.as_str()).collect();
    if paths.len() >= 2 {
        return true;
    }
    if sites.len() < 2 {
        return false;
    }
    // One file: the windows must not overlap.
    let mut starts: Vec<usize> = sites.iter().map(|s| s.start).collect();
    starts.sort_unstable();
    starts.windows(2).any(|w| w[1] - w[0] >= DUP_WINDOW)
}

/// FNV-1a 64 over each hash's little-endian bytes.
fn window_hash(hs: &[u64]) -> u64 {
    let bytes: Vec<u8> = hs.iter().flat_map(|x| x.to_le_bytes()).collect();
    fnv64a(&bytes)
}

fn fnv64a(b: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &c in b {
        h ^= u64::from(c);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

fn is_noise_line(norm: &str) -> bool {
    if matches!(
        norm,
        "returnnil"
            | "returnnull"
            | "return;"
            | "pass"
            | "break;"
            | "continue;"
            | "}else{"
            | "elsenull"
            | "end"
            | "fi"
            | "done"
    ) {
        return true;
    }
    // Pure punctuation and braces.
    !norm
        .chars()
        .any(|c| crate::gostr::is_letter(c) || crate::gostr::is_digit(c))
}

fn branch_weight(line: &str) -> i64 {
    [
        "if ", "else if", "elif ", "for ", "while ", "case ", "catch ", "except ", "match ",
        "when ", "&&", "||", "?", "switch ",
    ]
    .iter()
    .filter(|kw| line.contains(*kw))
    .count() as i64
}

/// Lowercased, with every space removed.
fn normalize_line(raw: &[u8]) -> String {
    String::from_utf8_lossy(raw)
        .chars()
        .filter(|c| !c.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect()
}

fn is_test_file(name: &str) -> bool {
    let n = name.to_lowercase();
    [
        "_test.go",
        "_test.py",
        "_spec.rb",
        ".test.ts",
        ".test.tsx",
        ".test.js",
        ".test.jsx",
        ".spec.ts",
        ".spec.tsx",
        ".spec.js",
        ".spec.jsx",
        "test.java",
        "tests.java",
        "test.kt",
        "test.swift",
        "_test.rs",
    ]
    .iter()
    .any(|s| n.ends_with(s))
        || n.starts_with("test_")
        || n.contains("_test.")
        || n.contains(".test.")
        || n.contains(".spec.")
}

/// [`crate::root`]'s walk skip list plus generated `bindings`.
fn skip_dir(name: &str) -> bool {
    name == "bindings" || crate::root::skip_dir(name)
}
