package lsp

import (
	"context"
	"fmt"
	"strings"
	"sync"

	"github.com/google/uuid"

	"recur/internal/lsp/managed"
)

// SessionInfo is the frontend-facing handle for an LSP session.
type SessionInfo struct {
	ID        string `json:"id"`
	ServerKey string `json:"serverKey"`
	RootURI   string `json:"rootUri"`
	RootPath  string `json:"rootPath"`
	Available bool   `json:"available"`
	// Reason explains soft-degrade when Available is false.
	Reason string `json:"reason,omitempty"`
	// VuePluginPath is the @vue/language-server package dir for TS initialize.
	VuePluginPath string `json:"vuePluginPath,omitempty"`
}

// Manager owns language server processes keyed by workspace + server.
type Manager struct {
	emit    Emitter
	dataDir string

	mu       sync.Mutex
	sessions map[string]*Session // session id → session
	byKey    map[string]string   // workspaceID\0serverKey → session id
}

// NewManager creates a manager that publishes via emit.
// dataDir is Recur's managed root (for download+pin language servers); empty
// disables managed installs (PATH-only).
func NewManager(emit Emitter, dataDir string) *Manager {
	return &Manager{
		emit:     emit,
		dataDir:  dataDir,
		sessions: make(map[string]*Session),
		byKey:    make(map[string]string),
	}
}

func sessionKey(workspaceID, serverKey string) string {
	return workspaceID + "\x00" + serverKey
}

// Ensure starts (or reuses) language servers for languageID under rootPath.
// When several servers claim the language, Extra on the service result carries
// the rest; this returns the first available (or a soft-degrade info).
func (m *Manager) Ensure(workspaceID, rootPath, languageID, relPath string) (SessionInfo, error) {
	all, err := m.EnsureAll(workspaceID, rootPath, languageID, relPath)
	if err != nil {
		return SessionInfo{}, err
	}
	if len(all) == 0 {
		return SessionInfo{
			Available: false,
			Reason:    "no language server registered for " + languageID,
		}, nil
	}
	return all[0], nil
}

// EnsureAll starts or reuses every non-disabled server for languageID.
// Unavailable servers are omitted; if none are available, the slice has one
// Available=false info. relPath applies per-server FileGlob.
func (m *Manager) EnsureAll(workspaceID, rootPath, languageID, relPath string) ([]SessionInfo, error) {
	specs := LookupAll(languageID, relPath)
	for _, spec := range specs {
		if spec.Key == "vue-language-server" && spec.Managed != "" {
			_, _ = managed.Ensure(context.Background(), m.dataDir, spec.Managed)
			break
		}
	}
	if len(specs) == 0 {
		return []SessionInfo{{
			Available: false,
			Reason:    "no language server registered for " + languageID,
		}}, nil
	}
	var out []SessionInfo
	var lastMiss SessionInfo
	for _, spec := range specs {
		info, err := m.ensureSpec(workspaceID, rootPath, spec)
		if err != nil {
			return nil, err
		}
		if info.Available {
			out = append(out, info)
		} else {
			lastMiss = info
		}
	}
	if len(out) == 0 {
		if lastMiss.Reason == "" {
			lastMiss.Reason = "no language server available for " + languageID
		}
		return []SessionInfo{lastMiss}, nil
	}
	return out, nil
}

func (m *Manager) ensureSpec(workspaceID, rootPath string, spec ServerSpec) (SessionInfo, error) {
	resolved := spec
	if resolved.Managed != "" {
		path, err := managed.Ensure(context.Background(), m.dataDir, resolved.Managed)
		if err == nil && path != "" {
			resolved.Command = path
		}
	}
	if !resolved.Available() {
		reason := resolved.Command + " not found on PATH"
		if resolved.Managed != "" {
			reason = resolved.Managed + " not installed (and not on PATH)"
		}
		return SessionInfo{
			ServerKey: resolved.Key,
			Available: false,
			Reason:    reason,
		}, nil
	}

	plugin := managed.VuePluginPath(m.dataDir)

	m.mu.Lock()
	defer m.mu.Unlock()

	key := sessionKey(workspaceID, resolved.Key)
	if id, ok := m.byKey[key]; ok {
		if s, ok := m.sessions[id]; ok && !s.isClosed() {
			// simplify: restarting TS drops already-mounted .ts clients until that view remounts. Upgrade: reattach those views.
			if resolved.Key != "typescript-language-server" || s.VuePluginPath != "" || plugin == "" {
				return sessionInfo(s), nil
			}
			delete(m.byKey, key)
			delete(m.sessions, id)
			m.mu.Unlock()
			s.Close()
			m.mu.Lock()
		} else {
			delete(m.byKey, key)
			delete(m.sessions, id)
		}
	}

	id := uuid.NewString()
	rootPath = CanonPath(rootPath)
	rootURI := PathToURI(rootPath)
	s, err := startSession(id, workspaceID, rootPath, rootURI, resolved, m.wrapEmit(id, key))
	if err != nil {
		return SessionInfo{}, fmt.Errorf("start %s: %w", resolved.Key, err)
	}
	if resolved.Key == "typescript-language-server" || resolved.Key == "vue-language-server" {
		s.VuePluginPath = plugin
	}
	m.sessions[id] = s
	m.byKey[key] = id
	return sessionInfo(s), nil
}

func sessionInfo(s *Session) SessionInfo {
	return SessionInfo{
		ID:            s.ID,
		ServerKey:     s.ServerKey,
		RootURI:       s.RootURI,
		RootPath:      s.RootPath,
		Available:     true,
		VuePluginPath: s.VuePluginPath,
	}
}

// wrapEmit drops bookkeeping when a session exits.
func (m *Manager) wrapEmit(id, key string) Emitter {
	return func(name string, data any) {
		if name == "lsp.exit" {
			m.mu.Lock()
			if m.byKey[key] == id {
				delete(m.byKey, key)
			}
			delete(m.sessions, id)
			m.mu.Unlock()
		}
		if m.emit != nil {
			m.emit(name, data)
		}
	}
}

// Write forwards a JSON-RPC body to the session.
func (m *Manager) Write(id, message string) error {
	m.mu.Lock()
	s := m.sessions[id]
	m.mu.Unlock()
	if s == nil {
		return fmt.Errorf("unknown lsp session")
	}
	return s.Write(message)
}

// Close stops one session.
func (m *Manager) Close(id string) {
	m.mu.Lock()
	s := m.sessions[id]
	if s != nil {
		delete(m.sessions, id)
		key := sessionKey(s.WorkspaceID, s.ServerKey)
		if m.byKey[key] == id {
			delete(m.byKey, key)
		}
	}
	m.mu.Unlock()
	if s != nil {
		s.Close()
	}
}

// CloseWorkspace stops every session for a workspace.
func (m *Manager) CloseWorkspace(workspaceID string) {
	m.mu.Lock()
	var victims []*Session
	for id, s := range m.sessions {
		if s.WorkspaceID == workspaceID {
			victims = append(victims, s)
			delete(m.sessions, id)
			delete(m.byKey, sessionKey(s.WorkspaceID, s.ServerKey))
		}
	}
	m.mu.Unlock()
	for _, s := range victims {
		s.Close()
	}
}

// CloseServers stops every session whose ServerKey is in keys (Settings disable).
func (m *Manager) CloseServers(keys []string) {
	want := make(map[string]bool, len(keys))
	for _, k := range keys {
		if k = strings.TrimSpace(k); k != "" {
			want[k] = true
		}
	}
	if len(want) == 0 {
		return
	}
	m.mu.Lock()
	var victims []*Session
	for id, s := range m.sessions {
		if !want[s.ServerKey] {
			continue
		}
		victims = append(victims, s)
		delete(m.sessions, id)
		delete(m.byKey, sessionKey(s.WorkspaceID, s.ServerKey))
	}
	m.mu.Unlock()
	for _, s := range victims {
		s.Close()
	}
}

// CloseAll stops every session (app shutdown).
func (m *Manager) CloseAll() {
	m.mu.Lock()
	victims := make([]*Session, 0, len(m.sessions))
	for id, s := range m.sessions {
		victims = append(victims, s)
		delete(m.sessions, id)
	}
	m.byKey = make(map[string]string)
	m.mu.Unlock()
	for _, s := range victims {
		s.Close()
	}
}
