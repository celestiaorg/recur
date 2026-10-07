package app

import (
	"testing"
	"time"

	"recur/internal/domain"
	"recur/internal/engine"
	"recur/internal/store"
)

func TestListScheduledPromptsAcrossProjects(t *testing.T) {
	t.Setenv("RECUR_DATA_DIR", t.TempDir())
	st, err := store.New()
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { st.Close() })
	for _, id := range []string{"one", "two"} {
		if _, err := st.AddProject(domain.Project{ID: id, Name: id}); err != nil {
			t.Fatal(err)
		}
		if err := st.SaveScheduledPrompt(domain.ScheduledPrompt{ID: id, ProjectID: id, Name: id, Prompt: "p", BaseBranch: "main", Mode: "ask", Model: "claude-code/sonnet", Cadence: "cron", Cron: "0 9 * * *", TimeZone: "UTC"}); err != nil {
			t.Fatal(err)
		}
	}
	eng := engine.New(st)
	t.Cleanup(eng.Shutdown)
	svc := NewServices(eng)
	all, err := svc.Projects.ListScheduledPrompts("")
	if err != nil || len(all) != 2 {
		t.Fatalf("ListScheduledPrompts(all) = %+v, %v, want two schedules", all, err)
	}
	for _, id := range []string{"one", "two"} {
		got, err := svc.Projects.ListScheduledPrompts(id)
		if err != nil || len(got) != 1 || got[0].ProjectID != id {
			t.Errorf("ListScheduledPrompts(%s) = %+v, %v, want only its schedule", id, got, err)
		}
	}
}

func TestPreviewScheduledPrompt(t *testing.T) {
	svc := &ProjectsService{}
	p := domain.ScheduledPrompt{Cadence: "cron", Cron: "0 9 1,15 * *", TimeZone: "UTC"}
	times, err := svc.PreviewScheduledPrompt(p)
	if err != nil || len(times) != 3 {
		t.Fatalf("preview = %v, %v", times, err)
	}
	for i, at := range times {
		date := time.UnixMilli(at).UTC()
		if date.Hour() != 9 || (date.Day() != 1 && date.Day() != 15) || (i > 0 && at <= times[i-1]) {
			t.Fatalf("unexpected preview %v", times)
		}
	}
	p.Cron = "bad cron"
	if _, err := svc.PreviewScheduledPrompt(p); err == nil {
		t.Fatal("preview accepted invalid cron")
	}
	p.Cron = "@reboot"
	if times, err := svc.PreviewScheduledPrompt(p); err != nil || len(times) != 0 {
		t.Fatalf("startup preview = %v, %v", times, err)
	}
}
