package engine

import (
	"encoding/base64"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"time"

	"github.com/google/uuid"

	"recur/internal/domain"
	"recur/internal/mcp"
	"recur/internal/store"
)

var artifactExt = map[domain.ArtifactKind]string{
	domain.ArtifactHTML:    ".html",
	domain.ArtifactSVG:     ".svg",
	domain.ArtifactMermaid: ".mmd",
	domain.ArtifactPNG:     ".png",
	domain.ArtifactJPEG:    ".jpg",
	domain.ArtifactCSV:     ".csv",
	domain.ArtifactJSON:    ".json",
	domain.ArtifactMD:      ".md",
}

func parseArtifactKind(s string) (domain.ArtifactKind, error) {
	k := domain.ArtifactKind(strings.ToLower(strings.TrimSpace(s)))
	if _, ok := artifactExt[k]; !ok {
		return "", fmt.Errorf("unsupported artifact kind %q", s)
	}
	return k, nil
}

func (r engineReporter) CreateArtifact(workspaceID, agentCode string, req mcp.CreateArtifactRequest) (domain.Artifact, error) {
	return r.eng.CreateArtifact(workspaceID, agentCode, req)
}

func (e *Engine) CreateArtifact(workspaceID, agentCode string, req mcp.CreateArtifactRequest) (domain.Artifact, error) {
	kind, err := parseArtifactKind(req.Kind)
	if err != nil {
		return domain.Artifact{}, err
	}
	root, err := store.DataDir()
	if err != nil {
		return domain.Artifact{}, err
	}
	dir, err := store.ArtifactsDir(root, workspaceID)
	if err != nil {
		return domain.Artifact{}, err
	}
	// UUID filenames avoid silent overwrite collisions on same-title creates.
	id := uuid.NewString()
	ext := artifactExt[kind]
	filename := id + ext
	full := filepath.Join(dir, filename)
	data := []byte(req.Content)
	if kind == domain.ArtifactPNG || kind == domain.ArtifactJPEG {
		decoded, derr := base64.StdEncoding.DecodeString(strings.TrimSpace(req.Content))
		if derr != nil {
			return domain.Artifact{}, fmt.Errorf("png/jpeg content must be base64: %w", derr)
		}
		data = decoded
	}
	if err := os.WriteFile(full, data, 0o644); err != nil {
		return domain.Artifact{}, err
	}
	a := domain.Artifact{
		ID:          id,
		WorkspaceID: workspaceID,
		Title:       strings.TrimSpace(req.Title),
		Kind:        kind,
		Path:        filepath.ToSlash(filepath.Join("workspaces", workspaceID, "artifacts", filename)),
		CreatedAt:   time.Now().UTC().Format(time.RFC3339Nano),
		CreatedBy:   agentCode,
	}
	metaPath := filepath.Join(dir, id+".meta.json")
	if b, err := json.MarshalIndent(a, "", "  "); err == nil {
		_ = os.WriteFile(metaPath, b, 0o644)
	}
	e.Bus.Publish(Event{Name: "artifact.created", Data: map[string]any{
		"workspaceId": workspaceID,
		"artifact":    a,
	}})
	return a, nil
}

func (r engineReporter) ListArtifacts(workspaceID, agentCode string) ([]domain.Artifact, error) {
	_ = agentCode
	return r.eng.ListArtifacts(workspaceID)
}

func (e *Engine) ListArtifacts(workspaceID string) ([]domain.Artifact, error) {
	return listArtifacts(workspaceID)
}

// hoistWorkspaceArtifacts moves host-owned artifact files onto dest and rewrites
// metadata paths. Dest wins on id collision so a live workspace is not overwritten.
func (e *Engine) hoistWorkspaceArtifacts(fromID, toID string, remap map[string]string) error {
	fromID, toID = strings.TrimSpace(fromID), strings.TrimSpace(toID)
	if fromID == "" || toID == "" || fromID == toID {
		return nil
	}
	list, err := listArtifacts(fromID)
	if err != nil {
		return err
	}
	if len(list) == 0 {
		return nil
	}
	root, err := store.DataDir()
	if err != nil {
		return err
	}
	destDir, err := store.ArtifactsDir(root, toID)
	if err != nil {
		return err
	}
	srcDir := filepath.Join(root, "workspaces", fromID, "artifacts")
	for _, a := range list {
		filename := a.Path
		if i := strings.LastIndex(filename, "/"); i >= 0 {
			filename = filename[i+1:]
		}
		if filename == "" {
			ext := artifactExt[a.Kind]
			if ext == "" {
				continue
			}
			filename = a.ID + ext
		}
		destFile := filepath.Join(destDir, filename)
		destMeta := filepath.Join(destDir, a.ID+".meta.json")
		if _, err := os.Stat(destMeta); err == nil {
			continue
		}
		if _, err := os.Stat(destFile); err == nil {
			continue
		}
		srcFile := filepath.Join(srcDir, filename)
		if err := moveArtifactFile(srcFile, destFile); err != nil {
			return fmt.Errorf("hoist artifact %s: %w", a.ID, err)
		}
		a.WorkspaceID = toID
		a.Path = filepath.ToSlash(filepath.Join("workspaces", toID, "artifacts", filename))
		if next := remap[a.CreatedBy]; next != "" {
			a.CreatedBy = next
		}
		srcMeta := filepath.Join(srcDir, a.ID+".meta.json")
		if err := commitHoistedArtifact(srcFile, destFile, srcMeta, destMeta, a); err != nil {
			return fmt.Errorf("hoist artifact %s: %w", a.ID, err)
		}
		if e != nil {
			e.Bus.Publish(Event{Name: "artifact.created", Data: map[string]any{
				"workspaceId": toID,
				"artifact":    a,
			}})
		}
	}
	return nil
}

// commitHoistedArtifact writes dest metadata then drops source metadata.
// A metadata write failure rolls the content file back onto srcFile.
func commitHoistedArtifact(srcFile, destFile, srcMeta, destMeta string, a domain.Artifact) error {
	b, err := json.MarshalIndent(a, "", "  ")
	if err != nil {
		_ = moveArtifactFile(destFile, srcFile)
		return err
	}
	if err := os.WriteFile(destMeta, b, 0o644); err != nil {
		_ = moveArtifactFile(destFile, srcFile)
		return err
	}
	_ = os.Remove(srcMeta)
	return nil
}

func moveArtifactFile(src, dst string) error {
	if err := os.Rename(src, dst); err == nil {
		return nil
	}
	data, err := os.ReadFile(src)
	if err != nil {
		return err
	}
	if err := os.WriteFile(dst, data, 0o644); err != nil {
		return err
	}
	return os.Remove(src)
}

func listArtifacts(workspaceID string) ([]domain.Artifact, error) {
	if workspaceID == "" {
		return nil, nil
	}
	root, err := store.DataDir()
	if err != nil {
		return nil, err
	}
	// Read-only: do not MkdirAll. Listing a workspace that never created
	// artifacts must not leave workspaces/<id>/artifacts/ behind.
	dir := filepath.Join(root, "workspaces", workspaceID, "artifacts")
	entries, err := os.ReadDir(dir)
	if err != nil {
		if os.IsNotExist(err) {
			return nil, nil
		}
		return nil, err
	}
	out := make([]domain.Artifact, 0)
	for _, ent := range entries {
		if ent.IsDir() || !strings.HasSuffix(ent.Name(), ".meta.json") {
			continue
		}
		b, err := os.ReadFile(filepath.Join(dir, ent.Name()))
		if err != nil {
			continue
		}
		var a domain.Artifact
		if json.Unmarshal(b, &a) != nil {
			continue
		}
		out = append(out, a)
	}
	sort.Slice(out, func(i, j int) bool {
		ti, tj := parseArtifactTime(out[i].CreatedAt), parseArtifactTime(out[j].CreatedAt)
		if !ti.Equal(tj) {
			return ti.After(tj)
		}
		return out[i].ID > out[j].ID
	})
	return out, nil
}

func parseArtifactTime(s string) time.Time {
	t, err := time.Parse(time.RFC3339, strings.TrimSpace(s))
	if err != nil {
		return time.Time{}
	}
	return t
}

func (r engineReporter) PresentArtifact(workspaceID, agentCode, artifactID string) string {
	_ = agentCode
	return r.eng.PresentArtifact(workspaceID, artifactID)
}

func (e *Engine) PresentArtifact(workspaceID, artifactID string) string {
	list, err := e.ListArtifacts(workspaceID)
	if err != nil {
		return "Could not list artifacts: " + err.Error()
	}
	for _, a := range list {
		if a.ID == artifactID {
			e.Bus.Publish(Event{Name: "artifact.presented", Data: map[string]any{
				"workspaceId": workspaceID,
				"artifact":    a,
			}})
			return "Presented artifact " + a.ID + " (" + a.Title + ")."
		}
	}
	return "Artifact not found: " + artifactID
}

func (r engineReporter) ReadArtifact(workspaceID, agentCode, artifactID string) (string, error) {
	_ = agentCode
	return r.eng.ReadArtifact(workspaceID, artifactID)
}

func (e *Engine) ReadArtifact(workspaceID, artifactID string) (string, error) {
	artifactID = strings.TrimSpace(artifactID)
	if artifactID == "" {
		return "", fmt.Errorf("artifact id is required")
	}
	list, err := e.ListArtifacts(workspaceID)
	if err != nil {
		return "", err
	}
	for _, a := range list {
		if a.ID != artifactID {
			continue
		}
		out := map[string]any{
			"id": a.ID, "title": a.Title, "kind": a.Kind, "path": a.Path,
		}
		if artifactKindIsBinary(a.Kind) {
			out["binary"] = true
		} else {
			body, rerr := readArtifactTextLimit(a, 0)
			if rerr != nil {
				return "", rerr
			}
			out["content"] = body
		}
		b, err := json.Marshal(out)
		if err != nil {
			return "", err
		}
		return string(b), nil
	}
	return "", fmt.Errorf("artifact not found: %s", artifactID)
}

func findArtifact(workspaceID, artifactID string) (domain.Artifact, error) {
	artifactID = strings.TrimSpace(artifactID)
	list, err := listArtifacts(workspaceID)
	if err != nil {
		return domain.Artifact{}, err
	}
	for _, a := range list {
		if a.ID == artifactID && artifactID != "" {
			return a, nil
		}
	}
	return domain.Artifact{}, fmt.Errorf("artifact not found: %s", artifactID)
}

func artifactMetaPath(a domain.Artifact) (string, error) {
	full, err := artifactAbsPath(a)
	if err != nil {
		return "", err
	}
	return filepath.Join(filepath.Dir(full), a.ID+".meta.json"), nil
}

func (e *Engine) RenameArtifact(workspaceID, artifactID, title string) (domain.Artifact, error) {
	title = strings.TrimSpace(title)
	if title == "" {
		return domain.Artifact{}, fmt.Errorf("title is required")
	}
	a, err := findArtifact(workspaceID, artifactID)
	if err != nil {
		return domain.Artifact{}, err
	}
	meta, err := artifactMetaPath(a)
	if err != nil {
		return domain.Artifact{}, err
	}
	a.Title = title
	b, err := json.MarshalIndent(a, "", "  ")
	if err != nil {
		return domain.Artifact{}, err
	}
	if err := os.WriteFile(meta, b, 0o644); err != nil {
		return domain.Artifact{}, err
	}
	e.Bus.Publish(Event{Name: "artifact.created", Data: map[string]any{
		"workspaceId": workspaceID,
		"artifact":    a,
	}})
	return a, nil
}

func (e *Engine) DeleteArtifact(workspaceID, artifactID string) error {
	a, err := findArtifact(workspaceID, artifactID)
	if err != nil {
		return err
	}
	full, err := artifactAbsPath(a)
	if err != nil {
		return err
	}
	meta, err := artifactMetaPath(a)
	if err != nil {
		return err
	}
	if err := os.Remove(meta); err != nil {
		return err
	}
	if err := os.Remove(full); err != nil && !os.IsNotExist(err) {
		return err
	}
	return nil
}

func (e *Engine) ArtifactAbsPath(workspaceID, artifactID string) (string, error) {
	a, err := findArtifact(workspaceID, artifactID)
	if err != nil {
		return "", err
	}
	return artifactAbsPath(a)
}
