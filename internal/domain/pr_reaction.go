package domain

import (
	"sort"
	"strings"
)

// GitHub conversation reaction contents, in GitHub's picker order.
var reactionContents = []string{"+1", "-1", "laugh", "hooray", "confused", "heart", "rocket", "eyes"}

var graphqlReaction = map[string]string{
	"+1":       "THUMBS_UP",
	"-1":       "THUMBS_DOWN",
	"laugh":    "LAUGH",
	"hooray":   "HOORAY",
	"confused": "CONFUSED",
	"heart":    "HEART",
	"rocket":   "ROCKET",
	"eyes":     "EYES",
}

var reactionAliases = map[string]string{
	"THUMBS_UP":   "+1",
	"THUMBS_DOWN": "-1",
}

func init() {
	for _, c := range reactionContents {
		reactionAliases[c] = c
		reactionAliases[strings.ToUpper(c)] = c
	}
	for rest, gql := range graphqlReaction {
		reactionAliases[gql] = rest
	}
}

// NormalizeReactionContent maps a GitHub REST or GraphQL reaction name to the
// REST content string (+1, laugh, …).
func NormalizeReactionContent(s string) (string, bool) {
	s = strings.TrimSpace(s)
	if s == "" {
		return "", false
	}
	if n, ok := reactionAliases[s]; ok {
		return n, true
	}
	if n, ok := reactionAliases[strings.ToUpper(s)]; ok {
		return n, true
	}
	if n, ok := reactionAliases[strings.ToLower(s)]; ok {
		return n, true
	}
	return "", false
}

// GraphQLReactionContent is the ReactionContent enum for addReaction/removeReaction.
func GraphQLReactionContent(s string) (string, bool) {
	n, ok := NormalizeReactionContent(s)
	if !ok {
		return "", false
	}
	g, ok := graphqlReaction[n]
	return g, ok
}

// ToggleReactions adds or removes the viewer's reaction of that content.
func ToggleReactions(rs []PRReaction, content string) []PRReaction {
	content, ok := NormalizeReactionContent(content)
	if !ok {
		return rs
	}
	out := make([]PRReaction, 0, len(rs)+1)
	found := false
	for _, r := range rs {
		n, ok := NormalizeReactionContent(r.Content)
		if !ok || r.Count <= 0 {
			continue
		}
		r.Content = n
		if n != content {
			out = append(out, r)
			continue
		}
		found = true
		if r.Mine {
			r.Count--
			r.Mine = false
		} else {
			r.Count++
			r.Mine = true
		}
		if r.Count > 0 {
			out = append(out, r)
		}
	}
	if !found {
		out = append(out, PRReaction{Content: content, Count: 1, Mine: true})
	}
	return SortReactions(out)
}

// SortReactions orders a rollup in GitHub picker order and drops empty groups.
func SortReactions(rs []PRReaction) []PRReaction {
	return sortReactions(rs)
}

// ReactionMine reports whether the viewer already reacted with content.
func ReactionMine(rs []PRReaction, content string) bool {
	content, ok := NormalizeReactionContent(content)
	if !ok {
		return false
	}
	for _, r := range rs {
		n, ok := NormalizeReactionContent(r.Content)
		if ok && n == content {
			return r.Mine
		}
	}
	return false
}

func sortReactions(rs []PRReaction) []PRReaction {
	kept := make([]PRReaction, 0, len(rs))
	for _, r := range rs {
		if r.Count > 0 && r.Content != "" {
			kept = append(kept, r)
		}
	}
	rs = kept
	if len(rs) < 2 {
		if len(rs) == 0 {
			return nil
		}
		return rs
	}
	rank := make(map[string]int, len(reactionContents))
	for i, c := range reactionContents {
		rank[c] = i
	}
	out := append([]PRReaction(nil), rs...)
	sort.SliceStable(out, func(i, j int) bool {
		return rank[out[i].Content] < rank[out[j].Content]
	})
	return out
}

// ReactionsEqual reports whether two reaction rollups match.
func ReactionsEqual(a, b []PRReaction) bool {
	if len(a) != len(b) {
		return false
	}
	for i := range a {
		if a[i].Content != b[i].Content || a[i].Count != b[i].Count || a[i].Mine != b[i].Mine {
			return false
		}
	}
	return true
}
