package bundles

import (
	"fmt"
	"os"
	"path/filepath"
	"sort"
	"strings"
)

// DiscoverRoots lists the scan roots in precedence order (later wins on id
// collision — callers should walk user then default, or use Discover which
// applies user-over-default).
type DiscoverRoots struct {
	UserDir    string // ~/.recur/bundles
	DefaultDir string // <DataDir>/Bundles
}

// DiscoverSkip is one bundle directory that was present under a scan root but
// not loaded (missing/unreadable manifest, parse/schema error, etc.), or a
// prompt skipped during slash collision resolution.
type DiscoverSkip struct {
	Dir    string // absolute bundle directory
	Name   string // directory basename (handy for UI) or prompt slash
	Source string // "user" | "default"
	Reason string
}

// Discover scans user then default roots, parses each bundle.toml, and returns
// deduped commands and prompts (user id wins; prompts also resolve slash
// collisions). Reload-on-demand — no file watcher in v1. Malformed packs are
// skipped (see DiscoverReport for reasons).
func Discover(roots DiscoverRoots) ([]Command, []PromptAction, error) {
	cat, err := DiscoverCatalog(roots)
	if err != nil {
		return nil, nil, err
	}
	return cat.Commands, cat.Prompts, nil
}

// DiscoverReport is Discover plus per-directory (and slash-collision) skip
// reasons so callers (Settings, CLI) can surface "why did my commands disappear?".
func DiscoverReport(roots DiscoverRoots) ([]Command, []PromptAction, []DiscoverSkip, error) {
	cat, err := DiscoverCatalog(roots)
	if err != nil {
		return nil, nil, nil, err
	}
	return cat.Commands, cat.Prompts, cat.Skips, nil
}

// themeSortOrder puts explicit order first; zero means "unordered"
// and sorts after every positive order (Settings display).
func themeSortOrder(order int) int {
	if order == 0 {
		return 1_000_000
	}
	return order
}

// DiscoverCatalog is DiscoverReport plus TextMate grammars, themes,
// terminal themes, language data, snippets, symbols, language servers, and settings.
// Command / prompt / grammar / theme / terminalTheme / languageData /
// snippet / symbol / language-server / setting precedence: user over default by id.
// TextMate still keys runtime tokenization on scopeName; after id dedupe,
// colliding scopeNames prefer user over default.
func DiscoverCatalog(roots DiscoverRoots) (Catalog, error) {
	byID := map[string]Command{}
	promptByID := map[string]PromptAction{}
	grammarByID := map[string]Grammar{}
	themeByID := map[string]Theme{}
	termThemeByID := map[string]TerminalTheme{}
	langDataByID := map[string]LanguageData{}
	snippetByID := map[string]Snippet{}
	symbolByID := map[string]Symbol{}
	serverByID := map[string]LanguageServer{}
	settingByID := map[string]Setting{}
	searchByID := map[string]SearchAction{}
	expandByID := map[string]ExpandAction{}
	roleByID := map[string]Role{}
	loopByID := map[string]LoopDef{}
	policyByID := map[string]Policy{}
	var skips []DiscoverSkip
	// Default first, then user overwrites.
	for _, root := range []struct {
		dir    string
		source string
	}{
		{roots.DefaultDir, "default"},
		{roots.UserDir, "user"},
	} {
		if strings.TrimSpace(root.dir) == "" {
			continue
		}
		entries, err := os.ReadDir(root.dir)
		if err != nil {
			if os.IsNotExist(err) {
				continue
			}
			return Catalog{}, fmt.Errorf("read bundles dir %q: %w", root.dir, err)
		}
		for _, e := range entries {
			if !e.IsDir() {
				continue
			}
			bundleDir := filepath.Join(root.dir, e.Name())
			data, err := os.ReadFile(filepath.Join(bundleDir, "bundle.toml"))
			if err != nil {
				// Skip unreadable/missing manifests so one bad pack cannot wipe the registry.
				skips = append(skips, DiscoverSkip{
					Dir: bundleDir, Name: e.Name(), Source: root.source,
					Reason: skipReadReason(err),
				})
				continue
			}
			parsed, err := ParseManifestFull(bundleDir, data)
			if err != nil {
				skips = append(skips, DiscoverSkip{
					Dir: bundleDir, Name: e.Name(), Source: root.source,
					Reason: err.Error(),
				})
				continue
			}
			for _, c := range parsed.Commands {
				c.SourceRoot = root.source
				byID[c.ID] = c
			}
			for _, p := range parsed.Prompts {
				p.SourceRoot = root.source
				promptByID[p.ID] = p
			}
			for _, g := range parsed.Grammars {
				g.SourceRoot = root.source
				grammarByID[g.ID] = g
			}
			for _, t := range parsed.Themes {
				t.SourceRoot = root.source
				themeByID[t.ID] = t
			}
			for _, t := range parsed.TerminalThemes {
				t.SourceRoot = root.source
				termThemeByID[t.ID] = t
			}
			for _, d := range parsed.LanguageData {
				d.SourceRoot = root.source
				langDataByID[d.ID] = d
			}
			for _, sn := range parsed.Snippets {
				sn.SourceRoot = root.source
				snippetByID[sn.ID] = sn
			}
			for _, sy := range parsed.Symbols {
				sy.SourceRoot = root.source
				symbolByID[sy.ID] = sy
			}
			for _, s := range parsed.LanguageServers {
				s.SourceRoot = root.source
				serverByID[s.ID] = s
			}
			for _, st := range parsed.Settings {
				st.SourceRoot = root.source
				settingByID[st.ID] = st
			}
			for _, se := range parsed.Searches {
				se.SourceRoot = root.source
				searchByID[se.ID] = se
			}
			for _, ex := range parsed.Expands {
				ex.SourceRoot = root.source
				expandByID[ex.ID] = ex
			}
			for _, r := range parsed.Roles {
				r.SourceRoot = root.source
				roleByID[r.ID] = r
			}
			for _, lp := range parsed.Loops {
				lp.SourceRoot = root.source
				loopByID[lp.ID] = lp
			}
			for _, p := range parsed.Policies {
				p.SourceRoot = root.source
				policyByID[p.ID] = p
			}
		}
	}
	out := make([]Command, 0, len(byID))
	for _, c := range byID {
		out = append(out, c)
	}
	sort.SliceStable(out, func(i, j int) bool {
		if out[i].Order != out[j].Order {
			return out[i].Order < out[j].Order
		}
		if out[i].Category != out[j].Category {
			return strings.ToLower(out[i].Category) < strings.ToLower(out[j].Category)
		}
		return strings.ToLower(out[i].Name) < strings.ToLower(out[j].Name)
	})

	promptList := make([]PromptAction, 0, len(promptByID))
	for _, p := range promptByID {
		promptList = append(promptList, p)
	}
	prompts, slashSkips := resolvePromptSlashes(promptList)
	skips = append(skips, slashSkips...)

	// Collapse id-deduped grammars onto unique scopeNames for TextMate.
	grammarList := make([]Grammar, 0, len(grammarByID))
	for _, g := range grammarByID {
		grammarList = append(grammarList, g)
	}
	sort.SliceStable(grammarList, func(i, j int) bool {
		// default before user so user wins on scopeName collisions.
		if grammarList[i].SourceRoot != grammarList[j].SourceRoot {
			return grammarList[i].SourceRoot < grammarList[j].SourceRoot
		}
		return grammarList[i].ID < grammarList[j].ID
	})
	grammarByScope := map[string]Grammar{}
	for _, g := range grammarList {
		grammarByScope[g.ScopeName] = g
	}
	grammars := make([]Grammar, 0, len(grammarByScope))
	for _, g := range grammarByScope {
		grammars = append(grammars, g)
	}
	sort.SliceStable(grammars, func(i, j int) bool {
		return grammars[i].ScopeName < grammars[j].ScopeName
	})

	themes := make([]Theme, 0, len(themeByID))
	for _, t := range themeByID {
		themes = append(themes, t)
	}
	sort.SliceStable(themes, func(i, j int) bool {
		oi, oj := themeSortOrder(themes[i].Order), themeSortOrder(themes[j].Order)
		if oi != oj {
			return oi < oj
		}
		return strings.ToLower(themes[i].Name) < strings.ToLower(themes[j].Name)
	})

	servers := make([]LanguageServer, 0, len(serverByID))
	for _, s := range serverByID {
		servers = append(servers, s)
	}
	sort.SliceStable(servers, func(i, j int) bool {
		return strings.ToLower(servers[i].ID) < strings.ToLower(servers[j].ID)
	})

	termThemes := make([]TerminalTheme, 0, len(termThemeByID))
	for _, t := range termThemeByID {
		termThemes = append(termThemes, t)
	}
	sort.SliceStable(termThemes, func(i, j int) bool {
		oi, oj := themeSortOrder(termThemes[i].Order), themeSortOrder(termThemes[j].Order)
		if oi != oj {
			return oi < oj
		}
		return strings.ToLower(termThemes[i].Name) < strings.ToLower(termThemes[j].Name)
	})

	sort.SliceStable(skips, func(i, j int) bool {
		if skips[i].Source != skips[j].Source {
			return skips[i].Source < skips[j].Source
		}
		return strings.ToLower(skips[i].Name) < strings.ToLower(skips[j].Name)
	})

	// Collapse id-deduped language data onto unique scopeNames (user wins).
	langList := make([]LanguageData, 0, len(langDataByID))
	for _, d := range langDataByID {
		langList = append(langList, d)
	}
	sort.SliceStable(langList, func(i, j int) bool {
		if langList[i].SourceRoot != langList[j].SourceRoot {
			return langList[i].SourceRoot < langList[j].SourceRoot
		}
		return langList[i].ID < langList[j].ID
	})
	langByScope := map[string]LanguageData{}
	for _, d := range langList {
		langByScope[d.ScopeName] = d
	}
	langData := make([]LanguageData, 0, len(langByScope))
	for _, d := range langByScope {
		langData = append(langData, d)
	}
	sort.SliceStable(langData, func(i, j int) bool {
		return langData[i].ScopeName < langData[j].ScopeName
	})

	snippets := make([]Snippet, 0, len(snippetByID))
	for _, s := range snippetByID {
		snippets = append(snippets, s)
	}
	sort.SliceStable(snippets, func(i, j int) bool {
		if snippets[i].Order != snippets[j].Order {
			return snippets[i].Order < snippets[j].Order
		}
		if snippets[i].BundleName != snippets[j].BundleName {
			return strings.ToLower(snippets[i].BundleName) < strings.ToLower(snippets[j].BundleName)
		}
		return strings.ToLower(snippets[i].Name) < strings.ToLower(snippets[j].Name)
	})

	symbols := make([]Symbol, 0, len(symbolByID))
	for _, s := range symbolByID {
		symbols = append(symbols, s)
	}
	sort.SliceStable(symbols, func(i, j int) bool {
		if symbols[i].BundleName != symbols[j].BundleName {
			return strings.ToLower(symbols[i].BundleName) < strings.ToLower(symbols[j].BundleName)
		}
		return symbols[i].ID < symbols[j].ID
	})

	settings := make([]Setting, 0, len(settingByID))
	for _, st := range settingByID {
		settings = append(settings, st)
	}
	sort.SliceStable(settings, func(i, j int) bool {
		if settings[i].BundleName != settings[j].BundleName {
			return strings.ToLower(settings[i].BundleName) < strings.ToLower(settings[j].BundleName)
		}
		if settings[i].Label != settings[j].Label {
			return strings.ToLower(settings[i].Label) < strings.ToLower(settings[j].Label)
		}
		return settings[i].ID < settings[j].ID
	})

	searches := make([]SearchAction, 0, len(searchByID))
	for _, se := range searchByID {
		searches = append(searches, se)
	}
	sort.SliceStable(searches, func(i, j int) bool {
		if searches[i].Name != searches[j].Name {
			return strings.ToLower(searches[i].Name) < strings.ToLower(searches[j].Name)
		}
		return searches[i].ID < searches[j].ID
	})

	expands := make([]ExpandAction, 0, len(expandByID))
	for _, ex := range expandByID {
		expands = append(expands, ex)
	}
	sort.SliceStable(expands, func(i, j int) bool {
		return expands[i].ID < expands[j].ID
	})

	roleList := make([]Role, 0, len(roleByID))
	for _, r := range roleByID {
		roleList = append(roleList, r)
	}
	sort.SliceStable(roleList, func(i, j int) bool {
		if roleList[i].SourceRoot != roleList[j].SourceRoot {
			return roleList[i].SourceRoot < roleList[j].SourceRoot
		}
		return roleList[i].ID < roleList[j].ID
	})
	codeOwner := map[string]string{}
	keptRoles := map[string]Role{}
	for _, r := range roleList {
		if prev, ok := codeOwner[r.Code]; ok && prev != r.ID {
			skips = append(skips, DiscoverSkip{
				Dir: r.BundleDir, Name: r.ID, Source: r.SourceRoot,
				Reason: fmt.Sprintf("role code %s collides with %s", r.Code, prev),
			})
			continue
		}
		codeOwner[r.Code] = r.ID
		keptRoles[r.ID] = r
	}
	roleByID = keptRoles

	roles := make([]Role, 0, len(roleByID))
	for _, r := range roleByID {
		roles = append(roles, r)
	}
	sort.SliceStable(roles, func(i, j int) bool {
		return strings.ToLower(roles[i].Name) < strings.ToLower(roles[j].Name)
	})

	loops := make([]LoopDef, 0, len(loopByID))
	for _, l := range loopByID {
		loops = append(loops, l)
	}
	sort.SliceStable(loops, func(i, j int) bool {
		return strings.ToLower(loops[i].Name) < strings.ToLower(loops[j].Name)
	})

	policies := make([]Policy, 0, len(policyByID))
	for _, p := range policyByID {
		policies = append(policies, p)
	}
	sort.SliceStable(policies, func(i, j int) bool {
		return strings.ToLower(policies[i].Name) < strings.ToLower(policies[j].Name)
	})

	sort.SliceStable(skips, func(i, j int) bool {
		if skips[i].Source != skips[j].Source {
			return skips[i].Source < skips[j].Source
		}
		return strings.ToLower(skips[i].Name) < strings.ToLower(skips[j].Name)
	})

	return Catalog{
		Commands:        out,
		Prompts:         prompts,
		Grammars:        grammars,
		Themes:          themes,
		TerminalThemes:  termThemes,
		LanguageData:    langData,
		Snippets:        snippets,
		Symbols:         symbols,
		LanguageServers: servers,
		Settings:        settings,
		Searches:        searches,
		Expands:         expands,
		Roles:           roles,
		Loops:           loops,
		Policies:        policies,
		Skips:           skips,
	}, nil
}

// resolvePromptSlashes keeps one prompt per slash after id dedupe.
// Precedence: user source beats default; same-source ties break by bundle id
// then prompt id (lexicographically smaller wins). Losers become DiscoverSkip.
func resolvePromptSlashes(prompts []PromptAction) ([]PromptAction, []DiscoverSkip) {
	bySlash := map[string][]PromptAction{}
	for _, p := range prompts {
		bySlash[p.Slash] = append(bySlash[p.Slash], p)
	}
	out := make([]PromptAction, 0, len(bySlash))
	var skips []DiscoverSkip
	for slash, group := range bySlash {
		if len(group) == 1 {
			out = append(out, group[0])
			continue
		}
		winner := pickPromptSlashWinner(group)
		for _, p := range group {
			if p.ID == winner.ID && p.BundleID == winner.BundleID {
				continue
			}
			skips = append(skips, DiscoverSkip{
				Dir:    p.BundleDir,
				Name:   p.Slash,
				Source: p.SourceRoot,
				Reason: fmt.Sprintf(
					"slash %q collides with prompt %q from bundle %q (kept %s/%s)",
					slash, winner.Name, winner.BundleName, winner.SourceRoot, winner.ID,
				),
			})
		}
		out = append(out, winner)
	}
	sort.SliceStable(out, func(i, j int) bool {
		return out[i].Slash < out[j].Slash
	})
	return out, skips
}

func pickPromptSlashWinner(group []PromptAction) PromptAction {
	best := group[0]
	for _, p := range group[1:] {
		if promptSlashBeats(p, best) {
			best = p
		}
	}
	return best
}

// promptSlashBeats reports whether a should win over b for the same slash.
func promptSlashBeats(a, b PromptAction) bool {
	if a.SourceRoot != b.SourceRoot {
		return a.SourceRoot == "user"
	}
	if a.BundleID != b.BundleID {
		return a.BundleID < b.BundleID
	}
	return a.ID < b.ID
}

func skipReadReason(err error) string {
	if os.IsNotExist(err) {
		return "missing bundle.toml"
	}
	if os.IsPermission(err) {
		return "cannot read bundle.toml (permission denied) — check directory permissions"
	}
	return fmt.Sprintf("cannot read bundle.toml: %v", err)
}

// FindByID returns the command with the given id from a Discover result.
func FindByID(cmds []Command, id string) (Command, bool) {
	for _, c := range cmds {
		if c.ID == id {
			return c, true
		}
	}
	return Command{}, false
}

// FindPromptByID returns the prompt action with the given id.
func FindPromptByID(prompts []PromptAction, id string) (PromptAction, bool) {
	for _, p := range prompts {
		if p.ID == id {
			return p, true
		}
	}
	return PromptAction{}, false
}

// FindPromptBySlash returns the prompt action with the given slash token.
func FindPromptBySlash(prompts []PromptAction, slash string) (PromptAction, bool) {
	slash = strings.TrimSpace(slash)
	for _, p := range prompts {
		if p.Slash == slash {
			return p, true
		}
	}
	return PromptAction{}, false
}

// FindLanguageServer returns the language server with the given id.
func FindLanguageServer(servers []LanguageServer, id string) (LanguageServer, bool) {
	for _, s := range servers {
		if s.ID == id {
			return s, true
		}
	}
	return LanguageServer{}, false
}
