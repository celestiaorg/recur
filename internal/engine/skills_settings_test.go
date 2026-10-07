package engine

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func TestListAndActivateSkillRespectDisabledSettings(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &captureRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "skills-settings", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}

	skillRoot := filepath.Join(ws.WorktreePath, ".agents", "skills", "noise-skill")
	if err := os.MkdirAll(skillRoot, 0o755); err != nil {
		t.Fatal(err)
	}
	body := "---\nname: noise-skill\ndescription: Noisy helper.\n---\n# Noise\n"
	if err := os.WriteFile(filepath.Join(skillRoot, "SKILL.md"), []byte(body), 0o644); err != nil {
		t.Fatal(err)
	}

	rep := engineReporter{eng: eng}
	list, err := rep.ListSkills(ws.ID, "PA")
	if err != nil {
		t.Fatalf("ListSkills: %v", err)
	}
	found := false
	for _, s := range list {
		if s.Name == "noise-skill" {
			found = true
			break
		}
	}
	if !found {
		t.Fatalf("expected noise-skill in catalog, got %#v", list)
	}

	set := st.Settings()
	set.DisabledSkills = []string{"noise-skill"}
	st.SetSettings(set)

	list, err = rep.ListSkills(ws.ID, "PA")
	if err != nil {
		t.Fatalf("ListSkills after disable: %v", err)
	}
	for _, s := range list {
		if s.Name == "noise-skill" {
			t.Fatal("disabled skill still listed")
		}
	}

	_, err = rep.ActivateSkill(ws.ID, "PA", "noise-skill")
	if err == nil || !strings.Contains(err.Error(), "disabled") {
		t.Fatalf("ActivateSkill want disabled error, got %v", err)
	}

	// Re-enable via empty list.
	set = st.Settings()
	set.DisabledSkills = nil
	st.SetSettings(set)
	out, err := rep.ActivateSkill(ws.ID, "PA", "noise-skill")
	if err != nil {
		t.Fatalf("ActivateSkill after enable: %v", err)
	}
	if !strings.Contains(out, `<skill_content name="noise-skill">`) {
		t.Fatalf("missing skill content: %s", out)
	}
}

func TestListSkillsRespectsProjectDisabledSkills(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &captureRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "proj-skills", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}

	skillRoot := filepath.Join(ws.WorktreePath, ".agents", "skills", "repo-only")
	if err := os.MkdirAll(skillRoot, 0o755); err != nil {
		t.Fatal(err)
	}
	body := "---\nname: repo-only\ndescription: Repo skill.\n---\n# Repo\n"
	if err := os.WriteFile(filepath.Join(skillRoot, "SKILL.md"), []byte(body), 0o644); err != nil {
		t.Fatal(err)
	}

	proj, ok := st.Project(projID)
	if !ok {
		t.Fatal("project missing")
	}
	proj.DisabledSkills = []string{"repo-only"}
	if _, err := st.UpdateProjectRunConfig(proj); err != nil {
		t.Fatalf("UpdateProjectRunConfig: %v", err)
	}

	rep := engineReporter{eng: eng}
	list, err := rep.ListSkills(ws.ID, "PA")
	if err != nil {
		t.Fatalf("ListSkills: %v", err)
	}
	for _, s := range list {
		if s.Name == "repo-only" {
			t.Fatal("project-disabled skill still listed")
		}
	}
	_, err = rep.ActivateSkill(ws.ID, "PA", "repo-only")
	if err == nil || !strings.Contains(err.Error(), "disabled") {
		t.Fatalf("ActivateSkill want disabled error, got %v", err)
	}
}

func TestListSkillsProjectEnableOverridesGlobalDisable(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &captureRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "proj-enable", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}

	skillRoot := filepath.Join(ws.WorktreePath, ".agents", "skills", "special")
	if err := os.MkdirAll(skillRoot, 0o755); err != nil {
		t.Fatal(err)
	}
	body := "---\nname: special\ndescription: Special.\n---\n# Special\n"
	if err := os.WriteFile(filepath.Join(skillRoot, "SKILL.md"), []byte(body), 0o644); err != nil {
		t.Fatal(err)
	}

	set := st.Settings()
	set.DisabledSkills = []string{"special"}
	st.SetSettings(set)

	rep := engineReporter{eng: eng}
	list, err := rep.ListSkills(ws.ID, "PA")
	if err != nil {
		t.Fatalf("ListSkills: %v", err)
	}
	for _, s := range list {
		if s.Name == "special" {
			t.Fatal("globally disabled skill should be omitted")
		}
	}

	proj, ok := st.Project(projID)
	if !ok {
		t.Fatal("project missing")
	}
	proj.EnabledSkills = []string{"special"}
	if _, err := st.UpdateProjectRunConfig(proj); err != nil {
		t.Fatalf("UpdateProjectRunConfig: %v", err)
	}

	list, err = rep.ListSkills(ws.ID, "PA")
	if err != nil {
		t.Fatalf("ListSkills after enable: %v", err)
	}
	found := false
	for _, s := range list {
		if s.Name == "special" {
			found = true
			break
		}
	}
	if !found {
		t.Fatal("project EnabledSkills should surface globally disabled skill")
	}
	if _, err := rep.ActivateSkill(ws.ID, "PA", "special"); err != nil {
		t.Fatalf("ActivateSkill after project enable: %v", err)
	}
}
