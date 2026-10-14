package app

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"log"
	"net/http"
	"net/url"
	"os"
	"os/exec"
	"runtime"
	"sort"
	"strings"
	"sync"
	"time"

	"recur/internal/domain"
	"recur/internal/engine"
)

const linearGraphQLURL = "https://api.linear.app/graphql"

const linearIssueFields = `id identifier title url createdAt updatedAt priority priorityLabel assignee { name displayName avatarUrl } state { name type color position } labels(first: 10) { nodes { name color } } project { name } team { key name }`

const linearOpenFilter = `state: {type: {nin: ["completed", "canceled", "duplicate"]}}`

const linearAssignedQuery = `query { viewer { assignedIssues(first: 100, filter: {` + linearOpenFilter + `}) { nodes { ` + linearIssueFields + ` } } } }`

const linearTeamsQuery = `query { viewer { teams(first: 50) { nodes { id } } } }`

const linearUnassignedQuery = `query($teams: [ID!], $after: String) { issues(first: 100, after: $after, filter: {team: {id: {in: $teams}}, assignee: {null: true}, ` + linearOpenFilter + `}) { nodes { ` + linearIssueFields + ` } pageInfo { hasNextPage endCursor } } }`

// simplify: unassigned Linear issues stop at 300 (3 pages). Raise it, or page
// lazily from the view, if teams carry bigger unassigned backlogs.
const linearUnassignedMax = 300

// AssignedIssue is one open issue that is assigned to the user or unassigned
// in their repos / Linear teams, from GitHub or Linear.
type AssignedIssue struct {
	Source        string                 `json:"source"` // "github" | "linear"
	ID            string                 `json:"id"`     // stable tracker id; matches Workspace.Issue.ID
	Ref           string                 `json:"ref"`    // owner/repo#12 or ENG-123
	Title         string                 `json:"title"`
	URL           string                 `json:"url"`
	CreatedAt     string                 `json:"createdAt"`
	UpdatedAt     string                 `json:"updatedAt"`
	Group         string                 `json:"group"` // "progress" | "todo" | "backlog"
	State         string                 `json:"state"`
	StateColor    string                 `json:"stateColor"`
	StatePosition float64                `json:"statePosition"` // Linear workflow order within a team
	Priority      int                    `json:"priority"`      // Linear: 0 none, 1 urgent … 4 low
	PriorityLabel string                 `json:"priorityLabel"`
	Project       string                 `json:"project"` // owner/repo, or Linear project / team
	Labels        []IssueLabel           `json:"labels"`
	Comments      int                    `json:"comments"`
	Assignees     []domain.PRParticipant `json:"assignees"`
	// ProjectID is the Recur project this issue belongs to, or "" when no
	// project's git remotes (GitHub) or LinearProjects setting (Linear) match.
	ProjectID     string `json:"projectId"`
	LinearProject string `json:"linearProject"`
	LinearTeam    string `json:"linearTeam"` // team key, e.g. ENG
}

// IssueLabel is a label chip; Color is a hex string with or without '#'.
type IssueLabel struct {
	Name  string `json:"name"`
	Color string `json:"color"`
}

// ListAssignedIssues merges open GitHub issues (via `gh`) and Linear issues
// (via the configured API key) that are assigned to the user, plus unassigned
// ones in the user's project repos and Linear teams, newest first. A source
// that is unavailable or fails is skipped; the others still show.
func (s *SettingsService) ListAssignedIssues() []AssignedIssue {
	ctx, cancel := context.WithTimeout(context.Background(), 15*time.Second)
	defer cancel()
	projects := s.st().Projects()
	repos := githubRemoteRepos(ctx, projects)
	key := linearKey(s.st().Settings())

	var (
		mu  sync.Mutex
		wg  sync.WaitGroup
		out []AssignedIssue
	)
	run := func(fetch func() []AssignedIssue) {
		wg.Add(1)
		go func() {
			defer wg.Done()
			got := fetch()
			mu.Lock()
			out = append(out, got...)
			mu.Unlock()
		}()
	}
	run(func() []AssignedIssue { return githubIssues(ctx, "--assignee", "@me") })
	// GitHub search caps the query at 256 chars, so repo: qualifiers go in batches.
	names := make([]string, 0, len(repos))
	for r := range repos {
		names = append(names, r)
	}
	sort.Strings(names)
	for i := 0; i < len(names); i += 5 {
		args := []string{"--no-assignee"}
		for _, r := range names[i:min(i+5, len(names))] {
			args = append(args, "--repo", r)
		}
		run(func() []AssignedIssue { return githubIssues(ctx, args...) })
	}
	if key != "" {
		run(func() []AssignedIssue { return linearAssignedIssues(ctx, key) })
		run(func() []AssignedIssue { return linearUnassignedIssues(ctx, key) })
	}
	wg.Wait()

	linkIssueProjects(out, projects, repos)
	sort.SliceStable(out, func(i, j int) bool { return out[i].UpdatedAt > out[j].UpdatedAt })
	return out
}

func githubIssues(ctx context.Context, filter ...string) []AssignedIssue {
	if _, err := exec.LookPath("gh"); err != nil {
		return nil
	}
	args := append([]string{"search", "issues", "--state", "open", "--limit", "100",
		"--json", "id,number,title,url,createdAt,updatedAt,repository,labels,commentsCount,assignees"}, filter...)
	raw, err := exec.CommandContext(ctx, "gh", args...).Output()
	if err != nil {
		return nil
	}
	var rows []struct {
		ID            string       `json:"id"`
		Number        int          `json:"number"`
		Title         string       `json:"title"`
		URL           string       `json:"url"`
		CreatedAt     string       `json:"createdAt"`
		UpdatedAt     string       `json:"updatedAt"`
		CommentsCount int          `json:"commentsCount"`
		Labels        []IssueLabel `json:"labels"`
		Assignees     []struct {
			Login string `json:"login"`
		} `json:"assignees"`
		Repository struct {
			NameWithOwner string `json:"nameWithOwner"`
		} `json:"repository"`
	}
	if json.Unmarshal(raw, &rows) != nil {
		return nil
	}
	out := make([]AssignedIssue, 0, len(rows))
	for _, r := range rows {
		var people []domain.PRParticipant
		for _, a := range r.Assignees {
			people = append(people, domain.PRParticipant{Login: a.Login})
		}
		out = append(out, AssignedIssue{
			Source: "github", ID: r.ID, Ref: fmt.Sprintf("%s#%d", r.Repository.NameWithOwner, r.Number),
			Title: r.Title, URL: r.URL, CreatedAt: r.CreatedAt, UpdatedAt: r.UpdatedAt,
			Group: "todo", State: "Open", Project: r.Repository.NameWithOwner,
			Labels: r.Labels, Comments: r.CommentsCount, Assignees: people,
		})
	}
	return out
}

type linearIssueNode struct {
	ID            string `json:"id"`
	Identifier    string `json:"identifier"`
	Title         string `json:"title"`
	URL           string `json:"url"`
	CreatedAt     string `json:"createdAt"`
	UpdatedAt     string `json:"updatedAt"`
	Priority      int    `json:"priority"`
	PriorityLabel string `json:"priorityLabel"`
	Assignee      *struct {
		Name        string `json:"name"`
		DisplayName string `json:"displayName"`
		AvatarURL   string `json:"avatarUrl"`
	} `json:"assignee"`
	State struct {
		Name     string  `json:"name"`
		Type     string  `json:"type"`
		Color    string  `json:"color"`
		Position float64 `json:"position"`
	} `json:"state"`
	Labels struct {
		Nodes []IssueLabel `json:"nodes"`
	} `json:"labels"`
	Project *struct {
		Name string `json:"name"`
	} `json:"project"`
	Team struct {
		Key  string `json:"key"`
		Name string `json:"name"`
	} `json:"team"`
}

type linearIssueNodes struct {
	Nodes []linearIssueNode `json:"nodes"`
}

func linearAssignedIssues(ctx context.Context, key string) []AssignedIssue {
	var data struct {
		Viewer struct {
			AssignedIssues linearIssueNodes `json:"assignedIssues"`
		} `json:"viewer"`
	}
	if err := linearGraphQL(ctx, key, linearAssignedQuery, nil, &data); err != nil {
		log.Printf("recur: linear assigned issues: %v", err)
		return nil
	}
	return linearToIssues(data.Viewer.AssignedIssues.Nodes)
}

// linearUnassignedIssues lists open unassigned issues in the user's teams,
// one flat paged query (nesting issues under teams multiplies Linear's
// query-complexity cost).
func linearUnassignedIssues(ctx context.Context, key string) []AssignedIssue {
	var teams struct {
		Viewer struct {
			Teams struct {
				Nodes []struct {
					ID string `json:"id"`
				} `json:"nodes"`
			} `json:"teams"`
		} `json:"viewer"`
	}
	if err := linearGraphQL(ctx, key, linearTeamsQuery, nil, &teams); err != nil {
		log.Printf("recur: linear teams: %v", err)
		return nil
	}
	ids := make([]string, 0, len(teams.Viewer.Teams.Nodes))
	for _, t := range teams.Viewer.Teams.Nodes {
		ids = append(ids, t.ID)
	}
	if len(ids) == 0 {
		return nil
	}
	var nodes []linearIssueNode
	after := ""
	for len(nodes) < linearUnassignedMax {
		vars := map[string]any{"teams": ids}
		if after != "" {
			vars["after"] = after
		}
		var page struct {
			Issues struct {
				Nodes    []linearIssueNode `json:"nodes"`
				PageInfo struct {
					HasNextPage bool   `json:"hasNextPage"`
					EndCursor   string `json:"endCursor"`
				} `json:"pageInfo"`
			} `json:"issues"`
		}
		if err := linearGraphQL(ctx, key, linearUnassignedQuery, vars, &page); err != nil {
			log.Printf("recur: linear unassigned issues: %v", err)
			break
		}
		nodes = append(nodes, page.Issues.Nodes...)
		if !page.Issues.PageInfo.HasNextPage || page.Issues.PageInfo.EndCursor == "" {
			break
		}
		after = page.Issues.PageInfo.EndCursor
	}
	return linearToIssues(nodes)
}

func linearToIssues(nodes []linearIssueNode) []AssignedIssue {
	out := make([]AssignedIssue, 0, len(nodes))
	for _, n := range nodes {
		project, linearProject := n.Team.Name, ""
		if n.Project != nil && n.Project.Name != "" {
			project, linearProject = n.Project.Name, n.Project.Name
		}
		var people []domain.PRParticipant
		if a := n.Assignee; a != nil {
			name := a.DisplayName
			if name == "" {
				name = a.Name
			}
			people = []domain.PRParticipant{{Name: name, AvatarURL: a.AvatarURL}}
		}
		out = append(out, AssignedIssue{
			Source: "linear", ID: n.ID, Ref: n.Identifier, Title: n.Title, URL: n.URL, CreatedAt: n.CreatedAt, UpdatedAt: n.UpdatedAt,
			Group: linearGroup(n.State.Type), State: n.State.Name, StateColor: n.State.Color, StatePosition: n.State.Position,
			Priority: n.Priority, PriorityLabel: n.PriorityLabel, Project: project, Labels: n.Labels.Nodes, Assignees: people,
			LinearProject: linearProject, LinearTeam: n.Team.Key,
		})
	}
	return out
}

// linearGraphQL posts one query and decodes its data into out.
func linearGraphQL(ctx context.Context, key, query string, vars map[string]any, out any) error {
	body, _ := json.Marshal(map[string]any{"query": query, "variables": vars})
	req, err := http.NewRequestWithContext(ctx, http.MethodPost, linearGraphQLURL, bytes.NewReader(body))
	if err != nil {
		return err
	}
	req.Header.Set("Authorization", key)
	req.Header.Set("Content-Type", "application/json")
	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		return err
	}
	defer resp.Body.Close()
	var res struct {
		Data   json.RawMessage `json:"data"`
		Errors []struct {
			Message string `json:"message"`
		} `json:"errors"`
	}
	if err := json.NewDecoder(resp.Body).Decode(&res); err != nil {
		return fmt.Errorf("HTTP %d: %w", resp.StatusCode, err)
	}
	if len(res.Errors) > 0 {
		return fmt.Errorf("%s", res.Errors[0].Message)
	}
	if resp.StatusCode != http.StatusOK {
		return fmt.Errorf("HTTP %d", resp.StatusCode)
	}
	return json.Unmarshal(res.Data, out)
}

func linearKey(set domain.Settings) string {
	if key := set.BundleValue(domain.SettingLinearAPIKey); key != "" {
		return key
	}
	return bundleEnvLinearKey()
}

// claimIssue assigns an unassigned issue to the user when a workspace links
// to it. Issues someone else already owns are left alone. Best effort.
func claimIssue(issue domain.IssueRef, set domain.Settings) {
	ctx, cancel := context.WithTimeout(context.Background(), 20*time.Second)
	defer cancel()
	var err error
	switch issue.Source {
	case "github":
		err = claimGitHubIssue(ctx, issue.Key)
	case "linear":
		if key := linearKey(set); key != "" {
			err = claimLinearIssue(ctx, key, issue.ID)
		}
	}
	if err != nil {
		log.Printf("recur: assign %s issue %s: %v", issue.Source, issue.Key, err)
	}
}

func claimGitHubIssue(ctx context.Context, key string) error {
	i := strings.LastIndex(key, "#")
	if i <= 0 {
		return fmt.Errorf("issue key %q has no owner/repo", key)
	}
	repo, num := key[:i], key[i+1:]
	raw, err := exec.CommandContext(ctx, "gh", "issue", "view", num, "--repo", repo, "--json", "assignees").Output()
	if err != nil {
		return err
	}
	var v struct {
		Assignees []json.RawMessage `json:"assignees"`
	}
	if err := json.Unmarshal(raw, &v); err != nil {
		return err
	}
	if len(v.Assignees) > 0 {
		return nil
	}
	return exec.CommandContext(ctx, "gh", "issue", "edit", num, "--repo", repo, "--add-assignee", "@me").Run()
}

func claimLinearIssue(ctx context.Context, key, id string) error {
	var data struct {
		Viewer struct {
			ID string `json:"id"`
		} `json:"viewer"`
		Issue struct {
			Assignee *struct {
				ID string `json:"id"`
			} `json:"assignee"`
		} `json:"issue"`
	}
	q := `query($id: String!) { viewer { id } issue(id: $id) { assignee { id } } }`
	if err := linearGraphQL(ctx, key, q, map[string]any{"id": id}, &data); err != nil {
		return err
	}
	if data.Issue.Assignee != nil || data.Viewer.ID == "" {
		return nil
	}
	m := `mutation($id: String!, $a: String!) { issueUpdate(id: $id, input: {assigneeId: $a}) { success } }`
	return linearGraphQL(ctx, key, m, map[string]any{"id": id, "a": data.Viewer.ID}, &struct{}{})
}

// githubRemoteRepos maps lowercased owner/repo of every GitHub remote
// (origin, upstream, …) to the project that has it, so fork checkouts still
// claim upstream issues.
func githubRemoteRepos(ctx context.Context, projects []domain.Project) map[string]string {
	out := map[string]string{}
	for _, p := range projects {
		if p.RepoPath == "" {
			continue
		}
		raw, err := exec.CommandContext(ctx, "git", "-C", p.RepoPath, "remote", "-v").Output()
		if err != nil {
			continue
		}
		for _, line := range strings.Split(string(raw), "\n") {
			f := strings.Fields(line)
			if len(f) < 2 {
				continue
			}
			if repo, ok := engine.OwnerRepoFromRemoteURL(f[1]); ok {
				if _, taken := out[strings.ToLower(repo)]; !taken {
					out[strings.ToLower(repo)] = p.ID
				}
			}
		}
	}
	return out
}

// linkIssueProjects sets ProjectID on each issue. Linear issues match a
// project's LinearProjects entries by Linear project name first, then team key.
func linkIssueProjects(issues []AssignedIssue, projects []domain.Project, repos map[string]string) {
	byLinear := map[string]string{}
	for _, p := range projects {
		for _, tok := range strings.Split(p.LinearProjects, ",") {
			if tok = strings.ToLower(strings.TrimSpace(tok)); tok != "" {
				if _, taken := byLinear[tok]; !taken {
					byLinear[tok] = p.ID
				}
			}
		}
	}
	for i := range issues {
		it := &issues[i]
		switch it.Source {
		case "github":
			it.ProjectID = repos[strings.ToLower(it.Project)]
		case "linear":
			if id, ok := byLinear[strings.ToLower(it.LinearProject)]; ok && it.LinearProject != "" {
				it.ProjectID = id
			} else if it.LinearTeam != "" {
				it.ProjectID = byLinear[strings.ToLower(it.LinearTeam)]
			}
		}
	}
}

// OpenLinearInApp opens a linear.app URL in the Linear desktop app. False
// means the caller should open it in the browser instead.
// simplify: macOS only (open fails when no app handles linear://). Add an
// xdg-mime handler check for Linux if Linear ships a desktop build there.
func (s *SettingsService) OpenLinearInApp(raw string) bool {
	appURL, ok := linearAppURL(raw)
	if !ok || runtime.GOOS != "darwin" {
		return false
	}
	return exec.Command("open", appURL).Run() == nil
}

func linearAppURL(raw string) (string, bool) {
	u, err := url.Parse(strings.TrimSpace(raw))
	if err != nil || u.Scheme != "https" || u.Host != "linear.app" {
		return "", false
	}
	return "linear:/" + u.RequestURI(), true
}

func linearGroup(stateType string) string {
	switch stateType {
	case "started":
		return "progress"
	case "backlog", "triage":
		return "backlog"
	}
	return "todo"
}

func bundleEnvLinearKey() string {
	return strings.TrimSpace(os.Getenv("LINEAR_API_KEY"))
}
