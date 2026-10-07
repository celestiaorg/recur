package store

import (
	"strings"
	"testing"
	"time"

	"recur/internal/domain"
)

func TestScheduledPromptPersistsClaimsAcrossRestart(t *testing.T) {
	dir := t.TempDir()
	s := openTemp(t, dir)
	off := false
	p := domain.ScheduledPrompt{Flags: map[string]string{"effort": "low"}, Simplify: &off, ID: "daily", ProjectID: s.Projects()[0].ID, Name: "Daily",
		Prompt: "check", BaseBranch: "main", Mode: "ask", Model: "claude-code/sonnet", Cadence: "cron", Cron: "0 9 * * *", TimeZone: "UTC", NextRunAt: 100}
	if err := s.SaveScheduledPrompt(p); err != nil {
		t.Fatal(err)
	}
	r := domain.ScheduledPromptRun{ID: "run", ScheduleID: p.ID, DueAt: 100, Status: "running"}
	p.NextRunAt = 200
	if ok, err := s.ClaimScheduledPrompt(p, r); err != nil || !ok {
		t.Fatalf("claim = %v, %v, want true, nil", ok, err)
	}
	s.Close()
	s = openTemp(t, dir)
	all, err := s.ScheduledPrompts()
	if err != nil {
		t.Fatal(err)
	}
	if len(all) != 1 || all[0].NextRunAt != 200 || all[0].LastRun == nil || all[0].LastRun.Status != "running" {
		t.Fatalf("reopened schedule = %+v, want next=200 and running occurrence", all)
	}
	if all[0].Flags["effort"] != "low" || all[0].Simplify == nil || *all[0].Simplify {
		t.Fatalf("lost schedule options: %+v", all[0])
	}
	r.ID = "duplicate"
	if ok, err := s.ClaimScheduledPrompt(p, r); err != nil || ok {
		t.Errorf("duplicate claim = %v, %v, want false, nil", ok, err)
	}
	if _, err := s.DeleteProject(p.ProjectID); err != nil {
		t.Fatal(err)
	}
	all, err = s.ScheduledPrompts()
	if err != nil || len(all) != 0 {
		t.Errorf("project delete schedules = %+v, %v, want empty", all, err)
	}
	runs, err := s.ScheduledPromptRuns()
	if err != nil || len(runs) != 0 {
		t.Errorf("project delete runs = %+v, %v, want empty", runs, err)
	}
}

func TestDueAndActiveScheduledPromptQueries(t *testing.T) {
	s := openTemp(t, t.TempDir())
	off := false
	proj := s.Projects()[0].ID
	due := domain.ScheduledPrompt{ID: "due", ProjectID: proj, Name: "Due", Prompt: "go", BaseBranch: "main",
		Mode: "ask", Model: "claude-code/sonnet", Cadence: "cron", Cron: "0 9 * * *", TimeZone: "UTC", NextRunAt: 100, Simplify: &off}
	later := due
	later.ID, later.Name, later.NextRunAt = "later", "Later", 9_999_999_999_000
	paused := due
	paused.ID, paused.Name, paused.Paused = "paused", "Paused", true
	reboot := due
	reboot.ID, reboot.Name, reboot.Cron, reboot.NextRunAt = "boot", "Boot", "@reboot", 0
	for _, p := range []domain.ScheduledPrompt{due, later, paused, reboot} {
		if err := s.SaveScheduledPrompt(p); err != nil {
			t.Fatal(err)
		}
	}
	now := time.UnixMilli(200)
	got, err := s.DueScheduledPrompts(now, false)
	if err != nil || len(got) != 1 || got[0].ID != "due" {
		t.Fatalf("due tick = %+v, %v, want only due", got, err)
	}
	got, err = s.DueScheduledPrompts(now, true)
	if err != nil || len(got) != 2 {
		t.Fatalf("startup due = %+v, %v, want due+reboot", got, err)
	}
	running := domain.ScheduledPromptRun{ID: "r1", ScheduleID: due.ID, DueAt: 50, Status: "running"}
	done := domain.ScheduledPromptRun{ID: "r2", ScheduleID: due.ID, DueAt: 10, Status: "finished"}
	if ok, err := s.ClaimScheduledPrompt(due, running); err != nil || !ok {
		t.Fatalf("claim running = %v, %v", ok, err)
	}
	if ok, err := s.ClaimScheduledPrompt(due, done); err != nil || !ok {
		t.Fatalf("claim finished = %v, %v", ok, err)
	}
	active, err := s.ActiveScheduledPromptRuns()
	if err != nil || len(active) != 1 || active[0].ID != "r1" {
		t.Fatalf("active runs = %+v, %v, want running only", active, err)
	}
	stopped := domain.ScheduledPromptRun{ID: "r3", ScheduleID: due.ID, DueAt: 60, Status: "interrupted"}
	if ok, err := s.ClaimScheduledPrompt(due, stopped); err != nil || !ok {
		t.Fatalf("claim interrupted = %v, %v", ok, err)
	}
	active, err = s.ActiveScheduledPromptRuns()
	if err != nil || len(active) != 2 || active[0].ID != "r1" || active[1].ID != "r3" {
		t.Fatalf("active runs = %+v, %v, want running and interrupted", active, err)
	}
}

func TestActiveScheduledPromptRunsUsesPartialIndex(t *testing.T) {
	s := openTemp(t, t.TempDir())
	rows, err := s.db.sql.Query(`EXPLAIN QUERY PLAN ` + runSelect + ` WHERE ` + activeRunFilter + ` ORDER BY created_at, rowid`)
	if err != nil {
		t.Fatal(err)
	}
	defer rows.Close()
	var plan []string
	for rows.Next() {
		var id, parent, notused int
		var detail string
		if err := rows.Scan(&id, &parent, &notused, &detail); err != nil {
			t.Fatal(err)
		}
		plan = append(plan, detail)
	}
	if err := rows.Err(); err != nil {
		t.Fatal(err)
	}
	joined := strings.Join(plan, "; ")
	if !strings.Contains(joined, "scheduled_prompt_run_active") {
		t.Errorf("EXPLAIN QUERY PLAN(%s) = %q, want scheduled_prompt_run_active", activeRunFilter, joined)
	}
	if strings.Contains(joined, "TEMP B-TREE") {
		t.Errorf("EXPLAIN QUERY PLAN(%s) = %q, want no TEMP B-TREE", activeRunFilter, joined)
	}
}

func TestScheduledPromptRunOutcomeKeepsClearedWorkspace(t *testing.T) {
	s := openTemp(t, t.TempDir())
	proj := s.Projects()[0]
	ws, err := s.AddWorkspace(domain.Workspace{ID: "sched-ws", ProjectID: proj.ID, Goal: "g"})
	if err != nil {
		t.Fatal(err)
	}
	off := false
	p := domain.ScheduledPrompt{ID: "once", ProjectID: proj.ID, Name: "Once", Prompt: "go", BaseBranch: "main",
		Mode: "ask", Model: "claude-code/sonnet", Cadence: "once", Date: "2026-09-10", Time: "09:00", TimeZone: "UTC", NextRunAt: 100, Simplify: &off}
	if err := s.SaveScheduledPrompt(p); err != nil {
		t.Fatal(err)
	}
	r := domain.ScheduledPromptRun{ID: "run", ScheduleID: p.ID, DueAt: 100, Status: "running"}
	if ok, err := s.ClaimScheduledPrompt(p, r); err != nil || !ok {
		t.Fatalf("claim = %v, %v", ok, err)
	}
	if err := s.LinkScheduledPromptRun(r.ID, ws.ID); err != nil {
		t.Fatal(err)
	}
	if err := s.DeleteWorkspace(ws.ID); err != nil {
		t.Fatal(err)
	}
	r.WorkspaceID, r.Status, r.Detail = ws.ID, "failed", "workspace gone"
	if err := s.UpdateScheduledPromptRun(r); err != nil {
		t.Fatalf("finalize after delete: %v", err)
	}
	runs, err := s.ScheduledPromptRuns()
	if err != nil || len(runs) != 1 || runs[0].Status != "failed" || runs[0].WorkspaceID != "" {
		t.Fatalf("run after delete = %+v, %v, want failed with cleared workspace", runs, err)
	}
}
