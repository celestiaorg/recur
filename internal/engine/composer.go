package engine

import (
	"context"
	"errors"
	"regexp"
	"strings"

	"recur/internal/bundles"
	"recur/internal/domain"
	"recur/internal/store"
)

// ErrEmptyPromptAction is returned when a leading /token action ran and
// produced no wire prompt. Loop turns treat this as on_fail; PA* / Maestro
// treat it as a no-send success (e.g. /update finished in git).
var ErrEmptyPromptAction = errors.New("prompt action returned no prompt")

// expandOpts configures ExpandComposer.
type expandOpts struct {
	worktree       string
	workspaceID    string
	ignoreDisabled bool // loop turns ignore Settings disabled-prompt list
	catalog        *bundles.Catalog
}

// ExpandResult is the wire prompt after slash-action + wrapComposer.
type ExpandResult struct {
	Prompt       string
	Display      string
	RanAction    bool
	FreshSession bool
}

// leadingSlashToken is a composer-leading /name (same token class as the
// frontend / menu). Remainder after the token must be empty or whitespace
// for the action to fire — matches promptActionQueryAt.
var leadingSlashToken = regexp.MustCompile(`^/([A-Za-z0-9][A-Za-z0-9_-]*)\s*$`)

// ExpandComposer is the one host-side expand for PA* send, Maestro send,
// and loop specialists: leading [[prompt]] slash (when matched) then
// wrapComposer (@file, /skill, @terminal, raw regions).
// [[prompt]].mode routing is inert — the caller already picked the role.
func (e *Engine) ExpandComposer(ctx context.Context, text string, opts expandOpts) (ExpandResult, error) {
	raw := text
	display := ""
	ran := false
	fresh := false
	if ctx == nil {
		ctx = context.Background()
	}

	if slash, ok := leadingPromptSlash(text); ok {
		if p, found := e.lookupPromptAction(slash, opts.ignoreDisabled, opts.catalog); found {
			res, err := e.runPromptAction(ctx, p, opts)
			if err != nil {
				return ExpandResult{RanAction: true, FreshSession: p.FreshSession}, err
			}
			ran = true
			fresh = p.FreshSession
			display = res.Display
			text = res.Prompt
			if strings.TrimSpace(text) == "" {
				return ExpandResult{Prompt: "", Display: display, RanAction: true, FreshSession: fresh}, ErrEmptyPromptAction
			}
		}
	}

	out := wrapComposer(text, opts.worktree, opts.workspaceID, func(name string) bool {
		return e.skillEnabledForWorkspace(opts.workspaceID, name)
	}, e.terminals)
	if display == "" && out != raw {
		display = raw
	}
	return ExpandResult{Prompt: out, Display: display, RanAction: ran, FreshSession: fresh}, nil
}

func leadingPromptSlash(text string) (string, bool) {
	s := strings.TrimSpace(text)
	m := leadingSlashToken.FindStringSubmatch(s)
	if m == nil {
		return "", false
	}
	return m[1], true
}

func (e *Engine) lookupPromptAction(slash string, ignoreDisabled bool, cat *bundles.Catalog) (bundles.PromptAction, bool) {
	var resolved bundles.Catalog
	if cat != nil {
		resolved = *cat
	} else {
		var err error
		resolved, err = e.discoverLoopCatalog()
		if err != nil {
			return bundles.PromptAction{}, false
		}
	}
	p, ok := bundles.FindPromptBySlash(resolved.Prompts, slash)
	if !ok {
		return bundles.PromptAction{}, false
	}
	if !ignoreDisabled && containsID(e.settings().DisabledBundlePrompts, p.ID) {
		return bundles.PromptAction{}, false
	}
	return p, true
}

func (e *Engine) runPromptAction(ctx context.Context, p bundles.PromptAction, opts expandOpts) (bundles.PromptResult, error) {
	cache, err := store.BundlesCacheDir()
	if err != nil {
		return bundles.PromptResult{}, err
	}
	return bundles.RunPrompt(ctx, cache, p, e.promptRunContext(opts.workspaceID, opts.worktree, p))
}

func (e *Engine) promptRunContext(workspaceID, worktree string, prompt bundles.PromptAction) bundles.PromptRunContext {
	rc := bundles.PromptRunContext{
		WorkspaceID:   workspaceID,
		Worktree:      worktree,
		WorkDir:       worktree,
		BundleSupport: prompt.BundleDir,
		TabSize:       4,
		SoftTabs:      true,
	}
	if ws, ok := e.Store.Workspace(workspaceID); ok {
		rc.ProjectID = ws.ProjectID
		rc.Port = ws.Port
		rc.BaseBranch = strings.TrimSpace(ws.BaseBranch)
		if proj, pok := e.Store.Project(ws.ProjectID); pok {
			if rc.BaseBranch == "" {
				rc.BaseBranch = strings.TrimSpace(proj.BaseBranch)
			}
			rc.UpdateBaseMethod = string(domain.EffectiveUpdateBaseMethod(proj, e.settings()))
		}
	}
	return rc
}
