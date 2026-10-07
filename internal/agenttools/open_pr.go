package agenttools

import (
	"encoding/json"
	"fmt"
	"strings"
)

var schemaOpenPR = json.RawMessage(`{
  "type": "object",
  "properties": {
    "title": {"type": "string", "description": "PR title, under 80 characters"},
    "body": {"type": "string", "description": "Markdown description of ALL workspace changes versus the base branch"},
    "draft": {"type": "boolean", "description": "Open as a GitHub draft PR (gh pr create --draft). Ignored if a PR is already open on GitHub."}
  },
  "required": ["title", "body"]
}`)

type openPrTool struct{}

func (openPrTool) Name() string { return "open_pr" }

func (openPrTool) Description() string {
	return "Open a pull request from this workspace. Recur pushes the branch and creates the GitHub " +
		"PR when the project opts in (otherwise a Local PR). Do not run `gh pr create` yourself. " +
		"Provide a title under 80 characters and a markdown body describing ALL workspace changes " +
		"versus the base branch — not only this chat turn. Set draft to true for a GitHub draft " +
		"PR. Commit uncommitted work first. If a GitHub PR is already open, this pushes new " +
		"commits and does not rewrite the description. If only a Local PR is open, title and " +
		"body update that Local PR and retry GitHub create when the project opts in. Refused " +
		"on the main working copy and while a loop is running. After a loop idles (including " +
		"Review & fix, Docs, and Security review & fix), open a PR from here or the titlebar " +
		"— the run does not self-open one."
}

func (openPrTool) Schema() json.RawMessage { return schemaOpenPR }

func (openPrTool) Execute(c Call) (string, error) {
	h, err := requireHost(c)
	if err != nil {
		return "", err
	}

	var in struct {
		Title string `json:"title"`
		Body  string `json:"body"`
		Draft bool   `json:"draft"`
	}
	if err := strictUnmarshal(c.Args, &in); err != nil {
		return "", err
	}
	if strings.TrimSpace(in.Title) == "" {
		return "", fmt.Errorf("open_pr: title is required")
	}
	if strings.TrimSpace(in.Body) == "" {
		return "", fmt.Errorf("open_pr: body is required")
	}
	return h.OpenPR(c.WorkspaceID, c.AgentCode, in.Title, in.Body, in.Draft), nil
}
