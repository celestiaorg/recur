package store

import (
	"database/sql"
	"fmt"
	"time"

	"recur/internal/domain"
)

// Time columns are ISO 8601 UTC text with milliseconds; the domain carries
// unix ms. ” means no time (a one-time schedule that already ran).
const isoMillis = "2006-01-02T15:04:05.000Z"

func isoFromMillis(ms int64) string {
	if ms == 0 {
		return ""
	}
	return time.UnixMilli(ms).UTC().Format(isoMillis)
}

func millisFromISO(s string) (int64, error) {
	if s == "" {
		return 0, nil
	}
	t, err := time.Parse(time.RFC3339Nano, s)
	if err != nil {
		return 0, fmt.Errorf("bad timestamp %q: %w", s, err)
	}
	return t.UnixMilli(), nil
}

const scheduleColumns = `id, project_id, name, prompt, base_branch, mode, model, simplify, loop_id, policy_id,
	cadence, cron, run_date, run_time, time_zone, paused, next_run_at`

func scanSchedule(rows *sql.Rows) (domain.ScheduledPrompt, error) {
	var p domain.ScheduledPrompt
	var simplify bool
	var next string
	if err := rows.Scan(&p.ID, &p.ProjectID, &p.Name, &p.Prompt, &p.BaseBranch, &p.Mode, &p.Model, &simplify, &p.LoopID, &p.PolicyID,
		&p.Cadence, &p.Cron, &p.Date, &p.Time, &p.TimeZone, &p.Paused, &next); err != nil {
		return p, err
	}
	p.Simplify = &simplify
	var err error
	p.NextRunAt, err = millisFromISO(next)
	return p, err
}

func scanRun(rows *sql.Rows) (domain.ScheduledPromptRun, error) {
	var r domain.ScheduledPromptRun
	var due string
	if err := rows.Scan(&r.ID, &r.ScheduleID, &due, &r.WorkspaceID, &r.Status, &r.Detail); err != nil {
		return r, err
	}
	var err error
	r.DueAt, err = millisFromISO(due)
	return r, err
}

func (s *Store) querySchedules(q string, args ...any) ([]domain.ScheduledPrompt, error) {
	out := []domain.ScheduledPrompt{}
	if s.db == nil {
		return out, nil
	}
	rows, err := s.db.sql.Query(q, args...)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	byID := map[string]int{}
	for rows.Next() {
		p, err := scanSchedule(rows)
		if err != nil {
			return nil, err
		}
		p.Flags = map[string]string{}
		byID[p.ID] = len(out)
		out = append(out, p)
	}
	if err := rows.Err(); err != nil {
		return nil, err
	}
	if len(out) == 0 {
		return out, nil
	}
	flags, err := s.db.sql.Query(`SELECT schedule_id, key, value FROM scheduled_prompt_flag`)
	if err != nil {
		return nil, err
	}
	defer flags.Close()
	for flags.Next() {
		var id, k, v string
		if err := flags.Scan(&id, &k, &v); err != nil {
			return nil, err
		}
		if i, ok := byID[id]; ok {
			out[i].Flags[k] = v
		}
	}
	if err := flags.Err(); err != nil {
		return nil, err
	}
	return out, nil
}

// ScheduledPrompts reads schedules with their flags and latest occurrence.
func (s *Store) ScheduledPrompts() ([]domain.ScheduledPrompt, error) {
	out, err := s.querySchedules(`SELECT ` + scheduleColumns + ` FROM scheduled_prompt ORDER BY created_at, rowid`)
	if err != nil {
		return nil, err
	}
	if s.db == nil || len(out) == 0 {
		return out, nil
	}
	byID := map[string]int{}
	for i, p := range out {
		byID[p.ID] = i
	}
	// Latest occurrence per schedule (most recently created row).
	last, err := s.db.sql.Query(`SELECT r.id, r.schedule_id, r.due_at, COALESCE(r.workspace_id, ''), r.status, r.detail
		FROM scheduled_prompt_run r
		WHERE r.rowid = (SELECT rowid FROM scheduled_prompt_run WHERE schedule_id = r.schedule_id ORDER BY created_at DESC, rowid DESC LIMIT 1)`)
	if err != nil {
		return nil, err
	}
	defer last.Close()
	for last.Next() {
		r, err := scanRun(last)
		if err != nil {
			return nil, err
		}
		if i, ok := byID[r.ScheduleID]; ok {
			run := r
			out[i].LastRun = &run
		}
	}
	return out, last.Err()
}

// DueScheduledPrompts is the scheduler tick set: unpaused rows that are due or
// @reboot on startup. It skips last-run history.
func (s *Store) DueScheduledPrompts(now time.Time, startup bool) ([]domain.ScheduledPrompt, error) {
	start := 0
	if startup {
		start = 1
	}
	return s.querySchedules(`SELECT `+scheduleColumns+` FROM scheduled_prompt
		WHERE paused = 0 AND ((next_run_at <> '' AND next_run_at <= ?) OR (? = 1 AND cadence = 'cron' AND cron = '@reboot'))
		ORDER BY created_at, rowid`, isoFromMillis(now.UnixMilli()), start)
}

func scheduleArgs(p domain.ScheduledPrompt) []any {
	simplify := p.Simplify == nil || *p.Simplify
	return []any{p.ID, p.ProjectID, p.Name, p.Prompt, p.BaseBranch, p.Mode, p.Model, simplify, p.LoopID, p.PolicyID,
		p.Cadence, p.Cron, p.Date, p.Time, p.TimeZone, p.Paused, isoFromMillis(p.NextRunAt)}
}

// upsertSchedule writes the schedule row and replaces its flags. A schedule's
// project is fixed: an update that names another project changes nothing.
func upsertSchedule(tx *sql.Tx, p domain.ScheduledPrompt) error {
	res, err := tx.Exec(`INSERT INTO scheduled_prompt(`+scheduleColumns+`)
		VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)
		ON CONFLICT(id) DO UPDATE SET
			name=excluded.name, prompt=excluded.prompt, base_branch=excluded.base_branch, mode=excluded.mode,
			model=excluded.model, simplify=excluded.simplify, loop_id=excluded.loop_id, policy_id=excluded.policy_id,
			cadence=excluded.cadence, cron=excluded.cron, run_date=excluded.run_date, run_time=excluded.run_time,
			time_zone=excluded.time_zone, paused=excluded.paused, next_run_at=excluded.next_run_at,
			updated_at=strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
		WHERE project_id = excluded.project_id`, scheduleArgs(p)...)
	if err != nil {
		return err
	}
	if n, _ := res.RowsAffected(); n == 0 {
		return fmt.Errorf("schedule %s belongs to another project", p.ID)
	}
	if _, err := tx.Exec(`DELETE FROM scheduled_prompt_flag WHERE schedule_id=?`, p.ID); err != nil {
		return err
	}
	for k, v := range p.Flags {
		if _, err := tx.Exec(`INSERT INTO scheduled_prompt_flag(schedule_id, key, value) VALUES(?,?,?)`, p.ID, k, v); err != nil {
			return err
		}
	}
	return nil
}

// SaveScheduledPrompt persists configuration without changing run history.
func (s *Store) SaveScheduledPrompt(p domain.ScheduledPrompt) error {
	if s.db == nil {
		return fmt.Errorf("schedules require a database")
	}
	tx, err := s.db.sql.Begin()
	if err != nil {
		return err
	}
	defer tx.Rollback()
	if err := upsertSchedule(tx, p); err != nil {
		return err
	}
	return tx.Commit()
}

// DeleteScheduledPrompt deletes future scheduling and history, keeping workspaces.
func (s *Store) DeleteScheduledPrompt(id string) error {
	if s.db == nil {
		return fmt.Errorf("schedules require a database")
	}
	_, err := s.db.sql.Exec(`DELETE FROM scheduled_prompt WHERE id=?`, id)
	return err
}

func (s *Store) queryRuns(q string, args ...any) ([]domain.ScheduledPromptRun, error) {
	out := []domain.ScheduledPromptRun{}
	if s.db == nil {
		return out, nil
	}
	rows, err := s.db.sql.Query(q, args...)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	for rows.Next() {
		r, err := scanRun(rows)
		if err != nil {
			return nil, err
		}
		out = append(out, r)
	}
	return out, rows.Err()
}

const runSelect = `SELECT id, schedule_id, due_at, COALESCE(workspace_id, ''), status, detail FROM scheduled_prompt_run`

// ScheduledPromptRuns returns occurrences oldest first, including active runs.
func (s *Store) ScheduledPromptRuns() ([]domain.ScheduledPromptRun, error) {
	return s.queryRuns(runSelect + ` ORDER BY created_at, rowid`)
}

// activeRunFilter is running, waiting, and interrupted. Interrupted stays in
// the overlap set so a restart does not launch over unanswered work.
const activeRunFilter = `status IN ('running', 'waiting', 'interrupted')`

// ActiveScheduledPromptRuns is occurrences that still block the next launch.
func (s *Store) ActiveScheduledPromptRuns() ([]domain.ScheduledPromptRun, error) {
	return s.queryRuns(runSelect + ` WHERE ` + activeRunFilter + ` ORDER BY created_at, rowid`)
}

// ClaimScheduledPrompt atomically records the occurrence and advances the
// schedule. A duplicate due time is a no-op, even after a process restart.
func (s *Store) ClaimScheduledPrompt(p domain.ScheduledPrompt, r domain.ScheduledPromptRun) (bool, error) {
	if s.db == nil {
		return false, fmt.Errorf("schedules require a database")
	}
	tx, err := s.db.sql.Begin()
	if err != nil {
		return false, err
	}
	defer tx.Rollback()
	res, err := tx.Exec(`INSERT INTO scheduled_prompt_run(id, schedule_id, due_at, status, detail) VALUES(?,?,?,?,?)
		ON CONFLICT(schedule_id, due_at) DO NOTHING`, r.ID, p.ID, isoFromMillis(r.DueAt), r.Status, r.Detail)
	if err != nil {
		return false, err
	}
	n, err := res.RowsAffected()
	if err != nil || n == 0 {
		return false, err
	}
	if err := upsertSchedule(tx, p); err != nil {
		return false, err
	}
	return true, tx.Commit()
}

// LinkScheduledPromptRun associates an occurrence with its workspace.
func (s *Store) LinkScheduledPromptRun(id, workspaceID string) error {
	if s.db == nil {
		return fmt.Errorf("schedules require a database")
	}
	_, err := s.db.sql.Exec(`UPDATE scheduled_prompt_run SET workspace_id=? WHERE id=?`, workspaceID, id)
	return err
}

// UpdateScheduledPromptRun records completion or failure without rewriting the
// workspace link, so a deleted workspace's SET NULL is preserved.
func (s *Store) UpdateScheduledPromptRun(r domain.ScheduledPromptRun) error {
	if s.db == nil {
		return fmt.Errorf("schedules require a database")
	}
	_, err := s.db.sql.Exec(`UPDATE scheduled_prompt_run SET status=?, detail=? WHERE id=?`, r.Status, r.Detail, r.ID)
	return err
}
