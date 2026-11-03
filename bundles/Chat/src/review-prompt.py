#!/usr/bin/env python3
# Shared Review / Re-review / simplify-review / simplify-audit prompt body.
# Sourced via RECUR_BUNDLE_SUPPORT.
# RECUR_REVIEW_PASS=initial|again
# RECUR_REVIEW_KIND=bugs|simplify|audit (default bugs)
import json, os, secrets

surface = (os.environ.get("RECUR_SURFACE") or "workspace").strip()
pass_kind = (os.environ.get("RECUR_REVIEW_PASS") or "initial").strip()
kind = (os.environ.get("RECUR_REVIEW_KIND") or "bugs").strip()  # bugs | simplify | audit
host = os.environ.get("RECUR_PR_HOST") == "1"
base = os.environ.get("RECUR_GIT_BASE", "")
merge_base = os.environ.get("RECUR_GIT_MERGE_BASE", "")
branch = os.environ.get("RECUR_GIT_BRANCH", "")
status = os.environ.get("_RECUR_STATUS", "")
stat = os.environ.get("_RECUR_STAT", "")
stat_work = os.environ.get("_RECUR_STAT_WORK", "")
diff_commits = os.environ.get("_RECUR_DIFF_COMMITS", "")
diff_work = os.environ.get("_RECUR_DIFF_WORK", "")
untracked = os.environ.get("_RECUR_UNTRACKED", "")
pr_view = os.environ.get("_RECUR_PR_VIEW", "")
pr_diff = os.environ.get("_RECUR_PR_DIFF", "")
pr_stat = os.environ.get("_RECUR_PR_STAT", "")
pr_num = (os.environ.get("RECUR_PR_NUMBER") or "").strip()
pr_url = (os.environ.get("RECUR_PR_URL") or "").strip()
pr_title = (os.environ.get("RECUR_PR_TITLE") or "").strip()
pr_base = (os.environ.get("RECUR_PR_BASE") or "").strip()
pr_branch = (os.environ.get("RECUR_PR_BRANCH") or "").strip()
# Set when this /re-review continues the same chat's earlier review: the embed
# is only the delta since that pass (git-context.sh), not the full branch diff.
since_review = bool((os.environ.get("RECUR_REVIEW_SINCE") or "").strip())

nonce = secrets.token_hex(4)
open_m, close_m = f"⟦raw:{nonce}⟧", f"⟦/raw:{nonce}⟧"


def scrub(s: str) -> str:
    return s.replace(open_m, "⟦raw\u200b:" + nonce + "⟧").replace(close_m, "⟦/raw\u200b:" + nonce + "⟧")


def pr_label() -> str:
    n = pr_num if pr_num and pr_num != "0" else ""
    if n and pr_url:
        return f"GitHub pull request #{n} ({pr_url})"
    if n:
        return f"GitHub pull request #{n}"
    if pr_url:
        return f"GitHub pull request ({pr_url})"
    return "this GitHub pull request"


again = pass_kind == "again"

if since_review:
    diff_sections = [
        "## git diff --stat (since your last review pass)",
        scrub(stat_work.strip() or "(empty)"),
        "",
        "## git diff (since your last review pass: committed or not, incl. new files)",
        "```diff",
        scrub(diff_work if diff_work.strip() else "(nothing changed since the last pass)"),
        "```",
    ]
else:
    diff_sections = [
        f"## git diff --stat {merge_base}...HEAD (committed vs base)",
        scrub(stat.strip() or "(empty)"),
        "",
        "## git diff --stat (uncommitted)",
        scrub(stat_work.strip() or "(empty)"),
        "",
        f"## git diff {merge_base}...HEAD",
        "```diff",
        scrub(diff_commits if diff_commits.strip() else "(empty)"),
        "```",
        "",
        "## git diff (uncommitted)",
        "```diff",
        scrub(diff_work if diff_work.strip() else "(empty)"),
        "```",
    ]

# Over-engineering pass: Ponytail review/audit body (MIT), filed via Recur findings.
# https://github.com/DietrichGebert/ponytail: skills/ponytail-review, ponytail-audit.
SIMPLIFY_FOCUS = """
Review diffs for unnecessary complexity. What to cut, what replaces it. The diff's best outcome is getting shorter.

## File findings

Call mcp__recur__report_finding for each cut. file, line, and end_line locate
it: do not repeat those in message. message is `tag: what to cut. what
replaces it.` severity: low for shrink, medium otherwise. Do not apply the fixes.

Tags:

- `delete:` dead code, unused flexibility, speculative feature. Replacement: nothing.
- `stdlib:` hand-rolled thing the standard library ships. Name the function.
- `native:` dependency or code doing what the platform already does. Name the feature.
- `yagni:` abstraction with one implementation, config nobody sets, layer with one caller.
- `shrink:` same logic, fewer lines. Show the shorter form.

## Examples

❌ "This EmailValidator class might be more complex than necessary, have you
considered whether all these validation rules are needed at this stage?"

✅ mcp__recur__report_finding: file: validators.ts, line: 12, end_line: 38,
severity: medium, message: `stdlib: 27-line validator class. "@" in email, 1 line, real validation is the confirmation mail.`

✅ mcp__recur__report_finding: file: dates.js, line: 4, severity: medium,
message: `native: moment.js imported for one format call. Intl.DateTimeFormat, 0 deps.`

✅ mcp__recur__report_finding: file: repo.py, line: 88, severity: medium,
message: `yagni: AbstractRepository with one implementation. Inline it until a second one exists.`

✅ mcp__recur__report_finding: file: client.ts, line: 52, end_line: 71,
severity: medium, message: `delete: retry wrapper around an idempotent local call. Nothing replaces it.`

✅ mcp__recur__report_finding: file: util.py, line: 30, end_line: 44,
severity: low, message: `shrink: manual loop builds dict. dict(zip(keys, values)), 1 line.`

## Scoring

End with the only metric that matters: `net: -<N> lines possible.`
Do not restate the filed findings in the closing reply.

If there is nothing to cut, file none and say `Lean already. Ship.` and stop.

## Boundaries

Scope: over-engineering and complexity only. Correctness bugs, security holes,
and performance are explicitly out of scope. Route them to /review, not this
pass. A single smoke test or `assert`-based self-check is the minimum, not
bloat: never flag it for deletion. Does not apply the fixes, only files them.
""".strip()

AUDIT_FOCUS = """
Review, repo-wide. Scan the whole tree instead of a diff (skip node_modules,
.git, and build output). Rank findings biggest cut first.

## File findings

Same as the simplify review: mcp__recur__report_finding. file/line/end_line
locate it; message is `tag: what to cut. what replaces it.` (no path or line in
the message). severity: low for shrink, medium otherwise. Do not apply the
fixes.

Tags:

- `delete:` dead code, unused flexibility, speculative feature. Replacement: nothing.
- `stdlib:` hand-rolled thing the standard library ships. Name the function.
- `native:` dependency or code doing what the platform already does. Name the feature.
- `yagni:` abstraction with one implementation, config nobody sets, layer with one caller.
- `shrink:` same logic, fewer lines. Show the shorter form.

## Hunt

Deps the stdlib or platform already ships, single-implementation interfaces,
factories with one product, wrappers that only delegate, files exporting one
thing, dead flags and config, hand-rolled stdlib.

## Scoring

End with `net: -<N> lines, -<M> deps possible.` Do not restate the filed
findings in the closing reply. Nothing to cut: `Lean already. Ship.`

## Boundaries

Scope: over-engineering and complexity only. Correctness bugs, security holes,
and performance are explicitly out of scope. Route them to /review. Files
findings, applies nothing. One-shot.
""".strip()

if surface == "github_pr":
    label = pr_label()
    titled = f' titled "{pr_title}"' if pr_title else ""
    if again:
        intro = (
            f"Please re-review {label}{titled} after the recent fixes. Do not make code changes."
        )
        display = f"Please re-review GitHub PR #{pr_num}" if pr_num and pr_num != "0" else "Please re-review this GitHub pull request"
    else:
        intro = (
            f"Review {label}{titled} and file every real bug, risk, or issue "
            "you find as a finding (mcp__recur__report_finding). Do not make code changes."
        )
        display = f"Please review GitHub PR #{pr_num}" if pr_num and pr_num != "0" else "Please review this GitHub pull request"
    host_note = (
        " cwd is the project's working copy, not a Recur worktree of this PR head: "
        "do NOT use `git diff` in cwd; use the embedded `gh pr diff` below."
        if host
        else " Prefer the embedded `gh pr diff` (the PR as on GitHub) over uncommitted worktree changes."
    )
    gather = (
        "PR context is already gathered below: do NOT re-run git status/diff or `gh pr diff` "
        "unless something looks incomplete or you need a specific file region. Prefer "
        "reading the changed files that matter for each finding."
    )
    if again:
        gather += " Call mcp__recur__list_findings when you need prior finding state."
    parts = [
        intro + host_note,
        "",
        "Findings stay in Recur as local drafts until the operator posts them as GitHub review comments.",
        "",
        gather,
        "",
        open_m,
        f"Current cwd branch: {branch}",
        f"PR head: {pr_branch or '(unknown)'}  base: {pr_base or base or '(unknown)'}",
        "",
        "## gh pr view --json",
        scrub(pr_view.strip() or "(empty: gh pr view failed; use gh pr view with the number above)"),
        "",
        "## gh pr diff --stat",
        scrub(pr_stat.strip() or "(empty)"),
        "",
        "## gh pr diff",
        "```diff",
        scrub(pr_diff if pr_diff.strip() else "(empty: fetch with gh pr diff)"),
        "```",
        close_m,
    ]
elif surface == "local_pr":
    n = pr_num if pr_num and pr_num != "0" else ""
    who = f"Local PR #{n}" if n else "this Local PR"
    titled = f' titled "{pr_title}"' if pr_title else ""
    if again and since_review:
        intro = (
            f"Please re-review {who}{titled}: only what changed since your last review pass is "
            "embedded below: the rest is already in this conversation. Re-check your earlier "
            "findings against these changes and file anything new. Do not make code changes."
        )
        display = f"Please re-review Local PR #{n} since the last pass" if n else "Please re-review this Local PR since the last pass"
    elif again:
        intro = f"Please re-review {who}{titled} after the recent fixes. Do not make code changes."
        display = f"Please re-review Local PR #{n}" if n else "Please re-review this Local PR"
    else:
        intro = (
            f"Review {who}{titled} and file every real bug, risk, or issue "
            "you find as a finding (mcp__recur__report_finding). Do not make code changes."
        )
        display = f"Please review Local PR #{n}" if n else "Please review this Local PR"
    gather = (
        "Git context is already gathered below: do NOT re-run git status/diff/log "
        "unless something looks incomplete or you need a specific file region. Prefer "
        "reading the changed files that matter for each finding."
    )
    if again:
        gather += " Call mcp__recur__list_findings when you need prior finding state."
    parts = [
        intro,
        "",
        "This is Recur's worktree-diff review, not a GitHub pull request.",
        "",
        gather,
        "",
        open_m,
        f"Current branch: {branch}",
        f"Compare base: {base} (merge-base {merge_base})",
        "",
        "## git status --short",
        scrub(status.strip() or "(clean)"),
        "",
        "## Untracked files",
        scrub(untracked.strip() or "(none)"),
        "",
    ] + diff_sections + [close_m]
else:
    if again and since_review:
        intro = (
            "Please re-review this worktree: only what changed since your last review pass is "
            "embedded below: the rest is already in this conversation. Re-check your earlier "
            "findings against these changes and file anything new. Do not make code changes."
        )
        display = "Please re-review what changed in this worktree since the last pass"
    elif again:
        intro = "Please re-review this worktree after the recent fixes. Do not make code changes."
        display = "Please re-review the changes in this worktree after the recent fixes"
    else:
        intro = (
            "Review the changes in this worktree and file every real bug, risk, or issue "
            "you find as a finding (mcp__recur__report_finding). Do not make code changes."
        )
        display = "Please review the changes in this worktree and file any real issues"
    gather = (
        "Git context is already gathered below: do NOT re-run git status/diff/log "
        "unless something looks incomplete or you need a specific file region. Prefer "
        "reading the changed files that matter for each finding."
    )
    if again:
        gather += " Call mcp__recur__list_findings when you need prior finding state."
    parts = [
        intro,
        "",
        gather,
        "",
        open_m,
        f"Current branch: {branch}",
        f"Compare base: {base} (merge-base {merge_base})",
        "",
        "## git status --short",
        scrub(status.strip() or "(clean)"),
        "",
        "## Untracked files",
        scrub(untracked.strip() or "(none)"),
        "",
    ] + diff_sections + [close_m]

if kind == "simplify":
    needle = (
        "and file every real bug, risk, or issue "
        "you find as a finding (mcp__recur__report_finding). Do not make code changes."
    )
    repl = (
        "for over-engineering only and file each cut as a finding "
        "(mcp__recur__report_finding). Do not make code changes."
    )
    parts[0] = parts[0].replace(needle, repl)
    if again:
        parts[0] = parts[0].replace(
            "Do not make code changes.",
            "This pass is remaining over-engineering only. Do not make code changes.",
            1,
        )
        display = "Simplify re-review: file remaining over-engineering findings"
    else:
        display = "Simplify review: file over-engineering findings"
    parts[1:1] = ["", SIMPLIFY_FOCUS]
elif kind == "audit":
    display = "Simplify audit: file over-engineering findings"
    if surface == "github_pr":
        host_bit = (
            " cwd is the project's working copy, not a checkout of this PR head."
            if host
            else " This surface is not a Recur worktree of the PR head."
        )
        print(json.dumps({
            "prompt": (
                "Refused: /simplify-audit is a whole-repository pass."
                + host_bit
                + " Use /simplify-review for over-engineering in the PR diff. "
                "Do not scan cwd and do not file findings."
            ),
            "display": "Simplify audit isn't available on a GitHub PR: use /simplify-review",
        }, ensure_ascii=False))
        raise SystemExit(0)
    # Do not keep the review git-diff embed (or "prefer the embedded diff").
    # Audit scans the tree; empty or truncated diffs would bias the pass.
    parts = [
        "Audit this repository for over-engineering. Scan the whole tree "
        "(skip node_modules, .git, and build output). File each cut as a finding "
        "(mcp__recur__report_finding). Do not make code changes.",
        "",
        AUDIT_FOCUS,
    ]
    if branch:
        parts[1:1] = ["", f"Current branch: {branch}"]

print(json.dumps({
    "prompt": "\n".join(parts),
    "display": display,
}, ensure_ascii=False))
