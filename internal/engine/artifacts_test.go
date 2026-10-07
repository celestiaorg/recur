package engine

import (
	"encoding/json"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"recur/internal/domain"
	"recur/internal/mcp"
	"recur/internal/store"
)

func TestCreateAndPresentArtifactEvents(t *testing.T) {
	eng, _, sink, _ := newTestEngine(t, nil)

	a, err := eng.CreateArtifact("ws1", "PA", mcp.CreateArtifactRequest{
		Title: "Notes", Kind: "md", Content: "# hi\n",
	})
	if err != nil {
		t.Fatal(err)
	}
	if a.ID == "" || a.Kind != domain.ArtifactMD {
		t.Fatalf("created artifact: %+v", a)
	}
	created := sink.byName("artifact.created")
	if len(created) != 1 {
		t.Fatalf("artifact.created count=%d", len(created))
	}

	list, err := eng.ListArtifacts("ws1")
	if err != nil {
		t.Fatal(err)
	}
	if len(list) != 1 || list[0].ID != a.ID {
		t.Fatalf("list: %+v", list)
	}
	if n := len(sink.byName("artifact.presented")); n != 0 {
		t.Fatalf("list_artifacts must not present: presented=%d", n)
	}

	msg := eng.PresentArtifact("ws1", a.ID)
	if !strings.Contains(msg, "Presented artifact") || !strings.Contains(msg, "Notes") {
		t.Fatalf("present msg: %q", msg)
	}
	presented := sink.byName("artifact.presented")
	if len(presented) != 1 {
		t.Fatalf("artifact.presented count=%d", len(presented))
	}
	data, _ := presented[0].Data.(map[string]any)
	got, _ := data["artifact"].(domain.Artifact)
	if got.ID != a.ID || data["workspaceId"] != "ws1" {
		t.Fatalf("presented payload: %+v", data)
	}

	if msg := eng.PresentArtifact("ws1", "missing"); !strings.HasPrefix(msg, "Artifact not found") {
		t.Fatalf("missing present: %q", msg)
	}
	if n := len(sink.byName("artifact.presented")); n != 1 {
		t.Fatalf("missing id published presented: %d", n)
	}
}

func TestReadArtifact(t *testing.T) {
	eng, _, sink, _ := newTestEngine(t, nil)
	a, err := eng.CreateArtifact("ws1", "PA", mcp.CreateArtifactRequest{
		Title: "Notes", Kind: "md", Content: "# hi\n",
	})
	if err != nil {
		t.Fatal(err)
	}

	got, err := eng.ReadArtifact("ws1", a.ID)
	if err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(got, a.ID) || !strings.Contains(got, "# hi") {
		t.Fatalf("read: %s", got)
	}
	if n := len(sink.byName("artifact.presented")); n != 0 {
		t.Fatalf("read_artifact must not present: presented=%d", n)
	}
	if _, err := eng.ReadArtifact("ws1", "missing"); err == nil {
		t.Fatal("missing id should error")
	}

	png, err := eng.CreateArtifact("ws1", "PA", mcp.CreateArtifactRequest{
		Title: "Dot", Kind: "png",
		Content: "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==",
	})
	if err != nil {
		t.Fatal(err)
	}
	got, err = eng.ReadArtifact("ws1", png.ID)
	if err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(got, `"binary":true`) {
		t.Fatalf("png should flag binary: %s", got)
	}
	if strings.Contains(got, `"content"`) {
		t.Fatalf("png must not inline bytes: %s", got)
	}

	body := strings.Repeat("x", maxArtifactBytes+1024)
	big, err := eng.CreateArtifact("ws1", "PA", mcp.CreateArtifactRequest{
		Title: "Big", Kind: "md", Content: body,
	})
	if err != nil {
		t.Fatal(err)
	}
	got, err = eng.ReadArtifact("ws1", big.ID)
	if err != nil {
		t.Fatal(err)
	}
	var parsed struct {
		Content string `json:"content"`
	}
	if err := json.Unmarshal([]byte(got), &parsed); err != nil {
		t.Fatalf("ReadArtifact(%s) JSON: %v", big.ID, err)
	}
	if parsed.Content != body {
		t.Errorf("ReadArtifact(%s) content len = %d, want %d (full body past mention cap)",
			big.ID, len(parsed.Content), len(body))
	}
	if strings.Contains(parsed.Content, "…(truncated)") {
		t.Errorf("ReadArtifact(%s) content includes truncation marker, want full body", big.ID)
	}
}

func TestHoistPRChatHostMovesArtifactsAndPlanPaths(t *testing.T) {
	eng, st, sink, _ := newTestEngine(t, nil)
	addSoloWorkspace(t, st, "W-live")
	addSoloWorkspace(t, st, "W-host")
	a, err := eng.CreateArtifact("W-host", "PA", mcp.CreateArtifactRequest{
		Title: "Notes", Kind: "md", Content: "# host\n",
	})
	if err != nil {
		t.Fatal(err)
	}
	st.UpdateWorkspace("W-host", func(w *domain.Workspace) {
		w.Plan = &domain.Plan{
			Summary: "Plan",
			Attachments: []domain.PlanAttachment{{
				ID: a.ID, Kind: "md", Title: "Notes", Path: a.Path,
			}},
		}
	})
	if err := eng.hoistPRChatHost("W-host", "W-live"); err != nil {
		t.Fatalf("hoistPRChatHost: %v", err)
	}
	got, err := eng.ListArtifacts("W-live")
	if err != nil {
		t.Fatal(err)
	}
	if len(got) != 1 || got[0].ID != a.ID || got[0].WorkspaceID != "W-live" {
		t.Fatalf("ListArtifacts(W-live) = %+v, want hoisted %s", got, a.ID)
	}
	wantPath := "workspaces/W-live/artifacts/" + a.ID + ".md"
	if got[0].Path != wantPath {
		t.Errorf("hoisted Path = %q, want %q", got[0].Path, wantPath)
	}
	if got[0].CreatedBy != "PA" {
		t.Errorf("hoisted CreatedBy = %q, want PA", got[0].CreatedBy)
	}
	hostList, err := eng.ListArtifacts("W-host")
	if err != nil {
		t.Fatal(err)
	}
	if len(hostList) != 0 {
		t.Fatalf("ListArtifacts(W-host) = %+v, want empty after hoist", hostList)
	}
	dest, ok := st.Workspace("W-live")
	if !ok || dest.Plan == nil || len(dest.Plan.Attachments) != 1 {
		t.Fatalf("dest plan attachments = %+v, want rewritten host attachment", dest.Plan)
	}
	if dest.Plan.Attachments[0].Path != wantPath {
		t.Errorf("plan attachment Path = %q, want %q", dest.Plan.Attachments[0].Path, wantPath)
	}
	found := false
	for _, ev := range sink.byName("artifact.created") {
		d, _ := ev.Data.(map[string]any)
		if d["workspaceId"] == "W-live" {
			found = true
			break
		}
	}
	if !found {
		t.Error("hoist did not publish artifact.created for dest")
	}
}

func TestHoistWorkspaceArtifactsSkipsDestCollision(t *testing.T) {
	eng, _, _, _ := newTestEngine(t, nil)
	dest, err := eng.CreateArtifact("W-live", "PA", mcp.CreateArtifactRequest{
		Title: "Dest", Kind: "md", Content: "dest\n",
	})
	if err != nil {
		t.Fatal(err)
	}
	root, err := store.DataDir()
	if err != nil {
		t.Fatal(err)
	}
	hostDir, err := store.ArtifactsDir(root, "W-host")
	if err != nil {
		t.Fatal(err)
	}
	filename := dest.ID + ".md"
	if err := os.WriteFile(filepath.Join(hostDir, filename), []byte("host\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	hostArt := dest
	hostArt.WorkspaceID = "W-host"
	hostArt.Title = "Host"
	hostArt.Path = "workspaces/W-host/artifacts/" + filename
	meta, err := json.MarshalIndent(hostArt, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(hostDir, dest.ID+".meta.json"), meta, 0o644); err != nil {
		t.Fatal(err)
	}
	if err := eng.hoistWorkspaceArtifacts("W-host", "W-live", nil); err != nil {
		t.Fatalf("hoistWorkspaceArtifacts: %v", err)
	}
	list, err := eng.ListArtifacts("W-live")
	if err != nil {
		t.Fatal(err)
	}
	if len(list) != 1 || list[0].ID != dest.ID || list[0].Title != "Dest" {
		t.Fatalf("ListArtifacts(W-live) = %+v, want dest-owned %s", list, dest.ID)
	}
	hostList, err := eng.ListArtifacts("W-host")
	if err != nil {
		t.Fatal(err)
	}
	if len(hostList) != 1 || hostList[0].Title != "Host" {
		t.Fatalf("ListArtifacts(W-host) = %+v, want skipped host artifact", hostList)
	}
}

func TestRewriteHoistArtifactPath(t *testing.T) {
	t.Parallel()
	got := rewriteHoistArtifactPath("workspaces/W-host/artifacts/a.md", "W-host", "W-live")
	if got != "workspaces/W-live/artifacts/a.md" {
		t.Errorf("rewriteHoistArtifactPath(...) = %q, want workspaces/W-live/artifacts/a.md", got)
	}
	keep := rewriteHoistArtifactPath("workspaces/other/artifacts/a.md", "W-host", "W-live")
	if keep != "workspaces/other/artifacts/a.md" {
		t.Errorf("rewriteHoistArtifactPath(unrelated) = %q, want original", keep)
	}
}

func TestCommitHoistedArtifactRollsBackMetaWriteFailure(t *testing.T) {
	dir := t.TempDir()
	srcFile := filepath.Join(dir, "src.md")
	destFile := filepath.Join(dir, "dest.md")
	srcMeta := filepath.Join(dir, "src.meta.json")
	destMeta := filepath.Join(dir, "dest.meta.json")
	if err := os.WriteFile(destFile, []byte("host\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(srcMeta, []byte(`{"id":"a"}`), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := os.Mkdir(destMeta, 0o755); err != nil {
		t.Fatal(err)
	}
	err := commitHoistedArtifact(srcFile, destFile, srcMeta, destMeta, domain.Artifact{
		ID: "a", Title: "Notes", Kind: domain.ArtifactMD,
	})
	if err == nil {
		t.Fatal("commitHoistedArtifact error = nil, want metadata write failure")
	}
	if _, err := os.Stat(srcMeta); err != nil {
		t.Fatalf("source metadata removed after failed dest write: %v", err)
	}
	if _, err := os.Stat(srcFile); err != nil {
		t.Fatalf("content was not rolled back to source: %v", err)
	}
	if _, err := os.Stat(destFile); !os.IsNotExist(err) {
		t.Fatalf("dest file still present after rollback: %v", err)
	}
}

func TestHoistWorkspaceArtifactsReturnsOnMoveFailure(t *testing.T) {
	eng, _, _, _ := newTestEngine(t, nil)
	a, err := eng.CreateArtifact("W-host", "PA", mcp.CreateArtifactRequest{
		Title: "Notes", Kind: "md", Content: "# host\n",
	})
	if err != nil {
		t.Fatal(err)
	}
	root, err := store.DataDir()
	if err != nil {
		t.Fatal(err)
	}
	destDir, err := store.ArtifactsDir(root, "W-live")
	if err != nil {
		t.Fatal(err)
	}
	if err := os.Chmod(destDir, 0o555); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = os.Chmod(destDir, 0o755) })
	if err := eng.hoistWorkspaceArtifacts("W-host", "W-live", nil); err == nil {
		t.Fatal("hoistWorkspaceArtifacts error = nil, want move failure")
	}
	hostList, err := eng.ListArtifacts("W-host")
	if err != nil {
		t.Fatal(err)
	}
	if len(hostList) != 1 || hostList[0].ID != a.ID {
		t.Fatalf("ListArtifacts(W-host) = %+v, want source metadata preserved", hostList)
	}
}

func TestHoistPRChatHostFailureDoesNotAliasRouting(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, nil)
	addSoloWorkspace(t, st, "W-live")
	addSoloWorkspace(t, st, "W-host")
	if _, err := eng.CreateArtifact("W-host", "PA", mcp.CreateArtifactRequest{
		Title: "Notes", Kind: "md", Content: "# host\n",
	}); err != nil {
		t.Fatal(err)
	}
	st.UpsertAgent("W-host", domain.Agent{ID: "W-host-PA", Code: "PA", Name: "Agent", SessionID: "prov-host"})
	root, err := store.DataDir()
	if err != nil {
		t.Fatal(err)
	}
	destDir, err := store.ArtifactsDir(root, "W-live")
	if err != nil {
		t.Fatal(err)
	}
	if err := os.Chmod(destDir, 0o555); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = os.Chmod(destDir, 0o755) })
	if err := eng.hoistPRChatHost("W-host", "W-live"); err == nil {
		t.Fatal("hoistPRChatHost error = nil, want move failure")
	}
	if id, code := eng.resolveHoistedChat("W-host", "PA"); id != "W-host" || code != "PA" {
		t.Fatalf("resolveHoistedChat = %s/%s, want W-host/PA after failed hoist", id, code)
	}
	if _, ok := st.Agent("W-host", "PA"); !ok {
		t.Fatal("host agent missing after failed hoist")
	}
}

func TestRenameAndDeleteArtifact(t *testing.T) {
	eng, _, _, _ := newTestEngine(t, nil)
	a, err := eng.CreateArtifact("ws1", "PA", mcp.CreateArtifactRequest{
		Title: "Notes", Kind: "md", Content: "# hi\n",
	})
	if err != nil {
		t.Fatal(err)
	}
	if _, err := eng.RenameArtifact("ws1", a.ID, "  "); err == nil {
		t.Fatal("blank title should error")
	}
	if _, err := eng.RenameArtifact("ws1", a.ID, "Renamed"); err != nil {
		t.Fatal(err)
	}
	list, _ := eng.ListArtifacts("ws1")
	if len(list) != 1 || list[0].Title != "Renamed" || list[0].Path != a.Path {
		t.Fatalf("after rename: %+v", list)
	}

	full, err := eng.ArtifactAbsPath("ws1", a.ID)
	if err != nil {
		t.Fatal(err)
	}
	if err := eng.DeleteArtifact("ws1", a.ID); err != nil {
		t.Fatal(err)
	}
	if list, _ := eng.ListArtifacts("ws1"); len(list) != 0 {
		t.Fatalf("after delete: %+v", list)
	}
	if _, err := os.Stat(full); !os.IsNotExist(err) {
		t.Fatalf("content file left behind: %v", err)
	}
	if err := eng.DeleteArtifact("ws1", a.ID); err == nil {
		t.Fatal("deleting a missing artifact should error")
	}
}
