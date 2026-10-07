package store

import (
	"database/sql"
	"encoding/json"
	"fmt"
	"sync/atomic"
	"time"

	_ "modernc.org/sqlite" // pure-Go SQLite driver (DECISIONS D5: no cgo)

	"recur/internal/domain"
)

// db is the SQLite persistence layer behind the in-memory Store.
//
// Persistence strategy (PRD §14): the simple, queried-by-id tables (projects,
// loops, policy, settings, inbox, escalations) are normalized into columns;
// the rich nested aggregates (a PR's commits/files/diff lines, an agent's steps,
// a loop's feed items, convergence series) are stored as JSON blobs in a single
// column. v1 reads/writes whole aggregates rather than querying their interiors,
// so JSON keeps the schema small without losing fidelity. See DECISIONS.
type db struct {
	sql *sql.DB

	// feedSort hands out strictly decreasing sort values for prepended feed rows.
	// It is seeded at or below the smallest sort already on disk (and never above
	// zero), so a prepend can never collide with an existing row and always sorts
	// ahead of the 0..N-1 numbering a full rewrite produces.
	feedSort atomic.Int64
}

// openDB opens (creating if needed) the SQLite database at path and applies
// the schema migrations.
func openDB(path string) (*db, error) {
	sqlDB, err := sql.Open("sqlite", path)
	if err != nil {
		return nil, fmt.Errorf("open sqlite %q: %w", path, err)
	}
	// modernc's driver is goroutine-safe via the database/sql pool, but the
	// Store already serializes all access under its mutex; a single connection
	// avoids "database is locked" on the single-file db.
	sqlDB.SetMaxOpenConns(1)
	// A transient lock — a just-closed connection's WAL checkpoint still settling on
	// app restart, or a quick close→reopen — should WAIT briefly rather than fail
	// immediately with SQLITE_BUSY. Set the busy timeout on the (single) connection
	// before migrate so the schema DDL itself is covered.
	if _, err := sqlDB.Exec("PRAGMA busy_timeout=5000"); err != nil {
		sqlDB.Close()
		return nil, fmt.Errorf("set busy_timeout: %w", err)
	}
	d := &db{sql: sqlDB}
	if err := d.migrate(); err != nil {
		sqlDB.Close()
		return nil, err
	}
	if err := d.seedFeedSort(); err != nil {
		sqlDB.Close()
		return nil, err
	}
	// Replay can leave hours of committed rows in the WAL; flush them into
	// the main file so a crash or a file copy is not a last-checkpoint view.
	d.checkpoint()
	return d, nil
}

// seedFeedSort primes the prepend counter from what is already persisted, so
// feed rows written in this session sort ahead of rows written in earlier ones.
func (d *db) seedFeedSort() error {
	var min int64
	if err := d.sql.QueryRow(`SELECT COALESCE(MIN(sort), 0) FROM feed`).Scan(&min); err != nil {
		return fmt.Errorf("seed feed sort: %w", err)
	}
	if min > 0 {
		min = 0
	}
	d.feedSort.Store(min)
	return nil
}

func (d *db) close() error {
	d.checkpoint()
	return d.sql.Close()
}

// checkpoint moves committed WAL frames into recur.db. TRUNCATE so a byte-copy
// of the main file (or a CLI sqlite3 that cannot see this process's WAL) still
// has every committed row. Best-effort: a busy writer leaves the WAL in place.
func (d *db) checkpoint() {
	if d == nil || d.sql == nil {
		return
	}
	_, _ = d.sql.Exec(`PRAGMA wal_checkpoint(TRUNCATE)`)
}

// migrate brings the database to the latest schema version via the ordered,
// versioned migration runner (see migrations.go). Historically this was a single
// idempotent CREATE ... IF NOT EXISTS block; that block is now migration 1
// (schemaV1Baseline), and the session tables are migration 2 — so an existing v1
// database migrates in place without losing data.
func (d *db) migrate() error {
	return d.runMigrations()
}

// needsSeed reports whether the database has never been initialized — keyed on the
// SETTINGS singleton, NOT on project count. This is deliberate: a user who deletes
// every project (or runs "Clear all data") must NOT be re-seeded — their DB stays
// initialized and their policy/settings are preserved. Only a brand-new (or
// hand-deleted) DB has no settings row.
func (d *db) needsSeed() (bool, error) {
	var n int
	if err := d.sql.QueryRow(`SELECT COUNT(*) FROM settings`).Scan(&n); err != nil {
		return false, fmt.Errorf("count settings: %w", err)
	}
	return n == 0, nil
}

// deleteByWorkspace removes every row a loop owns from a loop-keyed table (agents/feed/
// findings/events/escalations). Used by DeleteProject's cascade.
func (d *db) deleteByWorkspace(table, workspaceID string) error {
	if _, err := d.sql.Exec(fmt.Sprintf(`DELETE FROM %s WHERE workspace_id = ?`, table), workspaceID); err != nil {
		return fmt.Errorf("delete %s by loop %q: %w", table, workspaceID, err)
	}
	return nil
}

// clearData deletes all user/demo DATA (projects, loops, and everything hanging off
// them) but KEEPS the policy/settings/resource singletons and the PR number
// sequence, so "Clear all data" gives a clean app without resetting the user's
// policy — and the DB stays initialized (needsSeed stays false, so the demo never
// comes back). PR numbers keep climbing so a wipe cannot reuse #1.
func (d *db) clearData() error {
	// "session" cascades message/message_content/session_context (FKs ON).
	for _, t := range []string{"github_branch_rules", "projects", "project_groups", "workspaces", "agents", "feed", "prs", "escalations", "inbox", "findings", "events", "analysis", "session"} {
		if _, err := d.sql.Exec(`DELETE FROM ` + t); err != nil {
			return fmt.Errorf("clear %s: %w", t, err)
		}
	}
	return nil
}

// ---- persist (write the whole in-memory state once, used on seed) ----

// seedAll writes the entire in-memory state into an empty database in one tx.
func (d *db) seedAll(s *Store) error {
	tx, err := d.sql.Begin()
	if err != nil {
		return err
	}
	defer tx.Rollback() //nolint:errcheck // no-op after Commit

	for i, g := range s.projectGroups {
		if err := upsertProjectGroupTx(tx, g, i); err != nil {
			return err
		}
	}
	for i, p := range s.projects {
		if err := upsertProjectTx(tx, p, i); err != nil {
			return err
		}
	}
	for i, l := range s.workspaces {
		if err := upsertDocTx(tx, "workspaces", l.ID, l, i); err != nil {
			return err
		}
	}
	for workspaceID, agents := range s.agents {
		if err := replaceListTx(tx, "agents", workspaceID, toAny(agents)); err != nil {
			return err
		}
	}
	for workspaceID, items := range s.feed {
		if err := replaceListTx(tx, "feed", workspaceID, toAny(items)); err != nil {
			return err
		}
	}
	for i, p := range s.prs {
		if err := upsertDocTx(tx, "prs", p.ID, p, i); err != nil {
			return err
		}
	}
	for workspaceID, e := range s.escalations {
		if err := upsertEscalationTx(tx, workspaceID, e); err != nil {
			return err
		}
	}
	for i, it := range s.inbox {
		if err := upsertDocTx(tx, "inbox", it.ID, it, i); err != nil {
			return err
		}
	}
	for workspaceID, fs := range s.findings {
		for i, f := range fs {
			if err := upsertFindingTx(tx, f, workspaceID, i); err != nil {
				return err
			}
		}
	}
	for _, a := range s.analysis {
		if err := upsertAnalysisTx(tx, a); err != nil {
			return err
		}
	}
	if err := upsertSingletonTx(tx, "policy", s.policy); err != nil {
		return err
	}
	if err := upsertSingletonTx(tx, "settings", s.settings); err != nil {
		return err
	}
	if err := upsertSingletonTx(tx, "resource", s.resource); err != nil {
		return err
	}
	if err := setPRNumberSeqTx(tx, s.prNumberSeq); err != nil {
		return err
	}
	return tx.Commit()
}

// ---- load (rebuild the in-memory state from the database) ----

// loadAll reads the persisted state back into the in-memory Store fields.
func (d *db) loadAll(s *Store) error {
	var err error
	if s.projectGroups, err = d.loadProjectGroups(); err != nil {
		return err
	}
	if s.projects, err = d.loadProjects(); err != nil {
		return err
	}
	if s.workspaces, err = loadDocList[domain.Workspace](d, "workspaces"); err != nil {
		return err
	}
	if s.prs, err = loadDocList[domain.PR](d, "prs"); err != nil {
		return err
	}
	if err := d.backfillPRTimes(s); err != nil {
		return err
	}
	if s.inbox, err = loadDocList[domain.InboxItem](d, "inbox"); err != nil {
		return err
	}
	if s.agents, err = loadDocByWorkspace[domain.Agent](d, "agents"); err != nil {
		return err
	}
	if s.feed, err = loadDocByWorkspace[domain.FeedItem](d, "feed"); err != nil {
		return err
	}
	if s.escalations, err = d.loadEscalations(); err != nil {
		return err
	}
	if s.findings, err = loadFindings(d); err != nil {
		return err
	}
	if s.analysis, err = loadAnalysis(d); err != nil {
		return err
	}
	if err = loadSingleton(d, "policy", &s.policy); err != nil {
		return err
	}
	if err = loadSingleton(d, "settings", &s.settings); err != nil {
		return err
	}
	if err = loadSingleton(d, "resource", &s.resource); err != nil {
		return err
	}
	if s.prNumberSeq, err = d.loadPRNumberSeq(); err != nil {
		return err
	}
	if s.githubBranchRules, err = d.loadGitHubBranchRules(); err != nil {
		return err
	}
	return nil
}

// projectExtra is the JSON shape of projects.extra_json (machine-local fields).
type projectExtra struct {
	Pic                  string                     `json:"pic"`
	Color                string                     `json:"color"`
	Open                 bool                       `json:"open"`
	Idle                 bool                       `json:"idle"`
	Hidden               bool                       `json:"hidden,omitempty"`
	PROnGitHub           bool                       `json:"prOnGithub"`
	DeleteBranchOnRemove bool                       `json:"deleteBranchOnRemove,omitempty"`
	MergeMethod          domain.MergeMethod         `json:"mergeMethod,omitempty"`
	UpdateBaseMethod     domain.UpdateBaseMethod    `json:"updateBaseMethod,omitempty"`
	SquashCommitMessage  domain.SquashCommitMessage `json:"squashCommitMessage,omitempty"`
	MergeCommitMessage   domain.MergeCommitMessage  `json:"mergeCommitMessage,omitempty"`
	PinnedRun            string                     `json:"pinnedRun,omitempty"`
	DisabledSkills       []string                   `json:"disabledSkills,omitempty"`
	EnabledSkills        []string                   `json:"enabledSkills,omitempty"`
	DisabledMCP          []string                   `json:"disabledMCP,omitempty"`
	EnabledMCP           []string                   `json:"enabledMCP,omitempty"`
}

func (d *db) loadProjectGroups() ([]domain.ProjectGroup, error) {
	rows, err := d.sql.Query(`SELECT id, name, open, created_at, updated_at FROM project_groups ORDER BY sort`)
	if err != nil {
		return nil, fmt.Errorf("query project_groups: %w", err)
	}
	defer rows.Close()
	var out []domain.ProjectGroup
	for rows.Next() {
		var g domain.ProjectGroup
		var open int
		if err := rows.Scan(&g.ID, &g.Name, &open, &g.CreatedAt, &g.UpdatedAt); err != nil {
			return nil, fmt.Errorf("scan project_group: %w", err)
		}
		g.Open = open != 0
		out = append(out, g)
	}
	return out, rows.Err()
}

func (d *db) loadProjects() ([]domain.Project, error) {
	rows, err := d.sql.Query(`SELECT id, name, repo_path, base_branch, group_id, extra_json FROM projects ORDER BY sort`)
	if err != nil {
		return nil, fmt.Errorf("query projects: %w", err)
	}
	defer rows.Close()
	var out []domain.Project
	for rows.Next() {
		var p domain.Project
		var extra string
		if err := rows.Scan(&p.ID, &p.Name, &p.RepoPath, &p.BaseBranch, &p.GroupID, &extra); err != nil {
			return nil, fmt.Errorf("scan project: %w", err)
		}
		// extra holds presentation + ADE fields not given their own columns.
		var e projectExtra
		if err := json.Unmarshal([]byte(extra), &e); err != nil {
			return nil, fmt.Errorf("unmarshal project extra: %w", err)
		}
		p.Pic, p.Color, p.Open, p.Idle, p.Hidden = e.Pic, e.Color, e.Open, e.Idle, e.Hidden
		p.PROnGitHub, p.DeleteBranchOnRemove = e.PROnGitHub, e.DeleteBranchOnRemove
		p.MergeMethod, p.UpdateBaseMethod = e.MergeMethod, e.UpdateBaseMethod
		p.SquashCommitMessage, p.MergeCommitMessage = e.SquashCommitMessage, e.MergeCommitMessage
		p.PinnedRun = e.PinnedRun
		p.DisabledSkills, p.EnabledSkills = e.DisabledSkills, e.EnabledSkills
		p.DisabledMCP, p.EnabledMCP = e.DisabledMCP, e.EnabledMCP
		out = append(out, p)
	}
	return out, rows.Err()
}

func (d *db) loadEscalations() (map[string]domain.Escalation, error) {
	rows, err := d.sql.Query(`SELECT doc_json FROM escalations`)
	if err != nil {
		return nil, fmt.Errorf("query escalations: %w", err)
	}
	defer rows.Close()
	out := map[string]domain.Escalation{}
	for rows.Next() {
		var doc string
		if err := rows.Scan(&doc); err != nil {
			return nil, fmt.Errorf("scan escalation: %w", err)
		}
		var e domain.Escalation
		if err := json.Unmarshal([]byte(doc), &e); err != nil {
			return nil, fmt.Errorf("unmarshal escalation: %w", err)
		}
		out[escalationKey(e.WorkspaceID, e.AgentCode)] = e
	}
	return out, rows.Err()
}

// ---- generic loaders ----

func loadDocList[T any](d *db, table string) ([]T, error) {
	rows, err := d.sql.Query(fmt.Sprintf(`SELECT doc_json FROM %s ORDER BY sort`, table))
	if err != nil {
		return nil, fmt.Errorf("query %s: %w", table, err)
	}
	defer rows.Close()
	var out []T
	for rows.Next() {
		var doc string
		if err := rows.Scan(&doc); err != nil {
			return nil, fmt.Errorf("scan %s: %w", table, err)
		}
		var v T
		if err := json.Unmarshal([]byte(doc), &v); err != nil {
			return nil, fmt.Errorf("unmarshal %s: %w", table, err)
		}
		out = append(out, v)
	}
	return out, rows.Err()
}

func loadDocByWorkspace[T any](d *db, table string) (map[string][]T, error) {
	rows, err := d.sql.Query(fmt.Sprintf(`SELECT workspace_id, doc_json FROM %s ORDER BY sort`, table))
	if err != nil {
		return nil, fmt.Errorf("query %s: %w", table, err)
	}
	defer rows.Close()
	out := map[string][]T{}
	for rows.Next() {
		var workspaceID, doc string
		if err := rows.Scan(&workspaceID, &doc); err != nil {
			return nil, fmt.Errorf("scan %s: %w", table, err)
		}
		var v T
		if err := json.Unmarshal([]byte(doc), &v); err != nil {
			return nil, fmt.Errorf("unmarshal %s: %w", table, err)
		}
		out[workspaceID] = append(out[workspaceID], v)
	}
	return out, rows.Err()
}

// loadFindings rebuilds the per-loop findings map from the findings table.
func loadFindings(d *db) (map[string][]domain.Finding, error) {
	rows, err := d.sql.Query(`SELECT workspace_id, doc_json FROM findings ORDER BY workspace_id, sort, id`)
	if err != nil {
		return nil, fmt.Errorf("query findings: %w", err)
	}
	defer rows.Close()
	out := map[string][]domain.Finding{}
	for rows.Next() {
		var workspaceID, doc string
		if err := rows.Scan(&workspaceID, &doc); err != nil {
			return nil, fmt.Errorf("scan finding: %w", err)
		}
		var f domain.Finding
		if err := json.Unmarshal([]byte(doc), &f); err != nil {
			return nil, fmt.Errorf("unmarshal finding: %w", err)
		}
		ensureFindingCreatedAt(&f)
		out[workspaceID] = append(out[workspaceID], f)
	}
	return out, rows.Err()
}

func loadSingleton[T any](d *db, table string, dst *T) error {
	var doc string
	err := d.sql.QueryRow(fmt.Sprintf(`SELECT doc_json FROM %s WHERE id = 1`, table)).Scan(&doc)
	if err == sql.ErrNoRows {
		return nil // leave the zero value
	}
	if err != nil {
		return fmt.Errorf("query %s: %w", table, err)
	}
	if err := json.Unmarshal([]byte(doc), dst); err != nil {
		return fmt.Errorf("unmarshal %s: %w", table, err)
	}
	return nil
}

// ---- write-through helpers (called by Store mutations, each its own tx) ----

func (d *db) upsertProject(p domain.Project, sort int) error {
	return upsertProjectTx(d.sql, p, sort)
}

func (d *db) upsertProjectGroup(g domain.ProjectGroup, sort int) error {
	return upsertProjectGroupTx(d.sql, g, sort)
}

// deleteProjectGroupAndUngroup clears member projects' group_id then deletes the
// group row in one transaction so a partial failure cannot leave them ungrouped
// while the group still exists.
func (d *db) deleteProjectGroupAndUngroup(id string) error {
	tx, err := d.sql.Begin()
	if err != nil {
		return err
	}
	defer tx.Rollback() //nolint:errcheck // no-op after Commit
	if _, err := tx.Exec(`UPDATE projects SET group_id = '' WHERE group_id = ?`, id); err != nil {
		return fmt.Errorf("clear project group_id %q: %w", id, err)
	}
	if _, err := tx.Exec(`DELETE FROM project_groups WHERE id = ?`, id); err != nil {
		return fmt.Errorf("delete project_group %q: %w", id, err)
	}
	return tx.Commit()
}

// setProjectSorts writes projects.sort = 0..N-1 for the given id order in one
// transaction. Caller must pass a complete permutation of existing project ids.
func (d *db) setProjectSorts(ids []string) error {
	tx, err := d.sql.Begin()
	if err != nil {
		return err
	}
	defer tx.Rollback() //nolint:errcheck // no-op after Commit
	for i, id := range ids {
		res, err := tx.Exec(`UPDATE projects SET sort = ? WHERE id = ?`, i, id)
		if err != nil {
			return fmt.Errorf("set project sort %q: %w", id, err)
		}
		n, err := res.RowsAffected()
		if err != nil {
			return fmt.Errorf("set project sort %q rows: %w", id, err)
		}
		if n == 0 {
			return fmt.Errorf("set project sort: unknown project %q", id)
		}
	}
	return tx.Commit()
}

// setProjectGroupSorts writes project_groups.sort = 0..N-1 for the given id order.
func (d *db) setProjectGroupSorts(ids []string) error {
	tx, err := d.sql.Begin()
	if err != nil {
		return err
	}
	defer tx.Rollback() //nolint:errcheck // no-op after Commit
	for i, id := range ids {
		res, err := tx.Exec(`UPDATE project_groups SET sort = ? WHERE id = ?`, i, id)
		if err != nil {
			return fmt.Errorf("set project_group sort %q: %w", id, err)
		}
		n, err := res.RowsAffected()
		if err != nil {
			return fmt.Errorf("set project_group sort %q rows: %w", id, err)
		}
		if n == 0 {
			return fmt.Errorf("set project_group sort: unknown group %q", id)
		}
	}
	return tx.Commit()
}

func (d *db) upsertDoc(table, id string, v any, sort int) error {
	return upsertDocTx(d.sql, table, id, v, sort)
}

// deleteDoc removes a single doc-table row by id (used by inbox row removal when an
// escalation is answered). No-op-safe: deleting a missing id affects 0 rows.
func (d *db) deleteDoc(table, id string) error {
	if _, err := d.sql.Exec(fmt.Sprintf(`DELETE FROM %s WHERE id = ?`, table), id); err != nil {
		return fmt.Errorf("delete %s %q: %w", table, id, err)
	}
	return nil
}

func (d *db) upsertEscalation(workspaceID string, e domain.Escalation) error {
	return upsertEscalationTx(d.sql, workspaceID, e)
}

func (d *db) deleteEscalations(workspaceID string) error {
	_, err := d.sql.Exec(`DELETE FROM escalations WHERE workspace_id = ? OR workspace_id LIKE ?`,
		workspaceID, workspaceID+escalationKeySep+"%")
	if err != nil {
		return fmt.Errorf("delete escalations for %q: %w", workspaceID, err)
	}
	return nil
}

func (d *db) upsertSingleton(table string, v any) error {
	return upsertSingletonTx(d.sql, table, v)
}

func (d *db) setPRNumberSeq(n int) error {
	return setPRNumberSeqTx(d.sql, n)
}

func (d *db) loadPRNumberSeq() (int, error) {
	var n int
	err := d.sql.QueryRow(`SELECT last_number FROM pr_seq WHERE id = 1`).Scan(&n)
	if err == sql.ErrNoRows {
		return 0, nil
	}
	if err != nil {
		return 0, fmt.Errorf("query pr_seq: %w", err)
	}
	return n, nil
}

// replaceList swaps all rows for a workspace_id in a per-loop table (agents/feed).
func (d *db) replaceList(table, workspaceID string, items []any) error {
	tx, err := d.sql.Begin()
	if err != nil {
		return err
	}
	defer tx.Rollback() //nolint:errcheck
	if err := replaceListTx(tx, table, workspaceID, items); err != nil {
		return err
	}
	return tx.Commit()
}

// prependFeedItem inserts ONE new newest-first feed row.
//
// The feed used to be persisted with replaceList: delete every row the loop
// owns, then re-insert and re-marshal all of them, on every single item an
// agent streamed. That is O(N) writes per append and O(N²) over a loop's life.
// Ordering is preserved instead by giving the new row a sort value below every
// existing one, since rows load back ORDER BY sort.
func (d *db) prependFeedItem(workspaceID string, item domain.FeedItem) error {
	doc, err := json.Marshal(item)
	if err != nil {
		return err
	}
	if _, err := d.sql.Exec(
		`INSERT INTO feed (workspace_id, doc_json, sort) VALUES (?,?,?)`,
		workspaceID, string(doc), d.feedSort.Add(-1),
	); err != nil {
		return fmt.Errorf("prepend feed for %q: %w", workspaceID, err)
	}
	return nil
}

// appendEvent inserts one durable event (append-only; never updated in place).
// Returns the autoincrement row id assigned by SQLite.
func (d *db) appendEvent(e domain.StoredEvent) (int64, error) {
	res, err := d.sql.Exec(
		`INSERT INTO events (workspace_id, agent_id, type, payload_json, ts) VALUES (?,?,?,?,?)`,
		e.WorkspaceID, e.AgentID, e.Type, e.PayloadJSON, e.TS,
	)
	if err != nil {
		return 0, fmt.Errorf("append event: %w", err)
	}
	return res.LastInsertId()
}

// upsertFinding inserts or replaces one finding row (matched by id), keyed by
// workspace_id with a stable sort for ordering.
func (d *db) upsertFinding(f domain.Finding, sort int) error {
	return upsertFindingTx(d.sql, f, f.WorkspaceID, sort)
}

func (d *db) upsertAnalysis(a domain.AnalysisRun) error {
	return upsertAnalysisTx(d.sql, a)
}

func upsertAnalysisTx(x execer, a domain.AnalysisRun) error {
	doc, err := json.Marshal(a)
	if err != nil {
		return err
	}
	_, err = x.Exec(
		`INSERT INTO analysis (id, project_id, workspace_id, doc_json, ts) VALUES (?,?,?,?,?)
			ON CONFLICT(id) DO UPDATE SET doc_json=excluded.doc_json, project_id=excluded.project_id, workspace_id=excluded.workspace_id, ts=excluded.ts`,
		a.ID, a.ProjectID, a.WorkspaceID, string(doc), a.TS,
	)
	if err != nil {
		return fmt.Errorf("upsert analysis %q: %w", a.ID, err)
	}
	return nil
}

func loadAnalysis(d *db) ([]domain.AnalysisRun, error) {
	rows, err := d.sql.Query(`SELECT doc_json FROM analysis ORDER BY ts DESC`)
	if err != nil {
		return nil, fmt.Errorf("query analysis: %w", err)
	}
	defer rows.Close()
	var out []domain.AnalysisRun
	for rows.Next() {
		var doc string
		if err := rows.Scan(&doc); err != nil {
			return nil, fmt.Errorf("scan analysis: %w", err)
		}
		var a domain.AnalysisRun
		if err := json.Unmarshal([]byte(doc), &a); err != nil {
			return nil, fmt.Errorf("unmarshal analysis: %w", err)
		}
		out = append(out, a)
	}
	return out, rows.Err()
}

// upsertFindingTx writes one finding via any execer (db or tx).
func upsertFindingTx(x execer, f domain.Finding, workspaceID string, sort int) error {
	doc, err := json.Marshal(f)
	if err != nil {
		return err
	}
	_, err = x.Exec(
		`INSERT INTO findings (id, workspace_id, doc_json, sort) VALUES (?,?,?,?)
			ON CONFLICT(id) DO UPDATE SET doc_json=excluded.doc_json, workspace_id=excluded.workspace_id`,
		f.ID, workspaceID, string(doc), sort,
	)
	if err != nil {
		return fmt.Errorf("upsert finding %q: %w", f.ID, err)
	}
	return nil
}

// findingsForWorkspace returns a loop's findings in insertion order.
func (d *db) findingsForWorkspace(workspaceID string) ([]domain.Finding, error) {
	rows, err := d.sql.Query(`SELECT doc_json FROM findings WHERE workspace_id = ? ORDER BY sort, id`, workspaceID)
	if err != nil {
		return nil, fmt.Errorf("query findings for %q: %w", workspaceID, err)
	}
	defer rows.Close()
	var out []domain.Finding
	for rows.Next() {
		var doc string
		if err := rows.Scan(&doc); err != nil {
			return nil, fmt.Errorf("scan finding: %w", err)
		}
		var f domain.Finding
		if err := json.Unmarshal([]byte(doc), &f); err != nil {
			return nil, fmt.Errorf("unmarshal finding: %w", err)
		}
		ensureFindingCreatedAt(&f)
		out = append(out, f)
	}
	return out, rows.Err()
}

// eventsForWorkspace returns every durable event for a loop in insertion order.
func (d *db) eventsForWorkspace(workspaceID string) ([]domain.StoredEvent, error) {
	rows, err := d.sql.Query(
		`SELECT id, workspace_id, agent_id, type, payload_json, ts FROM events WHERE workspace_id = ? ORDER BY id`,
		workspaceID,
	)
	if err != nil {
		return nil, fmt.Errorf("query events for %q: %w", workspaceID, err)
	}
	defer rows.Close()
	var out []domain.StoredEvent
	for rows.Next() {
		var e domain.StoredEvent
		var agentID sql.NullString
		if err := rows.Scan(&e.ID, &e.WorkspaceID, &agentID, &e.Type, &e.PayloadJSON, &e.TS); err != nil {
			return nil, fmt.Errorf("scan event: %w", err)
		}
		e.AgentID = agentID.String
		out = append(out, e)
	}
	return out, rows.Err()
}

// ---- tx-level primitives (work on *sql.DB or *sql.Tx via execer) ----

type execer interface {
	Exec(query string, args ...any) (sql.Result, error)
}

func upsertProjectGroupTx(x execer, g domain.ProjectGroup, sort int) error {
	open := 0
	if g.Open {
		open = 1
	}
	_, err := x.Exec(`INSERT INTO project_groups (id, name, open, sort, created_at, updated_at)
		VALUES (?,?,?,?,?,?)
		ON CONFLICT(id) DO UPDATE SET
			name=excluded.name, open=excluded.open, updated_at=excluded.updated_at`,
		g.ID, g.Name, open, sort, g.CreatedAt, g.UpdatedAt)
	if err != nil {
		return fmt.Errorf("upsert project_group %q: %w", g.ID, err)
	}
	return nil
}

func upsertProjectTx(x execer, p domain.Project, sort int) error {
	// extra_json carries machine-local fields not given their own columns.
	// Run-config (setup/teardown/runs/env) lives only in project.toml.
	extra, err := json.Marshal(domain.Project{
		Pic: p.Pic, Color: p.Color, Open: p.Open, Idle: p.Idle, Hidden: p.Hidden,
		PROnGitHub: p.PROnGitHub, DeleteBranchOnRemove: p.DeleteBranchOnRemove,
		MergeMethod: p.MergeMethod, UpdateBaseMethod: p.UpdateBaseMethod,
		SquashCommitMessage: p.SquashCommitMessage, MergeCommitMessage: p.MergeCommitMessage,
		PinnedRun:      p.PinnedRun,
		DisabledSkills: p.DisabledSkills, EnabledSkills: p.EnabledSkills,
		DisabledMCP: p.DisabledMCP, EnabledMCP: p.EnabledMCP,
	})
	if err != nil {
		return err
	}
	_, err = x.Exec(`INSERT INTO projects (id, name, repo_path, base_branch, group_id, extra_json, sort)
		VALUES (?,?,?,?,?,?,?)
		ON CONFLICT(id) DO UPDATE SET
			name=excluded.name, repo_path=excluded.repo_path, base_branch=excluded.base_branch,
			group_id=excluded.group_id, extra_json=excluded.extra_json`,
		p.ID, p.Name, p.RepoPath, p.BaseBranch, p.GroupID, string(extra), sort)
	if err != nil {
		return fmt.Errorf("upsert project %q: %w", p.ID, err)
	}
	return nil
}

func upsertDocTx(x execer, table, id string, v any, sort int) error {
	doc, err := json.Marshal(v)
	if err != nil {
		return err
	}
	q := fmt.Sprintf(`INSERT INTO %s (id, doc_json, sort) VALUES (?,?,?)
		ON CONFLICT(id) DO UPDATE SET doc_json=excluded.doc_json`, table)
	if _, err := x.Exec(q, id, string(doc), sort); err != nil {
		return fmt.Errorf("upsert %s %q: %w", table, id, err)
	}
	return nil
}

func upsertEscalationTx(x execer, workspaceID string, e domain.Escalation) error {
	doc, err := json.Marshal(e)
	if err != nil {
		return err
	}
	_, err = x.Exec(`INSERT INTO escalations (workspace_id, doc_json) VALUES (?,?)
		ON CONFLICT(workspace_id) DO UPDATE SET doc_json=excluded.doc_json`, workspaceID, string(doc))
	if err != nil {
		return fmt.Errorf("upsert escalation %q: %w", workspaceID, err)
	}
	return nil
}

func upsertSingletonTx(x execer, table string, v any) error {
	doc, err := json.Marshal(v)
	if err != nil {
		return err
	}
	q := fmt.Sprintf(`INSERT INTO %s (id, doc_json) VALUES (1, ?)
		ON CONFLICT(id) DO UPDATE SET doc_json=excluded.doc_json`, table)
	if _, err := x.Exec(q, string(doc)); err != nil {
		return fmt.Errorf("upsert %s: %w", table, err)
	}
	return nil
}

func setPRNumberSeqTx(x execer, n int) error {
	_, err := x.Exec(`INSERT INTO pr_seq (id, last_number) VALUES (1, ?)
		ON CONFLICT(id) DO UPDATE SET last_number=excluded.last_number`, n)
	if err != nil {
		return fmt.Errorf("upsert pr_seq: %w", err)
	}
	return nil
}

func replaceListTx(tx *sql.Tx, table, workspaceID string, items []any) error {
	if _, err := tx.Exec(fmt.Sprintf(`DELETE FROM %s WHERE workspace_id = ?`, table), workspaceID); err != nil {
		return fmt.Errorf("clear %s for %q: %w", table, workspaceID, err)
	}
	for i, it := range items {
		doc, err := json.Marshal(it)
		if err != nil {
			return err
		}
		if _, err := tx.Exec(fmt.Sprintf(`INSERT INTO %s (workspace_id, doc_json, sort) VALUES (?,?,?)`, table), workspaceID, string(doc), i); err != nil {
			return fmt.Errorf("insert %s for %q: %w", table, workspaceID, err)
		}
	}
	return nil
}

func toAny[T any](in []T) []any {
	out := make([]any, len(in))
	for i, v := range in {
		out[i] = v
	}
	return out
}

type githubBranchRule struct {
	Require bool
	At      time.Time
}

func githubBranchRuleKey(projectID, repo, branch string) string {
	return projectID + "\x00" + repo + "\x00" + branch
}

func (d *db) loadGitHubBranchRules() (map[string]githubBranchRule, error) {
	rows, err := d.sql.Query(`SELECT project_id, repo, branch, require_pr, checked_at FROM github_branch_rules`)
	if err != nil {
		return nil, fmt.Errorf("query github_branch_rules: %w", err)
	}
	defer rows.Close()
	out := map[string]githubBranchRule{}
	for rows.Next() {
		var projectID, repo, branch string
		var require, checkedAt int64
		if err := rows.Scan(&projectID, &repo, &branch, &require, &checkedAt); err != nil {
			return nil, fmt.Errorf("scan github_branch_rules: %w", err)
		}
		out[githubBranchRuleKey(projectID, repo, branch)] = githubBranchRule{
			Require: require != 0,
			At:      time.UnixMilli(checkedAt),
		}
	}
	if err := rows.Err(); err != nil {
		return nil, err
	}
	return out, nil
}

func (d *db) upsertGitHubBranchRule(projectID, repo, branch string, e githubBranchRule) error {
	require := 0
	if e.Require {
		require = 1
	}
	_, err := d.sql.Exec(`INSERT INTO github_branch_rules (project_id, repo, branch, require_pr, checked_at)
		VALUES (?,?,?,?,?)
		ON CONFLICT(project_id, repo, branch) DO UPDATE SET require_pr=excluded.require_pr, checked_at=excluded.checked_at`,
		projectID, repo, branch, require, e.At.UnixMilli())
	if err != nil {
		return fmt.Errorf("upsert github_branch_rules %s %s %s: %w", projectID, repo, branch, err)
	}
	return nil
}
