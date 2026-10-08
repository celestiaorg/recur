package app

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"net/http"
	"os"
	"os/exec"
	"sort"
	"strings"
	"time"

	"recur/internal/domain"
)

const linearGraphQLURL = "https://api.linear.app/graphql"

const linearAssignedQuery = `query { viewer { assignedIssues(first: 50, filter: {state: {type: {nin: ["completed", "canceled"]}}}) { nodes { identifier title url updatedAt team { key } } } } }`

// AssignedIssue is one open issue assigned to the user, from GitHub or Linear.
type AssignedIssue struct {
	Source    string `json:"source"` // "github" | "linear"
	Ref       string `json:"ref"`    // owner/repo#12 or ENG-123
	Title     string `json:"title"`
	URL       string `json:"url"`
	UpdatedAt string `json:"updatedAt"`
}

// ListAssignedIssues merges open GitHub issues (via `gh`) and Linear issues
// (via the configured API key) assigned to the user, newest first. A source
// that is unavailable or fails is skipped; the other still shows.
func (s *SettingsService) ListAssignedIssues() []AssignedIssue {
	ctx, cancel := context.WithTimeout(context.Background(), 15*time.Second)
	defer cancel()
	out := append(githubAssignedIssues(ctx), linearAssignedIssues(ctx, s.st().Settings())...)
	sort.SliceStable(out, func(i, j int) bool { return out[i].UpdatedAt > out[j].UpdatedAt })
	return out
}

func githubAssignedIssues(ctx context.Context) []AssignedIssue {
	if _, err := exec.LookPath("gh"); err != nil {
		return nil
	}
	raw, err := exec.CommandContext(ctx, "gh", "search", "issues", "--assignee", "@me", "--state", "open",
		"--limit", "50", "--json", "number,title,url,updatedAt,repository").Output()
	if err != nil {
		return nil
	}
	var rows []struct {
		Number     int    `json:"number"`
		Title      string `json:"title"`
		URL        string `json:"url"`
		UpdatedAt  string `json:"updatedAt"`
		Repository struct {
			NameWithOwner string `json:"nameWithOwner"`
		} `json:"repository"`
	}
	if json.Unmarshal(raw, &rows) != nil {
		return nil
	}
	out := make([]AssignedIssue, 0, len(rows))
	for _, r := range rows {
		out = append(out, AssignedIssue{
			Source: "github", Ref: fmt.Sprintf("%s#%d", r.Repository.NameWithOwner, r.Number),
			Title: r.Title, URL: r.URL, UpdatedAt: r.UpdatedAt,
		})
	}
	return out
}

func linearAssignedIssues(ctx context.Context, set domain.Settings) []AssignedIssue {
	key := set.BundleValue(domain.SettingLinearAPIKey)
	if key == "" {
		key = bundleEnvLinearKey()
	}
	if key == "" {
		return nil
	}
	body, _ := json.Marshal(map[string]string{"query": linearAssignedQuery})
	req, err := http.NewRequestWithContext(ctx, http.MethodPost, linearGraphQLURL, bytes.NewReader(body))
	if err != nil {
		return nil
	}
	req.Header.Set("Authorization", key)
	req.Header.Set("Content-Type", "application/json")
	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		return nil
	}
	defer resp.Body.Close()
	var res struct {
		Data struct {
			Viewer struct {
				AssignedIssues struct {
					Nodes []struct {
						Identifier string `json:"identifier"`
						Title      string `json:"title"`
						URL        string `json:"url"`
						UpdatedAt  string `json:"updatedAt"`
					} `json:"nodes"`
				} `json:"assignedIssues"`
			} `json:"viewer"`
		} `json:"data"`
	}
	if resp.StatusCode != http.StatusOK || json.NewDecoder(resp.Body).Decode(&res) != nil {
		return nil
	}
	nodes := res.Data.Viewer.AssignedIssues.Nodes
	out := make([]AssignedIssue, 0, len(nodes))
	for _, n := range nodes {
		out = append(out, AssignedIssue{Source: "linear", Ref: n.Identifier, Title: n.Title, URL: n.URL, UpdatedAt: n.UpdatedAt})
	}
	return out
}

func bundleEnvLinearKey() string {
	return strings.TrimSpace(os.Getenv("LINEAR_API_KEY"))
}
