package app

import (
	"fmt"
	"strings"

	"recur/internal/bundles"
	"recur/internal/code"
	"recur/internal/lsp"
	"recur/internal/store"
)

// LSPService supervises language server processes for the Observatory editor.
//
// Contract (mirrors TerminalService streaming):
//
//	EnsureLSP(projectID, workspaceID, languageID, relPath) → SessionInfo + URIs
//	WriteLSP(id, message)  // JSON-RPC body, no Content-Length headers
//	CloseLSP(id)
//	  ...lsp.message events {id, message}...
//	  ...lsp.exit events {id}...
//
// The frontend @codemirror/lsp-client owns protocol/UI; this service only
// frames stdio and resolves worktree roots.
type LSPService struct {
	base
	mgr *lsp.Manager
}

// LanguageServerInfo is one discovered / builtin language server for Settings.
type LanguageServerInfo struct {
	ID         string   `json:"id"`
	Languages  []string `json:"languages"`
	Command    string   `json:"command"`
	Managed    string   `json:"managed,omitempty"`
	Source     string   `json:"source"` // "bundle" | "builtin"
	BundleName string   `json:"bundleName,omitempty"`
	Available  bool     `json:"available"`
}

// EnsureResult is returned when opening a file that may need a language server.
type EnsureResult struct {
	lsp.SessionInfo
	FileURI    string            `json:"fileUri,omitempty"`
	LanguageID string            `json:"languageId,omitempty"`
	Extra      []lsp.SessionInfo `json:"extra,omitempty"`
}

// EnsureLSP starts or reuses a language server for the file's language.
// Soft-degrades (Available=false) when no server is registered or missing.
func (s *LSPService) EnsureLSP(projectID, workspaceID, languageID, relPath string) (EnsureResult, error) {
	s.refreshRegistry()
	root, _, err := s.resolveRoot(projectID, workspaceID)
	if err != nil {
		return EnsureResult{}, err
	}
	rel := strings.TrimSpace(relPath)
	lang := strings.TrimSpace(languageID)
	root = lsp.CanonPath(root)
	infos, err := s.mgr.EnsureAll(workspaceID, root, lang, rel)
	if err != nil {
		return EnsureResult{}, err
	}
	if lang == "" {
		if specs := lsp.LookupAll("", rel); len(specs) > 0 && len(specs[0].Languages) > 0 {
			lang = specs[0].Languages[0]
		}
	}
	primary := lsp.SessionInfo{Available: false, Reason: "no language server registered for " + lang}
	var extra []lsp.SessionInfo
	if len(infos) > 0 {
		primary = infos[0]
		if len(infos) > 1 {
			extra = infos[1:]
		}
	}
	out := EnsureResult{SessionInfo: primary, LanguageID: lang, Extra: extra}
	if rel != "" {
		abs, err := code.AbsPath(root, rel)
		if err != nil {
			return EnsureResult{}, err
		}
		out.FileURI = lsp.PathToURI(lsp.CanonPath(abs))
	}
	return out, nil
}

// ListLanguageServers returns merged Bundle + builtin language servers for Settings.
func (s *LSPService) ListLanguageServers() ([]LanguageServerInfo, error) {
	s.refreshRegistry()
	specs := lsp.AllServers()
	out := make([]LanguageServerInfo, 0, len(specs))
	for _, spec := range specs {
		available := spec.Available()
		if !available && spec.Managed != "" {
			// Managed may still install on Ensure; report PATH presence only here.
			available = false
		}
		out = append(out, LanguageServerInfo{
			ID:         spec.Key,
			Languages:  append([]string(nil), spec.Languages...),
			Command:    spec.Command,
			Managed:    spec.Managed,
			Source:     spec.Source,
			BundleName: spec.BundleName,
			Available:  available,
		})
	}
	return out, nil
}

// WriteLSP sends one JSON-RPC message body to the session.
func (s *LSPService) WriteLSP(id, message string) error {
	if strings.TrimSpace(id) == "" {
		return fmt.Errorf("missing lsp session id")
	}
	return s.mgr.Write(id, message)
}

// CloseLSP stops one language server session.
func (s *LSPService) CloseLSP(id string) error {
	s.mgr.Close(id)
	return nil
}

// CloseWorkspaceLSP stops every language server for a workspace.
func (s *LSPService) CloseWorkspaceLSP(workspaceID string) error {
	s.mgr.CloseWorkspace(workspaceID)
	return nil
}

// ServiceShutdown kills every language server on app quit.
func (s *LSPService) ServiceShutdown() error {
	s.mgr.CloseAll()
	return nil
}

func (s *LSPService) refreshRegistry() {
	lsp.SetDisabledServers(s.st().Settings().DisabledLanguageServers)
	def, err := store.BundlesDir()
	if err != nil {
		return
	}
	user, err := store.UserBundlesDir()
	if err != nil {
		return
	}
	cat, err := bundles.DiscoverCatalog(bundles.DiscoverRoots{UserDir: user, DefaultDir: def})
	if err != nil {
		return
	}
	lsp.SetBundleServers(cat.LanguageServers)
}

// resolveRoot matches CodeService worktree-then-repo precedence.
func (s *LSPService) resolveRoot(projectID, workspaceID string) (root string, writable bool, err error) {
	var repo, wt string
	if workspaceID != "" {
		ws, ok := s.st().Workspace(workspaceID)
		if !ok {
			return "", false, fmt.Errorf("workspace not found")
		}
		wt = ws.WorktreePath
		if projectID == "" {
			projectID = ws.ProjectID
		}
	}
	if projectID != "" {
		proj, ok := s.st().Project(projectID)
		if !ok {
			return "", false, fmt.Errorf("project not found")
		}
		repo = proj.RepoPath
	}
	return code.ResolveRootInfo(repo, wt)
}
