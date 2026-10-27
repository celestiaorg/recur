#!/usr/bin/env python3
"""Mechanical cross-check of PARITY.md's ticks against the Go/Svelte sources.

Run from anywhere: `python3 rust/tools/parity-sweep/sweep.py` (or `wails3 task check:rust:parity`).
Exits 1 and lists every gap. Stdlib only.

Checks:
  bindings  every function in frontend/bindings/recur/internal/app/<svc>.ts is a method of the
            matching `recur_services::api` trait and of its `impl … for AppServices`
  events    every Wails event name the frontend or Go emits or listens to is an `AppEvent`, and
            the real code (not events.rs, not the mock) builds or matches that variant
  tools     Go's agent tool catalog (dumped with `go run -overlay`, no Go file written) equals
            crates/agent-tools/tests/golden/catalog.json in order, names, schemas, descriptions
            and halts; `tests/catalog.rs` holds the Rust registry to that file byte for byte
  cli       every cobra `Use:` in internal/cli is a command in recur-cli
  svelte    every .svelte file is a ticked PARITY.md row naming a Rust path that exists, in a
            crate the desktop links; every view crate with `register` is registered by the desktop
  gotests   every Go `func TestX` has a Rust test named snake_case(X), a Rust source that names
            `TestX`, or a row in go-tests.tsv (Rust tests that exist, comma-separated, or `n/a: <reason>`)
"""

import json
import re
import subprocess
import sys
import tempfile
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
RUST = ROOT / "rust"
HERE = Path(__file__).resolve().parent
gaps: dict[str, list[str]] = {}
notes: dict[str, str] = {}


def gap(check, msg):
    gaps.setdefault(check, []).append(msg)


def read(p):
    return p.read_text(encoding="utf-8", errors="replace")


def rs_files(*dirs):
    for d in dirs:
        yield from (p for p in (RUST / d).rglob("*.rs") if "/target/" not in str(p))


def is_test_path(p):
    s = str(p)
    return "/tests/" in s or "/spec/" in s or s.endswith("tests.rs") or s.endswith("_tests.rs")


def snake(name):
    for acronym, word in (("PRs", "Prs"), ("GitHub", "Github"), ("IDs", "Ids"), ("URLs", "Urls")):
        name = name.replace(acronym, word)
    s = re.sub(r"([A-Z]+)([A-Z][a-z])", r"\1_\2", name)
    s = re.sub(r"([a-z0-9])([A-Z])", r"\1_\2", s)
    return re.sub(r"_+", "_", s).strip("_").lower()


# ---------- bindings ----------


def trait_blocks(src):
    """`trait Name … { … }` and `impl Name for AppServices { … }` bodies by brace matching."""
    out = {}
    for m in re.finditer(r"pub trait (\w+)[^{]*\{|impl (\w+) for AppServices\s*\{", src):
        depth, i = 1, m.end()
        while depth and i < len(src):
            depth += {"{": 1, "}": -1}.get(src[i], 0)
            i += 1
        key = ("trait", m.group(1)) if m.group(1) else ("impl", m.group(2))
        out[key] = out.get(key, "") + src[m.end() : i]
    return out


def check_bindings():
    blocks = {}
    for p in list(rs_files("crates/services/src/api")) + list(rs_files("crates/services/src/app")):
        if not is_test_path(p):
            for k, v in trait_blocks(read(p)).items():
                blocks[k] = blocks.get(k, "") + v
    traits = {k[1].lower(): k[1] for k in blocks if k[0] == "trait"}
    n = 0
    for ts in sorted((ROOT / "frontend/bindings/recur/internal/app").glob("*service.ts")):
        trait = traits.get(ts.stem)
        if not trait:
            gap("bindings", f"{ts.name}: no recur_services::api trait named like `{ts.stem}`")
            continue
        for fn in re.findall(r"^export function (\w+)\(", read(ts), re.M):
            n += 1
            want = snake(fn)
            pat = rf"\bfn {want}\b"
            if not re.search(pat, blocks[("trait", trait)]):
                gap("bindings", f"{ts.stem}.{fn}: no `{trait}::{want}`")
            elif not re.search(pat, blocks.get(("impl", trait), "")):
                gap("bindings", f"{ts.stem}.{fn}: `{trait}::{want}` not implemented by AppServices")
    notes["bindings"] = f"{n} bound functions"


# ---------- events ----------


# UI-to-backend events whose job the Rust shell does in process, with no event between them.
EVENTS_IN_PROCESS = {
    "menu.state": "the shell rebuilds the native menu from AppState::menu_state (recur_app::shell::sync_menus)",
    "menu.themeState": "the shell rebuilds the Appearance radios from AppState (recur_app::shell::sync_menus)",
    "menu.themes": "the shell rebuilds the Appearance submenus from AppState's catalogs (recur_app::shell::sync_menus)",
    "window.titlebarDblclick": "the titlebar applies AppleActionOnDoubleClick itself (recur_app::chrome::apply_titlebar_double_click)",
}


def check_events():
    names = set()
    for p in (ROOT / "frontend/src").rglob("*"):
        if p.suffix in (".ts", ".svelte") and ".test." not in p.name:
            names |= set(re.findall(r"Events\.(?:On|Emit|Once|OnMultiple)\(\s*[`\"']([\w.:-]+)", read(p)))
    for p in list((ROOT / "internal").rglob("*.go")) + list(ROOT.glob("*.go")):
        if not p.name.endswith("_test.go"):
            names |= set(re.findall(r"\.(?:Emit|EmitEvent|emit)\(\s*\"([\w.:-]+)\"", read(p)))
            names |= set(re.findall(r"Event\.On\(\s*\"([\w.:-]+)\"", read(p)))
    events = read(RUST / "crates/services/src/events.rs")
    by_name = dict((n, v) for v, n in re.findall(r"Self::(\w+)(?:\(_\))? => \"([\w.:-]+)\"", events))
    real = "\n".join(
        read(p)
        for p in rs_files("crates", "providers")
        if not is_test_path(p)
        and not p.match("crates/services/src/events.rs")
        and "/services/src/mock/" not in str(p)
    )
    engine = "\n".join(read(p) for p in rs_files("crates/engine/src", "crates/services/src/app") if not is_test_path(p))
    for name in sorted(names):
        if name.startswith("term."):
            variant = "Term"
        elif name.startswith("wails:updater:") and name != "wails:updater:user:restart":
            variant = "Updater"
        else:
            variant = by_name.get(name)
        if not variant:
            gap("events", f"{name}: no AppEvent variant")
        elif name in EVENTS_IN_PROCESS:
            continue
        elif not re.search(rf"AppEvent::{variant}\b", real) and f'"{name}"' not in engine:
            gap("events", f"{name}: AppEvent::{variant} is never built or matched outside events.rs and the mock")
    notes["events"] = f"{len(names)} event names ({len(EVENTS_IN_PROCESS)} handled in process)"


# ---------- tools ----------


GO_DUMP = """package main

import (
	"encoding/json"
	"os"

	"recur/internal/agenttools"
)

type entry struct {
	Name        string `json:"name"`
	Description string `json:"description"`
	Schema      string `json:"schema"`
	Halts       bool   `json:"halts"`
}

func dump(ts []agenttools.Tool) []entry {
	out := []entry{}
	for _, t := range ts {
		out = append(out, entry{t.Name(), t.Description(), string(t.Schema()), agenttools.Halts(t)})
	}
	return out
}

func main() {
	json.NewEncoder(os.Stdout).Encode(map[string][]entry{
		"host":   dump(agenttools.HostTools()),
		"coding": dump(agenttools.CodingTools()),
	})
}
"""


def go_tools():
    """Go's catalog, from a throwaway main package laid over the module (`go run -overlay`)."""
    with tempfile.TemporaryDirectory() as tmp:
        main = Path(tmp) / "main.go"
        main.write_text(GO_DUMP.replace("recur/internal", f"{go_module()}/internal"))
        overlay = Path(tmp) / "overlay.json"
        virtual = ROOT / "internal/agenttools/paritysweep/main.go"
        overlay.write_text(json.dumps({"Replace": {str(virtual): str(main)}}))
        out = subprocess.run(
            ["go", "run", f"-overlay={overlay}", "./internal/agenttools/paritysweep"],
            cwd=ROOT, capture_output=True, text=True,
        )
        if out.returncode:
            raise SystemExit(f"tools: go run failed:\n{out.stderr}")
        return json.loads(out.stdout)


def go_module():
    return re.search(r"^module (\S+)", read(ROOT / "go.mod"), re.M).group(1)


def check_tools():
    golden = json.loads(read(RUST / "crates/agent-tools/tests/golden/catalog.json"))
    go = go_tools()
    for kind in ("host", "coding"):
        want = {e["name"]: e for e in go[kind]}
        got = {e["name"]: e for e in golden[kind]}
        if [e["name"] for e in go[kind]] != [e["name"] for e in golden[kind]]:
            gap("tools", f"{kind}: tool order differs from Go's")
        for name, e in want.items():
            r = got.get(name)
            if not r:
                gap("tools", f"{name}: not in the Rust registry")
                continue
            for field in ("schema", "description", "halts"):
                if e[field] != r[field]:
                    gap("tools", f"{name}: {field} differs from Go")
        for name in sorted(set(got) - set(want)):
            gap("tools", f"{name}: in the Rust registry but not in internal/agenttools")
    notes["tools"] = f"{len(go['host'])} host + {len(go['coding'])} coding tools"


# ---------- cli ----------


def check_cli():
    rust = "\n".join(read(p) for p in rs_files("crates/cli/src"))
    uses = []
    for p in (ROOT / "internal/cli").glob("*.go"):
        if not p.name.endswith("_test.go"):
            uses += re.findall(r"Use:\s*\"([^\"]+)\"", read(p))
    for u in uses:
        if f'"{u}"' not in rust:
            gap("cli", f"`{u}`: no recur-cli command with that use line")
    notes["cli"] = f"{len(uses)} commands"


# ---------- svelte ----------


def crates():
    out = {}
    for toml in RUST.glob("**/Cargo.toml"):
        if "/target/" in str(toml) or "/vendor/" in str(toml) or toml.parent == RUST:
            continue
        data = tomllib.loads(read(toml))
        if "package" in data:
            deps = set(data.get("dependencies", {})) | set(
                d for t in data.get("target", {}).values() for d in t.get("dependencies", {})
            )
            out[data["package"]["name"].replace("-", "_")] = (toml.parent, {d.replace("-", "_") for d in deps})
    return out


def closure(all_crates, start):
    seen, todo = set(), [start]
    while todo:
        c = todo.pop()
        if c in seen or c not in all_crates:
            continue
        seen.add(c)
        todo += all_crates[c][1]
    return seen


def item_exists(crate_dir, path):
    src = crate_dir / "src"
    mods, item = path[:-1], path[-1]
    d = src
    for m in mods:
        if (d / f"{m}.rs").exists() or (d / m / "mod.rs").exists():
            d = d / m
        else:
            return False
    files = [p for p in [d.with_suffix(".rs"), d / "mod.rs"] if p.exists()] or list(src.glob("*.rs"))
    files += list(d.rglob("*.rs")) if d.is_dir() else []
    pat = re.compile(rf"\b(?:struct|enum|fn|mod|trait|type|const|static)\s+{re.escape(item)}\b")
    return (d / f"{item}.rs").exists() or (d / item).is_dir() or any(pat.search(read(f)) for f in files)


def check_svelte():
    parity = read(RUST / "docs/PARITY.md").splitlines()
    all_crates = crates()
    linked = closure(all_crates, "recur_desktop")
    svelte = sorted((ROOT / "frontend/src").rglob("*.svelte"))
    for f in svelte:
        rows = [l for l in parity if re.search(rf"`(?:[\w/]*/)?{re.escape(f.name)}`", l)]
        ticked = [l for l in rows if l.lstrip().startswith("- [x]")]
        if not ticked:
            gap("svelte", f"{f.relative_to(ROOT)}: no ticked PARITY.md row")
            continue
        paths = [
            p.replace("-", "_").split("::") for l in ticked for p in re.findall(r"`(recur[-_][\w-]+(?:::\w+)*)", l)
        ]
        if not paths:
            gap("svelte", f"{f.name}: its row names no `recur_…` path")
            continue
        for p in paths:
            crate = all_crates.get(p[0])
            if not crate:
                gap("svelte", f"{f.name}: `{'::'.join(p)}`: no crate {p[0]}")
            elif p[0] not in linked:
                gap("svelte", f"{f.name}: {p[0]} is not linked into recur-desktop")
            elif len(p) > 1 and not item_exists(crate[0], p[1:]):
                gap("svelte", f"{f.name}: `{'::'.join(p)}` does not exist")
    desktop = read(RUST / "crates/desktop/src/main.rs")
    for name, (d, _) in all_crates.items():
        if re.search(r"crates/(views|panes)/", str(d)) and re.search(r"pub fn register\(", read(d / "src/lib.rs")):
            if f"{name}::register(" not in desktop:
                gap("svelte", f"{name}::register is not called by the desktop")
    notes["svelte"] = f"{len(svelte)} components and views"


# ---------- gotests ----------


def rust_tests():
    names, mentions, located = set(), set(), set()
    attr = re.compile(r"#\[(?:[\w:]*test|rstest)\b[^\]]*\]\s*(?:#\[[^\]]*\]\s*)*(?:pub\s+)?(?:async\s+)?fn\s+(\w+)")
    for p in rs_files("crates", "providers", "tools"):
        src = read(p)
        found = set(attr.findall(src))
        names |= found
        located |= {f"{p.relative_to(RUST)}::{n}" for n in found}
        mentions |= set(re.findall(r"\b(Test[A-Z]\w*)", src))
    return names, mentions, located


def check_gotests():
    names, mentions, located = rust_tests()
    table = {}
    for line in read(HERE / "go-tests.tsv").splitlines():
        if line and not line.startswith("#"):
            key, _, target = line.partition("\t")
            table[key] = target.strip()
    seen, total, mapped, na = set(), 0, 0, 0
    files = [p for p in list((ROOT / "internal").rglob("*_test.go")) + list((ROOT / "cmd").rglob("*_test.go")) + list(ROOT.glob("*_test.go"))]
    for p in sorted(files):
        pkg = str(p.parent.relative_to(ROOT))
        for t in re.findall(r"^func (Test\w+)\(\s*\w+ \*testing\.T\)", read(p), re.M):
            key = f"{pkg}.{t}"
            seen.add(key)
            total += 1
            if key in table:
                target = table[key]
                if target.startswith("n/a:") and len(target) > 6:
                    na += 1
                elif all(t.strip() in located for t in target.split(",")):
                    mapped += 1
                else:
                    missing = [t.strip() for t in target.split(",") if t.strip() not in located]
                    gap("gotests", f"{key}: go-tests.tsv names `{', '.join(missing)}`, not a Rust test")
            elif snake(t[4:]) in names or t in mentions:
                mapped += 1
            else:
                gap("gotests", f"{key}: no Rust test `{snake(t[4:])}` and no go-tests.tsv row")
    for key in sorted(set(table) - seen):
        gap("gotests", f"{key}: stale go-tests.tsv row (no such Go test)")
    notes["gotests"] = f"{total} Go tests: {mapped} ported, {na} not applicable"


CHECKS = {
    "bindings": check_bindings,
    "events": check_events,
    "tools": check_tools,
    "cli": check_cli,
    "svelte": check_svelte,
    "gotests": check_gotests,
}

if __name__ == "__main__":
    only = sys.argv[1:] or list(CHECKS)
    for name in only:
        CHECKS[name]()
    for name in only:
        g = gaps.get(name, [])
        print(f"{name}: {notes.get(name, '')}: {'ok' if not g else f'{len(g)} gaps'}")
        for m in g:
            print(f"  {m}")
    sys.exit(1 if any(gaps.get(n) for n in only) else 0)
