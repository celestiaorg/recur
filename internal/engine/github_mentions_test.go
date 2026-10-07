package engine

import (
	"os"
	"path/filepath"
	"slices"
	"strings"
	"testing"

	"recur/internal/domain"
)

func TestParseMentionableUsers(t *testing.T) {
	t.Parallel()
	nodes := `{"data":{"repository":{"mentionableUsers":{"nodes":[{"login":"octocat","name":"The Octocat","avatarUrl":"https://avatars.githubusercontent.com/u/1"},{"login":"","name":"No Login"},{"login":"hubot","name":"Hubot","avatarUrl":""}]}}}}`
	tests := []struct {
		raw  string
		want []domain.PRParticipant
	}{
		{
			raw: nodes,
			want: []domain.PRParticipant{
				{Login: "octocat", Name: "The Octocat", AvatarURL: "https://avatars.githubusercontent.com/u/1"},
				{Login: "hubot", Name: "Hubot", AvatarURL: ghAvatarURL("hubot", "")},
			},
		},
		{raw: "", want: nil},
		{raw: "{", want: nil},
	}
	for _, tt := range tests {
		got := parseMentionableUsers(tt.raw)
		if !slices.Equal(got, tt.want) {
			t.Errorf("parseMentionableUsers(%q) = %v, want %v", tt.raw, got, tt.want)
		}
	}
}

func TestSearchPRMentionableUsers(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	if got := eng.SearchPRMentionableUsers("missing", "octo"); got != nil {
		t.Errorf("SearchPRMentionableUsers(missing, octo) = %v, want %v", got, []domain.PRParticipant(nil))
	}

	local, err := st.AddPR(domain.PR{
		ID: "local-1", Source: domain.SourceLocal, URL: "https://github.com/acme/app/pull/1",
	})
	if err != nil {
		t.Fatalf("AddPR(local): %v", err)
	}
	if got := eng.SearchPRMentionableUsers(local.ID, "octo"); got != nil {
		t.Errorf("SearchPRMentionableUsers(%s, octo) = %v, want %v", local.ID, got, []domain.PRParticipant(nil))
	}

	noURL, err := st.AddPR(domain.PR{ID: "gh-nourl", Source: domain.SourceGitHub, URL: "not-a-pr"})
	if err != nil {
		t.Fatalf("AddPR(no URL): %v", err)
	}
	if got := eng.SearchPRMentionableUsers(noURL.ID, "octo"); got != nil {
		t.Errorf("SearchPRMentionableUsers(%s, octo) = %v, want %v", noURL.ID, got, []domain.PRParticipant(nil))
	}
}

func TestSearchPRMentionableUsersPassesStringGraphQLFields(t *testing.T) {
	dir := installFakeGH(t, `#!/bin/sh
printf '%s\0' "$@" >> "$0.args"
echo '{"data":{"repository":{"mentionableUsers":{"nodes":[]}}}}'
`)
	eng, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	pr, err := st.AddPR(domain.PR{
		ID: "gh-typed", Source: domain.SourceGitHub, URL: "https://github.com/123/true/pull/1",
	})
	if err != nil {
		t.Fatalf("AddPR: %v", err)
	}
	for _, q := range []string{"123", "true", "false", "null"} {
		eng.SearchPRMentionableUsers(pr.ID, q)
	}
	logged, err := os.ReadFile(filepath.Join(dir, "gh.args"))
	if err != nil {
		t.Fatalf("read fake gh args: %v", err)
	}
	args := strings.Split(strings.TrimSuffix(string(logged), "\x00"), "\x00")
	var raw, typed []string
	for i := 0; i < len(args)-1; i++ {
		switch args[i] {
		case "-f":
			raw = append(raw, args[i+1])
			i++
		case "-F":
			typed = append(typed, args[i+1])
			i++
		}
	}
	for _, want := range []string{"o=123", "r=true", "q=123", "q=true", "q=false", "q=null"} {
		if !slices.Contains(raw, want) {
			t.Errorf("-f %s missing; -f=%v -F=%v", want, raw, typed)
		}
	}
	for _, field := range typed {
		if strings.HasPrefix(field, "o=") || strings.HasPrefix(field, "r=") || strings.HasPrefix(field, "q=") {
			t.Errorf("typed -F %s; want -f for String variables (args=%q)", field, logged)
		}
	}
}
