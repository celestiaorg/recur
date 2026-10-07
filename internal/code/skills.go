package code

import (
	"context"
	"os"
	"path/filepath"
	"regexp"
	"sort"
	"strings"
	"unicode/utf8"

	"recur/internal/domain"
)

var skillNameRE = regexp.MustCompile(`^[a-z][a-z0-9-]*$`)

// ListSkills discovers Agent Skills under the worktree and the user's home
// skill dirs. Dedupes by name (workspace wins over user). Cancelling ctx
// aborts. Roots come from domain.SkillRoots (provider-agnostic).
//
// Includes user-invocable: false skills (model-only); the composer menu filters
// those out via ListSkillsForMenu.
func ListSkills(ctx context.Context, worktree string) ([]domain.AgentSkill, error) {
	wsRoots, userRoots := domain.SkillRoots()
	byName := map[string]domain.AgentSkill{}

	scan := func(roots []string, base, scope string) error {
		for _, rel := range roots {
			if cerr := ctx.Err(); cerr != nil {
				return cerr
			}
			root := filepath.Join(base, filepath.FromSlash(rel))
			if err := collectSkills(ctx, root, scope, byName); err != nil {
				return err
			}
		}
		return nil
	}

	if worktree != "" {
		if err := scan(wsRoots, worktree, "workspace"); err != nil {
			return nil, err
		}
	}
	if home, err := os.UserHomeDir(); err == nil && home != "" {
		if err := scan(userRoots, home, "user"); err != nil {
			return nil, err
		}
	}

	out := make([]domain.AgentSkill, 0, len(byName))
	for _, s := range byName {
		out = append(out, s)
	}
	sort.Slice(out, func(i, j int) bool {
		return out[i].Name < out[j].Name
	})
	return out, nil
}

// ListSkillsForMenu is ListSkills filtered to user-invocable skills for the
// composer `/` typeahead. Model-only skills (user-invocable: false) stay
// available via mcp__recur__activate_skill.
func ListSkillsForMenu(ctx context.Context, worktree string) ([]domain.AgentSkill, error) {
	all, err := ListSkills(ctx, worktree)
	if err != nil {
		return nil, err
	}
	out := make([]domain.AgentSkill, 0, len(all))
	for _, s := range all {
		if s.UserInvocable {
			out = append(out, s)
		}
	}
	return out, nil
}

// SkillByName returns the skill with the given name, or false if missing.
func SkillByName(ctx context.Context, worktree, name string) (domain.AgentSkill, bool, error) {
	name = strings.TrimSpace(name)
	if name == "" || !skillNameRE.MatchString(name) {
		return domain.AgentSkill{}, false, nil
	}
	all, err := ListSkills(ctx, worktree)
	if err != nil {
		return domain.AgentSkill{}, false, err
	}
	for _, s := range all {
		if s.Name == name {
			return s, true, nil
		}
	}
	return domain.AgentSkill{}, false, nil
}

func collectSkills(ctx context.Context, root, scope string, byName map[string]domain.AgentSkill) error {
	info, err := os.Stat(root)
	if err != nil || !info.IsDir() {
		return nil
	}
	return filepath.WalkDir(root, func(path string, d os.DirEntry, err error) error {
		if err != nil {
			return nil
		}
		if cerr := ctx.Err(); cerr != nil {
			return cerr
		}
		if d.IsDir() {
			// Don't descend into huge trees under a misplaced skills root.
			if path != root && (d.Name() == "node_modules" || d.Name() == ".git") {
				return filepath.SkipDir
			}
			return nil
		}
		if d.Name() != "SKILL.md" && d.Name() != "skill.md" {
			return nil
		}
		dir := filepath.Dir(path)
		name := filepath.Base(dir)
		if !skillNameRE.MatchString(name) {
			return nil
		}
		if _, exists := byName[name]; exists {
			return nil // workspace (or earlier root) wins
		}
		meta, _, ok := readSkillFile(path)
		if !ok || meta.Description == "" {
			// Description is required for progressive-disclosure catalogs.
			return nil
		}
		byName[name] = domain.AgentSkill{
			Name:          name,
			Description:   truncateSkillDesc(meta.Description, 160),
			Scope:         scope,
			Path:          path,
			UserInvocable: meta.UserInvocable,
		}
		return nil
	})
}

type skillFileMeta struct {
	Description            string
	UserInvocable          bool
	DisableModelInvocation bool
}

// readSkillFile parses SKILL.md frontmatter + body. ok is false when the file
// can't be read or YAML is completely unusable (no description and no body).
func readSkillFile(path string) (meta skillFileMeta, body string, ok bool) {
	b, err := os.ReadFile(path)
	if err != nil {
		return skillFileMeta{}, "", false
	}
	raw := string(b)
	fm, body := frontmatterBlock(raw)
	meta.Description = parseFrontmatterDescription(fm)
	meta.UserInvocable = !frontmatterBoolFalse(fm, "user-invocable") // default true
	meta.DisableModelInvocation = frontmatterBoolTrue(fm, "disable-model-invocation")
	body = strings.TrimSpace(body)
	if fm == "" {
		body = strings.TrimSpace(raw)
	}
	if meta.Description == "" && body == "" {
		return meta, body, false
	}
	return meta, body, true
}

// ReadSkillBody returns the markdown body of SKILL.md with YAML frontmatter
// stripped (tier-2 activation content). Empty when unreadable.
func ReadSkillBody(path string) string {
	_, body, ok := readSkillFile(path)
	if !ok {
		return ""
	}
	return body
}

// SkillDisablesModelInvocation reports whether the skill opted out of
// model-driven catalog activation (still valid for user `/` activation).
func SkillDisablesModelInvocation(path string) bool {
	meta, _, ok := readSkillFile(path)
	return ok && meta.DisableModelInvocation
}

// ListSkillResources returns skill-directory-relative paths for bundled
// scripts/references/assets (not eagerly read). Capped for prompt size.
func ListSkillResources(skillMDPath string, limit int) []string {
	if limit <= 0 {
		limit = 40
	}
	dir := filepath.Dir(skillMDPath)
	var out []string
	_ = filepath.WalkDir(dir, func(path string, d os.DirEntry, err error) error {
		if err != nil || path == dir {
			return nil
		}
		if d.IsDir() {
			name := d.Name()
			if name == "node_modules" || name == ".git" {
				return filepath.SkipDir
			}
			return nil
		}
		base := filepath.Base(path)
		if strings.EqualFold(base, "SKILL.md") {
			return nil
		}
		rel, err := filepath.Rel(dir, path)
		if err != nil {
			return nil
		}
		out = append(out, filepath.ToSlash(rel))
		if len(out) >= limit {
			return filepath.SkipAll
		}
		return nil
	})
	sort.Strings(out)
	if len(out) > limit {
		out = out[:limit]
	}
	return out
}

func frontmatterBlock(raw string) (fm, body string) {
	text := strings.TrimLeft(raw, "\uFEFF \t")
	if !strings.HasPrefix(text, "---") {
		return "", strings.TrimSpace(raw)
	}
	rest := strings.TrimPrefix(text, "---")
	rest = strings.TrimLeft(rest, "\r\n")
	end := strings.Index(rest, "\n---")
	if end < 0 {
		return "", strings.TrimSpace(raw)
	}
	fm = rest[:end]
	body = rest[end+len("\n---"):]
	body = strings.TrimLeft(body, "\r\n")
	return fm, body
}

func truncateSkillDesc(val string, max int) string {
	val = strings.Join(strings.Fields(val), " ")
	if max <= 0 || len(val) <= max {
		return val
	}
	cut := max - 1
	if cut < 0 {
		cut = 0
	}
	for cut > 0 && !utf8.RuneStart(val[cut]) {
		cut--
	}
	return val[:cut] + "…"
}

func parseFrontmatterDescription(fm string) string {
	lines := strings.Split(fm, "\n")
	for i := 0; i < len(lines); i++ {
		line := strings.TrimRight(lines[i], "\r")
		trimmed := strings.TrimSpace(line)
		if !strings.HasPrefix(trimmed, "description:") {
			continue
		}
		val := strings.TrimSpace(strings.TrimPrefix(trimmed, "description:"))
		// Folded block scalar: description: >  / description: |
		if val == ">" || val == "|" || val == ">-" || val == "|-" {
			var parts []string
			for j := i + 1; j < len(lines); j++ {
				l := strings.TrimRight(lines[j], "\r")
				if len(l) > 0 && (l[0] == ' ' || l[0] == '\t') {
					parts = append(parts, strings.TrimSpace(l))
					continue
				}
				if strings.TrimSpace(l) == "" && len(parts) > 0 {
					break
				}
				if strings.TrimSpace(l) != "" && !strings.HasPrefix(l, " ") && !strings.HasPrefix(l, "\t") {
					break
				}
			}
			val = strings.Join(parts, " ")
		} else {
			val = strings.Trim(val, `"'`)
		}
		return strings.Join(strings.Fields(val), " ")
	}
	return ""
}

func frontmatterBoolTrue(fm, key string) bool {
	v, ok := frontmatterScalar(fm, key)
	if !ok {
		return false
	}
	switch strings.ToLower(strings.TrimSpace(v)) {
	case "true", "yes", "1":
		return true
	default:
		return false
	}
}

// frontmatterBoolFalse is true when the key is explicitly false (default true for user-invocable).
func frontmatterBoolFalse(fm, key string) bool {
	v, ok := frontmatterScalar(fm, key)
	if !ok {
		return false
	}
	switch strings.ToLower(strings.TrimSpace(v)) {
	case "false", "no", "0":
		return true
	default:
		return false
	}
}

func frontmatterScalar(fm, key string) (string, bool) {
	prefix := key + ":"
	for _, line := range strings.Split(fm, "\n") {
		trimmed := strings.TrimSpace(strings.TrimRight(line, "\r"))
		if !strings.HasPrefix(trimmed, prefix) {
			continue
		}
		return strings.Trim(strings.TrimSpace(strings.TrimPrefix(trimmed, prefix)), `"'`), true
	}
	return "", false
}
