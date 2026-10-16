// Package store holds Recur's persisted state, backed by SQLite
// (modernc.org/sqlite, pure-Go). The in-memory fields are the
// live working set the engine and bound services read; mutations write through
// to the database so they survive an app restart.
//
// On first run with an empty database the store writes default policy and
// settings; later runs load from disk. The mock dataset is test-only
// (NewInMemory and the persistence-test helper), not a product path.
package store

import (
	"encoding/json"
	"fmt"
	"strconv"
	"strings"
	"sync"
	"time"

	"github.com/google/uuid"

	"recur/internal/domain"
	"recur/internal/projfile"
	"recur/internal/shortid"
)

// Store is the single source of truth for loops, PRs, projects, inbox, policy, etc.
// All access is mutex-guarded so the engine goroutines and bound API calls are safe.
type Store struct {
	mu sync.RWMutex

	// feedMu serializes the loop-feed writers (PrependFeed / PatchFeedByToolUseID)
	// so their SQLite writes happen in the same order as their in-memory edits,
	// WITHOUT holding mu across the database call. mu is the store-wide lock —
	// every bound service call takes it to look up a loop or project — so a feed
	// write that held it across disk I/O stalled the entire UI, and an agent
	// streams feed items continuously. Lock order is always feedMu → mu.
	feedMu sync.Mutex

	db *db // SQLite backing; nil only in the legacy in-memory path (NewInMemory)

	projectGroups     []domain.ProjectGroup
	projects          []domain.Project
	workspaces        []domain.Workspace
	agents            map[string][]domain.Agent // workspaceID -> agents
	feed              map[string][]domain.FeedItem
	findings          map[string][]domain.Finding // workspaceID -> findings (PRD §8.3, §14)
	prs               []domain.PR
	prNumberSeq       int                          // last issued Local PR number; never decreases
	escalations       map[string]domain.Escalation // workspaceID -> escalation
	inbox             []domain.InboxItem
	analysis          []domain.AnalysisRun // observatory runs, newest first
	policy            domain.Policy
	settings          domain.Settings
	resource          domain.Resource
	githubBranchRules map[string]githubBranchRule // projectID\0repo\0branch
}

// New opens the SQLite-backed store under the managed root (PRD §14). On first
// run (empty db) it writes default policy/settings; on later runs it loads the
// persisted state. The managed root is overridable via $RECUR_DATA_DIR (used
// by tests).
func New() (*Store, error) {
	root, err := DataDir()
	if err != nil {
		return nil, fmt.Errorf("resolve data dir: %w", err)
	}
	return open(DBPath(root))
}

// open backs New and the tests; it opens/migrates the db at path and either
// seeds (if empty) or loads.
func open(path string) (*Store, error) {
	d, err := openDB(path)
	if err != nil {
		return nil, err
	}
	s := newEmpty()
	s.db = d

	fresh, err := d.needsSeed()
	if err != nil {
		d.close()
		return nil, err
	}
	if fresh {
		// Brand-new DB: always write the default policy/settings (+ a ZEROED resource),
		// so the app has sane config and a clean status bar. The mock dataset is
		// never written here — tests call seedDemo themselves (openTemp / NewInMemory).
		s.seedDefaults()
		if err := d.seedAll(s); err != nil {
			d.close()
			return nil, fmt.Errorf("seed db: %w", err)
		}
	} else if err := d.loadAll(s); err != nil {
		d.close()
		return nil, fmt.Errorf("load db: %w", err)
	}
	return s, nil
}

// NewInMemory returns a seeded store with no persistence — used by tests and
// callers that only need the M0 dataset without touching disk.
func NewInMemory() *Store {
	s := newEmpty()
	s.seed()
	return s
}

func newEmpty() *Store {
	return &Store{
		agents:            map[string][]domain.Agent{},
		feed:              map[string][]domain.FeedItem{},
		findings:          map[string][]domain.Finding{},
		escalations:       map[string]domain.Escalation{},
		githubBranchRules: map[string]githubBranchRule{},
	}
}

// Close releases the database. Safe to call on an in-memory store (no-op).
func (s *Store) Close() error {
	if s.db == nil {
		return nil
	}
	return s.db.close()
}

// ---- Project groups ----

func (s *Store) ProjectGroups() []domain.ProjectGroup {
	s.mu.RLock()
	defer s.mu.RUnlock()
	return append([]domain.ProjectGroup(nil), s.projectGroups...)
}

func (s *Store) ProjectGroup(id string) (domain.ProjectGroup, bool) {
	s.mu.RLock()
	defer s.mu.RUnlock()
	for _, g := range s.projectGroups {
		if g.ID == id {
			return g, true
		}
	}
	return domain.ProjectGroup{}, false
}

// HasProjectGroup reports whether a project group with the given id exists.
func (s *Store) HasProjectGroup(id string) bool {
	s.mu.RLock()
	defer s.mu.RUnlock()
	for _, g := range s.projectGroups {
		if g.ID == id {
			return true
		}
	}
	return false
}

// AddProjectGroup persists a new group (or updates an existing id in place).
func (s *Store) AddProjectGroup(g domain.ProjectGroup) (domain.ProjectGroup, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	idx := -1
	for i := range s.projectGroups {
		if s.projectGroups[i].ID == g.ID {
			idx = i
			break
		}
	}
	writeIdx := idx
	if writeIdx < 0 {
		writeIdx = len(s.projectGroups)
	}
	now := time.Now().UTC().Format(time.RFC3339)
	if idx >= 0 {
		if g.CreatedAt == "" {
			g.CreatedAt = s.projectGroups[idx].CreatedAt
		}
	} else if g.CreatedAt == "" {
		g.CreatedAt = now
	}
	g.UpdatedAt = now
	if s.db != nil {
		if err := s.db.upsertProjectGroup(g, writeIdx); err != nil {
			return domain.ProjectGroup{}, err
		}
	}
	if idx >= 0 {
		s.projectGroups[idx] = g
	} else {
		s.projectGroups = append(s.projectGroups, g)
	}
	return g, nil
}

// UpdateProjectGroup persists edits to an existing group.
func (s *Store) UpdateProjectGroup(g domain.ProjectGroup) (domain.ProjectGroup, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	for i := range s.projectGroups {
		if s.projectGroups[i].ID == g.ID {
			g.CreatedAt = s.projectGroups[i].CreatedAt
			g.UpdatedAt = time.Now().UTC().Format(time.RFC3339)
			if s.db != nil {
				if err := s.db.upsertProjectGroup(g, i); err != nil {
					return domain.ProjectGroup{}, err
				}
			}
			s.projectGroups[i] = g
			return g, nil
		}
	}
	return domain.ProjectGroup{}, fmt.Errorf("project group %q not found", g.ID)
}

// DeleteProjectGroup removes a group and clears group_id on its member projects.
func (s *Store) DeleteProjectGroup(id string) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	idx := -1
	for i := range s.projectGroups {
		if s.projectGroups[i].ID == id {
			idx = i
			break
		}
	}
	if idx < 0 {
		return nil
	}
	if s.db != nil {
		if err := s.db.deleteProjectGroupAndUngroup(id); err != nil {
			return err
		}
	}
	for i := range s.projects {
		if s.projects[i].GroupID == id {
			s.projects[i].GroupID = ""
		}
	}
	s.projectGroups = append(s.projectGroups[:idx], s.projectGroups[idx+1:]...)
	return nil
}

// ReorderProjectGroups persists a new sidebar group order. ids must be a
// permutation of the current project group ids.
func (s *Store) ReorderProjectGroups(ids []string) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	if len(ids) != len(s.projectGroups) {
		return fmt.Errorf("reorder groups: got %d ids, have %d groups", len(ids), len(s.projectGroups))
	}
	byID := make(map[string]domain.ProjectGroup, len(s.projectGroups))
	for _, g := range s.projectGroups {
		byID[g.ID] = g
	}
	next := make([]domain.ProjectGroup, 0, len(ids))
	seen := make(map[string]bool, len(ids))
	for _, id := range ids {
		g, ok := byID[id]
		if !ok {
			return fmt.Errorf("reorder groups: unknown group %q", id)
		}
		if seen[id] {
			return fmt.Errorf("reorder groups: duplicate group %q", id)
		}
		seen[id] = true
		next = append(next, g)
	}
	if s.db != nil {
		if err := s.db.setProjectGroupSorts(ids); err != nil {
			return err
		}
	}
	s.projectGroups = next
	return nil
}

// ---- Projects ----

func (s *Store) Projects() []domain.Project {
	s.mu.RLock()
	out := append([]domain.Project(nil), s.projects...)
	s.mu.RUnlock()
	for i := range out {
		out[i] = projfile.Overlay(out[i])
	}
	return out
}

func (s *Store) Project(id string) (domain.Project, bool) {
	s.mu.RLock()
	var p domain.Project
	ok := false
	for _, cand := range s.projects {
		if cand.ID == id {
			p, ok = cand, true
			break
		}
	}
	s.mu.RUnlock()
	if !ok {
		return domain.Project{}, false
	}
	return projfile.Overlay(p), true
}

// withValidPinnedRun keeps a pin only when that command still exists.
func withValidPinnedRun(p domain.Project) domain.Project {
	pin := strings.TrimSpace(p.PinnedRun)
	if pin == "" {
		p.PinnedRun = ""
		return p
	}
	for _, r := range domain.EffectiveRuns(p) {
		if r.Name == pin {
			p.PinnedRun = pin
			return p
		}
	}
	p.PinnedRun = ""
	return p
}

// persistProjectFile writes .recur/project.toml from p's run-config, creating
// or replacing the file. Callers set WriteProjectFile only on an explicit
// settings save — sidebar/pin/skills updates must not rewrite the file.
func persistProjectFile(p domain.Project) error {
	return projfile.Write(p)
}

// AddProject persists a new project and returns the saved copy. The caller is
// expected to have assigned a stable ID/slug. If the ID already exists the
// project is updated in place rather than duplicated.
//
// Run-config is read from <repo>/.recur/project.toml. WriteProjectFile
// creates or replaces that file from the submitted fields.
func (s *Store) AddProject(p domain.Project) (domain.Project, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	writeFile := p.WriteProjectFile
	p.WriteProjectFile = false
	if writeFile {
		if err := persistProjectFile(p); err != nil {
			return domain.Project{}, err
		}
	}
	p = withValidPinnedRun(p)
	idx := -1
	for i := range s.projects {
		if s.projects[i].ID == p.ID {
			idx = i
			break
		}
	}
	writeIdx := idx
	if writeIdx < 0 {
		writeIdx = len(s.projects)
	}
	if s.db != nil {
		if err := s.db.upsertProject(p, writeIdx); err != nil {
			return domain.Project{}, err
		}
	}
	if idx >= 0 {
		s.projects[idx] = p
	} else {
		s.projects = append(s.projects, p)
	}
	// Overlay on the way out so HasProjectFile / [[run]] match List/Get.
	// The in-memory + DB copy stays the machine-local record.
	return projfile.Overlay(p), nil
}

// UpdateProjectRunConfig persists machine-local edits to an existing project.
// WriteProjectFile creates or replaces .recur/project.toml from the submitted
// run-config. The returned copy is overlaid from that file.
func (s *Store) UpdateProjectRunConfig(p domain.Project) (domain.Project, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	for i := range s.projects {
		if s.projects[i].ID == p.ID {
			writeFile := p.WriteProjectFile
			p.WriteProjectFile = false
			p = withValidPinnedRun(p)
			if writeFile {
				if err := persistProjectFile(p); err != nil {
					return domain.Project{}, err
				}
			}
			if s.db != nil {
				if err := s.db.upsertProject(p, i); err != nil {
					return domain.Project{}, err
				}
			}
			s.projects[i] = p
			return projfile.Overlay(p), nil
		}
	}
	return domain.Project{}, fmt.Errorf("project %q not found", p.ID)
}

// HasProject reports whether a project with the given id already exists.
func (s *Store) HasProject(id string) bool {
	s.mu.RLock()
	defer s.mu.RUnlock()
	for _, p := range s.projects {
		if p.ID == id {
			return true
		}
	}
	return false
}

// ReorderProjects persists a new sidebar order. ids must be a permutation of the
// current project ids. The DB write happens FIRST; the in-memory slice is rebuilt
// only after it succeeds, so a write failure leaves order unchanged.
func (s *Store) ReorderProjects(ids []string) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	if len(ids) != len(s.projects) {
		return fmt.Errorf("reorder: got %d ids, have %d projects", len(ids), len(s.projects))
	}
	byID := make(map[string]domain.Project, len(s.projects))
	for _, p := range s.projects {
		byID[p.ID] = p
	}
	next := make([]domain.Project, 0, len(ids))
	seen := make(map[string]bool, len(ids))
	for _, id := range ids {
		p, ok := byID[id]
		if !ok {
			return fmt.Errorf("reorder: unknown project %q", id)
		}
		if seen[id] {
			return fmt.Errorf("reorder: duplicate project %q", id)
		}
		seen[id] = true
		next = append(next, p)
	}
	if s.db != nil {
		if err := s.db.setProjectSorts(ids); err != nil {
			return err
		}
	}
	s.projects = next
	return nil
}

// DeleteProject removes a project and CASCADES: every loop in the project and all of
// its dependent data (agents, feed, findings, events, escalations, PRs) plus any
// inbox rows pointing at those loops/PRs. Returns the deleted loop IDs. A missing
// project is a no-op. Policy/settings/resource are untouched.
func (s *Store) DeleteProject(id string) ([]string, error) {
	s.mu.Lock()
	defer s.mu.Unlock()

	var workspaceIDs []string
	for _, l := range s.workspaces {
		if l.ProjectID == id {
			workspaceIDs = append(workspaceIDs, l.ID)
		}
	}
	for _, lid := range workspaceIDs {
		if err := s.deleteWorkspaceLocked(lid); err != nil {
			return nil, err
		}
	}

	if s.db != nil {
		if err := s.db.deleteDoc("projects", id); err != nil {
			return nil, err
		}
	}
	var keptProjects []domain.Project
	for _, p := range s.projects {
		if p.ID != id {
			keptProjects = append(keptProjects, p)
		}
	}
	s.projects = keptProjects
	for k := range s.githubBranchRules {
		if strings.HasPrefix(k, id+"\x00") {
			delete(s.githubBranchRules, k)
		}
	}
	return workspaceIDs, nil
}

// deleteWorkspaceLocked removes one loop and everything scoped to it — its PRs, agents,
// feed, findings, durable events, escalation, and any inbox rows pointing at it or
// one of its PRs — from the DB (DB-first; on error the in-memory cache is left
// unchanged) and then from in-memory state. Caller must hold s.mu.
func (s *Store) deleteWorkspaceLocked(id string) error {
	prIDs := map[string]bool{}
	var keptPRs []domain.PR
	for _, p := range s.prs {
		if p.WorkspaceID == id {
			prIDs[p.ID] = true
		} else {
			keptPRs = append(keptPRs, p)
		}
	}
	var keptInbox []domain.InboxItem
	var deletedInbox []string
	for _, it := range s.inbox {
		if inboxTargetsDeletedWorkspace(it.Target, id, prIDs) {
			deletedInbox = append(deletedInbox, it.ID)
		} else {
			keptInbox = append(keptInbox, it)
		}
	}

	if s.db != nil {
		if err := s.db.deleteDoc("workspaces", id); err != nil {
			return err
		}
		// "session" cascades message/message_content/session_context via the
		// ON DELETE CASCADE relations (0002) — foreign_keys is ON per connection
		// and the pool is capped at one connection.
		for _, t := range []string{"agents", "feed", "findings", "events", "session"} {
			if err := s.db.deleteByWorkspace(t, id); err != nil {
				return err
			}
		}
		if err := s.db.deleteEscalations(id); err != nil {
			return err
		}
		for pid := range prIDs {
			if err := s.db.deleteDoc("prs", pid); err != nil {
				return err
			}
		}
		for _, iid := range deletedInbox {
			if err := s.db.deleteDoc("inbox", iid); err != nil {
				return err
			}
		}
	}

	var keptLoops []domain.Workspace
	for _, l := range s.workspaces {
		if l.ID != id {
			keptLoops = append(keptLoops, l)
		}
	}
	s.workspaces = keptLoops
	s.prs = keptPRs
	s.inbox = keptInbox
	delete(s.agents, id)
	delete(s.feed, id)
	delete(s.findings, id)
	delete(s.escalations, id)
	prefix := id + escalationKeySep
	for k := range s.escalations {
		if strings.HasPrefix(k, prefix) {
			delete(s.escalations, k)
		}
	}
	return nil
}

// DeleteWorkspace removes a single loop and its cascade (PRs, agents, feed, findings,
// events, escalation, inbox rows) — the per-loop counterpart to DeleteProject, used
// by hard delete / cancel paths (engine.DeleteWorkspace enforces the not-running
// precondition before calling this). RemoveWorkspace keeps the store row.
func (s *Store) DeleteWorkspace(id string) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	found := false
	for _, l := range s.workspaces {
		if l.ID == id {
			found = true
			break
		}
	}
	if !found {
		return fmt.Errorf("loop %q not found", id)
	}
	return s.deleteWorkspaceLocked(id)
}

// ClearAll wipes ALL data (projects, loops, PRs, inbox, agents, feed, findings,
// escalations, events) and resets the status-bar resource to zero, but KEEPS the
// policy, settings, and PR-number high-water mark. The app looks like a clean
// first-run except Local PR numbers keep climbing (workspace deletes must not
// reclaim them). The demo dataset does NOT come back (the DB stays initialized).
func (s *Store) ClearAll() error {
	s.mu.Lock()
	defer s.mu.Unlock()
	if s.db != nil {
		if err := s.db.clearData(); err != nil {
			return err
		}
	}
	s.projects = nil
	s.workspaces = nil
	s.prs = nil
	// prNumberSeq is kept: workspace/project deletes (and ClearAll) must not
	// reclaim PR numbers. The next Local PR after a wipe still goes up.
	s.inbox = nil
	s.agents = map[string][]domain.Agent{}
	s.feed = map[string][]domain.FeedItem{}
	s.findings = map[string][]domain.Finding{}
	s.escalations = map[string]domain.Escalation{}
	s.githubBranchRules = map[string]githubBranchRule{}
	s.resource = domain.Resource{}
	if s.db != nil {
		_ = s.db.upsertSingleton("resource", s.resource)
	}
	return nil
}

// ---- Loops ----

func (s *Store) Workspaces() []domain.Workspace {
	s.mu.RLock()
	defer s.mu.RUnlock()
	out := make([]domain.Workspace, len(s.workspaces))
	for i := range s.workspaces {
		out[i] = cloneWorkspace(s.workspaces[i])
	}
	return out
}

func (s *Store) Workspace(id string) (domain.Workspace, bool) {
	s.mu.RLock()
	defer s.mu.RUnlock()
	for _, l := range s.workspaces {
		if l.ID == id {
			return cloneWorkspace(l), true
		}
	}
	return domain.Workspace{}, false
}

// UpdateWorkspace applies a mutation under lock; used by the (fake, M0) emitter and the
// real engine (M2+). Returns the updated copy and whether it was found.
func (s *Store) UpdateWorkspace(id string, fn func(*domain.Workspace)) (domain.Workspace, bool) {
	s.mu.Lock()
	defer s.mu.Unlock()
	for i := range s.workspaces {
		if s.workspaces[i].ID == id {
			fn(&s.workspaces[i])
			if s.db != nil {
				// Best-effort write-through; a persistence failure must not lose
				// the in-memory update or panic an engine goroutine.
				_ = s.db.upsertDoc("workspaces", id, s.workspaces[i], i)
			}
			return s.workspaces[i], true
		}
	}
	return domain.Workspace{}, false
}

// ---- Agents ----

func (s *Store) Agents(workspaceID string) []domain.Agent {
	s.mu.RLock()
	defer s.mu.RUnlock()
	out := make([]domain.Agent, len(s.agents[workspaceID]))
	for i, a := range s.agents[workspaceID] {
		out[i] = cloneAgent(a)
	}
	return out
}

func (s *Store) Agent(workspaceID, code string) (domain.Agent, bool) {
	s.mu.RLock()
	defer s.mu.RUnlock()
	for _, a := range s.agents[workspaceID] {
		if a.Code == code || a.ID == code {
			return cloneAgent(a), true
		}
	}
	return domain.Agent{}, false
}

// UpsertAgent inserts or replaces an agent for a loop (engine spawn, M2),
// persisting the loop's agent list. Matched by Code.
func (s *Store) UpsertAgent(workspaceID string, a domain.Agent) {
	s.mu.Lock()
	defer s.mu.Unlock()
	list := s.agents[workspaceID]
	for i := range list {
		if list[i].Code == a.Code {
			list[i] = a
			s.agents[workspaceID] = list
			s.persistAgents(workspaceID)
			return
		}
	}
	s.agents[workspaceID] = append(list, a)
	s.persistAgents(workspaceID)
}

// UpdateAgent applies a mutation to a loop's agent (matched by Code) under lock,
// persisting the result. No-op if the agent isn't found.
func (s *Store) UpdateAgent(workspaceID, code string, fn func(*domain.Agent)) {
	s.mu.Lock()
	defer s.mu.Unlock()
	list := s.agents[workspaceID]
	for i := range list {
		if list[i].Code == code {
			fn(&list[i])
			s.agents[workspaceID] = list
			s.persistAgents(workspaceID)
			return
		}
	}
}

// DeleteAgent removes an agent (matched by Code) from a workspace. No-op if
// missing. Does not touch the durable session transcript.
func (s *Store) DeleteAgent(workspaceID, code string) {
	s.mu.Lock()
	defer s.mu.Unlock()
	list := s.agents[workspaceID]
	for i := range list {
		if list[i].Code == code {
			s.agents[workspaceID] = append(list[:i], list[i+1:]...)
			s.persistAgents(workspaceID)
			return
		}
	}
}

// MoveWorkspaceAgents relocates chat-tab agent rows from fromID onto toID.
// remap rewrites source codes that collided with a dest transcript. Dest
// placeholder tabs (same code, no provider session) take the source's
// resumable fields; clones get dest identity (ID + WorkspaceID).
func (s *Store) MoveWorkspaceAgents(fromID, toID string, remap map[string]string) {
	fromID = strings.TrimSpace(fromID)
	toID = strings.TrimSpace(toID)
	if fromID == "" || toID == "" || fromID == toID {
		return
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	from := s.agents[fromID]
	if len(from) == 0 {
		return
	}
	dest := append([]domain.Agent(nil), s.agents[toID]...)
	have := make(map[string]int, len(dest))
	for i, a := range dest {
		have[a.Code] = i
	}
	for _, a := range from {
		a = cloneAgent(a)
		if next := remap[a.Code]; next != "" {
			a.Code = next
		}
		a.WorkspaceID = toID
		a.ID = toID + "-" + a.Code
		if i, ok := have[a.Code]; ok {
			dest[i] = mergeHoistedAgent(dest[i], a)
			continue
		}
		have[a.Code] = len(dest)
		dest = append(dest, a)
	}
	s.agents[toID] = dest
	delete(s.agents, fromID)
	s.persistAgents(toID)
	s.persistAgents(fromID)
}

// ReassignWorkspaceFindings moves reviewer findings from fromID onto toID so a
// PR-host hoist does not leave report_finding rows on the archived host.
func (s *Store) ReassignWorkspaceFindings(fromID, toID string) int {
	fromID = strings.TrimSpace(fromID)
	toID = strings.TrimSpace(toID)
	if fromID == "" || toID == "" || fromID == toID {
		return 0
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	src := s.findings[fromID]
	if len(src) == 0 {
		return 0
	}
	dest := s.findings[toID]
	sort := len(dest)
	for i := range src {
		src[i].WorkspaceID = toID
		dest = append(dest, src[i])
		if s.db != nil {
			_ = s.db.upsertFinding(src[i], sort)
		}
		sort++
	}
	s.findings[toID] = dest
	delete(s.findings, fromID)
	return len(src)
}

func mergeHoistedAgent(dst, src domain.Agent) domain.Agent {
	out := cloneAgent(dst)
	if src.SessionID != "" {
		out.SessionID = src.SessionID
		if src.Model != "" {
			out.Model = src.Model
		}
		if src.Status != "" {
			out.Status = src.Status
		}
		if src.Name != "" {
			out.Name = src.Name
		}
	}
	out.Unseen = dst.Unseen || src.Unseen
	return out
}

// persistAgents writes the loop's agent list through to the db. Caller holds mu.
func (s *Store) persistAgents(workspaceID string) {
	if s.db != nil {
		_ = s.db.replaceList("agents", workspaceID, toAny(s.agents[workspaceID]))
	}
}

// CountGroup returns how many loops are in a list bucket (PRD §16 groups).
func (s *Store) CountGroup(g domain.Group) int {
	s.mu.RLock()
	defer s.mu.RUnlock()
	n := 0
	for _, l := range s.workspaces {
		if l.Group == g {
			n++
		}
	}
	return n
}

// ---- Activity feed ----

func (s *Store) Feed(workspaceID string) []domain.FeedItem {
	s.mu.RLock()
	defer s.mu.RUnlock()
	return append([]domain.FeedItem(nil), s.feed[workspaceID]...)
}

// PrependFeed adds a new activity item to the front (newest first).
//
// The database write is a single INSERT and happens outside the store-wide lock
// (see Store.feedMu): agents stream feed items constantly, and this used to
// hold the global write lock while deleting and re-inserting every row the loop
// owned — blocking every reader, including the whole code-browser surface.
func (s *Store) PrependFeed(workspaceID string, item domain.FeedItem) {
	s.feedMu.Lock()
	defer s.feedMu.Unlock()

	s.mu.Lock()
	// Shift in place instead of allocating a fresh slice per item: append grows
	// the backing array amortized, and the shift is one memmove.
	cur := append(s.feed[workspaceID], domain.FeedItem{})
	copy(cur[1:], cur)
	cur[0] = item
	s.feed[workspaceID] = cur
	s.mu.Unlock()

	if s.db != nil {
		_ = s.db.prependFeedItem(workspaceID, item)
	}
}

// UpsertFeed replaces the feed item with item.ID in place, or prepends it when
// absent. Typed cards (FeedItem.Card) re-emit under one ID as they progress, so
// the Activity feed shows one row per card, not one per update.
func (s *Store) UpsertFeed(workspaceID string, item domain.FeedItem) {
	s.feedMu.Lock()
	defer s.feedMu.Unlock()

	s.mu.Lock()
	items := s.feed[workspaceID]
	replaced := false
	for i := range items {
		if items[i].ID == item.ID {
			items[i] = item
			replaced = true
			break
		}
	}
	var snapshot []domain.FeedItem
	if replaced {
		snapshot = append([]domain.FeedItem(nil), items...)
	} else {
		cur := append(items, domain.FeedItem{})
		copy(cur[1:], cur)
		cur[0] = item
		s.feed[workspaceID] = cur
	}
	s.mu.Unlock()

	if s.db == nil {
		return
	}
	if replaced {
		_ = s.db.replaceList("feed", workspaceID, toAny(snapshot))
	} else {
		_ = s.db.prependFeedItem(workspaceID, item)
	}
}

// PatchFeedByToolUseID updates the feed item that recorded toolUseID (in place)
// and returns a copy of the patched item, or nil if none matched.
//
// Unlike a prepend this still rewrites the loop's rows — an edit in the middle
// of the list has no single-row identity to UPDATE — but the rewrite happens
// off the store-wide lock, and it renumbers sort to 0..N-1, which stays
// consistent with prepends (those only ever use negative sort values).
func (s *Store) PatchFeedByToolUseID(workspaceID, toolUseID string, fn func(*domain.FeedItem)) *domain.FeedItem {
	if toolUseID == "" || fn == nil {
		return nil
	}
	s.feedMu.Lock()
	defer s.feedMu.Unlock()

	var out *domain.FeedItem
	var snapshot []domain.FeedItem
	s.mu.Lock()
	items := s.feed[workspaceID]
	for i := range items {
		if items[i].ToolUseID != toolUseID {
			continue
		}
		fn(&items[i])
		s.feed[workspaceID] = items
		patched := items[i]
		out = &patched
		snapshot = append([]domain.FeedItem(nil), items...)
		break
	}
	s.mu.Unlock()

	if out == nil {
		return nil
	}
	if s.db != nil {
		_ = s.db.replaceList("feed", workspaceID, toAny(snapshot))
	}
	return out
}

// ---- durable events (append-only; PRD §6.5, §14; DECISIONS D13) ----

// AppendEvent persists one durable event to the append-only log and returns the
// assigned row id. Append-only: the row is never updated or deleted, so a reload
// can reconstruct the activity feed and history (PRD §6.5: "a reload reconstructs
// the truth"). On an in-memory store (no db) it is a no-op returning 0.
func (s *Store) AppendEvent(e domain.StoredEvent) (int64, error) {
	if s.db == nil {
		return 0, nil
	}
	// No store-mutex needed: the events table is append-only and the db serializes
	// writes on its single connection. Taking the store lock here would risk a
	// self-deadlock if ever called from within an UpdateWorkspace callback.
	return s.db.appendEvent(e)
}

// EventsForLoop returns every durable event recorded for a loop, oldest first.
// Used to reconstruct the activity feed on reload.
func (s *Store) EventsForLoop(workspaceID string) ([]domain.StoredEvent, error) {
	if s.db == nil {
		return nil, nil
	}
	return s.db.eventsForWorkspace(workspaceID)
}

// AddWorkspace registers a brand-new loop (engine StartWorkspace, M2). The DB write happens
// first; the in-memory cache is appended only on success. Loops are stored in
// insertion order (oldest first, newest appended at the end) so the engine can
// drain the OLDEST queued loop first. ListWorkspaces reverses that for the UI.
// An existing ID is treated as an in-place update so AddWorkspace is idempotent.
func (s *Store) AddWorkspace(l domain.Workspace) (domain.Workspace, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	for i := range s.workspaces {
		if s.workspaces[i].ID == l.ID {
			// Already present — treat as an update so AddWorkspace is idempotent.
			if s.db != nil {
				if err := s.db.upsertDoc("workspaces", l.ID, l, i); err != nil {
					return domain.Workspace{}, err
				}
			}
			s.workspaces[i] = l
			return cloneWorkspace(l), nil
		}
	}
	if s.db != nil {
		if err := s.db.upsertDoc("workspaces", l.ID, l, len(s.workspaces)); err != nil {
			return domain.Workspace{}, err
		}
	}
	s.workspaces = append(s.workspaces, l)
	return cloneWorkspace(l), nil
}

// ---- Findings (reviewer findings; PRD §8.3, §14) ----

// ensureFindingCreatedAt fills CreatedAt when missing so the Findings panel age
// column works for legacy rows (pre-createdAt) and IDs stamped as F-…-<unixNano>.
func ensureFindingCreatedAt(f *domain.Finding) {
	if f == nil || f.CreatedAt != "" {
		return
	}
	if f.UpdatedAt != "" {
		f.CreatedAt = f.UpdatedAt
		return
	}
	if i := strings.LastIndex(f.ID, "-"); i >= 0 {
		if n, err := strconv.ParseInt(f.ID[i+1:], 10, 64); err == nil && n > 1_000_000_000_000_000 {
			f.CreatedAt = time.Unix(0, n).UTC().Format(time.RFC3339)
		}
	}
}

// Findings returns a loop's findings in insertion order (oldest first).
func (s *Store) Findings(workspaceID string) []domain.Finding {
	s.mu.RLock()
	defer s.mu.RUnlock()
	src := s.findings[workspaceID]
	out := make([]domain.Finding, len(src))
	copy(out, src)
	for i := range out {
		ensureFindingCreatedAt(&out[i])
	}
	return out
}

// AddFinding records a new reviewer finding for a loop, persisting it. The
// engine stamps iteration/status; the store assigns a UUID and a public
// find_… short id (unique per workspace, collision-retried on insert).
// Returns the stored copy.
func (s *Store) AddFinding(f domain.Finding) domain.Finding {
	s.mu.Lock()
	defer s.mu.Unlock()
	if f.CreatedAt == "" {
		f.CreatedAt = time.Now().UTC().Format(time.RFC3339)
	}
	if f.ID == "" {
		f.ID = uuid.Must(uuid.NewV7()).String()
	}
	list := s.findings[f.WorkspaceID]
	ord := len(list)
	assignFindingShortID(&f, list)
	if s.db != nil {
		var err error
		for range shortid.DefaultAttempts {
			err = s.db.upsertFinding(f, ord)
			if err == nil || !isUniqueConstraint(err) {
				break
			}
			f.ShortID = ""
			assignFindingShortID(&f, list)
		}
		if err != nil {
			return f
		}
	}
	s.findings[f.WorkspaceID] = append(list, f)
	return f
}

// assignFindingShortID fills ShortID when missing or already taken in list.
func assignFindingShortID(f *domain.Finding, list []domain.Finding) {
	taken := func(id string) bool {
		for _, existing := range list {
			if existing.ShortID == id {
				return true
			}
		}
		return false
	}
	if f.ShortID != "" && !taken(f.ShortID) {
		return
	}
	if sid, err := domain.NewUniqueID(domain.TypeFinding, taken); err == nil {
		f.ShortID = sid
	}
}

// ResolveFindings marks every still-open finding for a loop resolved (engine bulk
// clear — kept for rails/tests and rare resume cleanup). Ignored/invalid findings
// are left alone. Prefer coder update_finding in the live convergence loop.
func (s *Store) ResolveFindings(workspaceID string) {
	s.mu.Lock()
	defer s.mu.Unlock()
	now := time.Now().UTC().Format(time.RFC3339)
	list := s.findings[workspaceID]
	for i := range list {
		if list[i].Status == string(domain.FindingOpen) {
			list[i].Status = string(domain.FindingResolved)
			list[i].UpdatedBy = domain.FindingActorEngine
			list[i].UpdatedAt = now
			list[i].StatusReason = ""
			if s.db != nil {
				_ = s.db.upsertFinding(list[i], i)
			}
		}
	}
	s.findings[workspaceID] = list
}

// SetFindingStatus flips one finding's status (human Ignore / Mark invalid / Reopen).
// Returns the updated finding, or an error if the id/status is unknown.
func (s *Store) SetFindingStatus(workspaceID, findingID, status string) (domain.Finding, error) {
	return s.UpdateFindingStatus(workspaceID, findingID, status, "", domain.FindingActorHuman)
}

// AddFindingNote appends an operator comment to a finding. body must be non-empty
// after trim. author defaults to human when empty.
func (s *Store) AddFindingNote(workspaceID, findingID, body, author string) (domain.Finding, error) {
	body = strings.TrimSpace(body)
	if body == "" {
		return domain.Finding{}, fmt.Errorf("finding note body is required")
	}
	switch author {
	case domain.FindingActorHuman, domain.FindingActorCoder, domain.FindingActorEngine:
	case "":
		author = domain.FindingActorHuman
	default:
		return domain.Finding{}, fmt.Errorf("invalid finding note author %q", author)
	}

	s.mu.Lock()
	defer s.mu.Unlock()
	list := s.findings[workspaceID]
	for i := range list {
		if !list[i].MatchesRef(findingID) {
			continue
		}
		note := domain.FindingNote{
			ID:        uuid.Must(uuid.NewV7()).String(),
			Author:    author,
			Body:      body,
			CreatedAt: time.Now().UTC().Format(time.RFC3339),
		}
		list[i].Notes = append(list[i].Notes, note)
		if s.db != nil {
			_ = s.db.upsertFinding(list[i], i)
		}
		s.findings[workspaceID] = list
		return list[i], nil
	}
	return domain.Finding{}, fmt.Errorf("finding %q not found", findingID)
}

// DeleteFindingNote removes one operator note from a finding by note id.
func (s *Store) DeleteFindingNote(workspaceID, findingID, noteID string) (domain.Finding, error) {
	noteID = strings.TrimSpace(noteID)
	if noteID == "" {
		return domain.Finding{}, fmt.Errorf("finding note id is required")
	}

	s.mu.Lock()
	defer s.mu.Unlock()
	list := s.findings[workspaceID]
	for i := range list {
		if !list[i].MatchesRef(findingID) {
			continue
		}
		notes := list[i].Notes
		kept := make([]domain.FindingNote, 0, len(notes))
		found := false
		for _, n := range notes {
			if n.ID == noteID {
				found = true
				continue
			}
			kept = append(kept, n)
		}
		if !found {
			return domain.Finding{}, fmt.Errorf("finding note %q not found", noteID)
		}
		if len(kept) == 0 {
			list[i].Notes = nil
		} else {
			list[i].Notes = kept
		}
		if s.db != nil {
			_ = s.db.upsertFinding(list[i], i)
		}
		s.findings[workspaceID] = list
		return list[i], nil
	}
	return domain.Finding{}, fmt.Errorf("finding %q not found", findingID)
}

// UpdateFindingStatus flips one finding's status with attribution. actor is
// human|coder|reviewer|engine. Coder and reviewer may only set resolved|invalid
// (invalid requires reason), and cannot mutate human-ignored findings or reopen
// to open. Engine may only resolve still-open findings; ignored/invalid are
// left intact. Idempotent: same status (+ same/empty reason) returns the row
// without error.
func (s *Store) UpdateFindingStatus(workspaceID, findingID, status, reason, actor string) (domain.Finding, error) {
	if !domain.ValidFindingStatus(status) {
		return domain.Finding{}, fmt.Errorf("invalid finding status %q", status)
	}
	reason = strings.TrimSpace(reason)
	switch actor {
	case domain.FindingActorHuman, domain.FindingActorCoder, domain.FindingActorReviewer, domain.FindingActorEngine:
	case "":
		actor = domain.FindingActorHuman
	default:
		return domain.Finding{}, fmt.Errorf("invalid finding actor %q", actor)
	}
	if actor == domain.FindingActorCoder || actor == domain.FindingActorReviewer {
		switch domain.FindingStatus(status) {
		case domain.FindingResolved, domain.FindingInvalid:
		default:
			return domain.Finding{}, fmt.Errorf("%s may only set status resolved or invalid, got %q", actor, status)
		}
		if status == string(domain.FindingInvalid) && reason == "" {
			return domain.Finding{}, fmt.Errorf("reason is required when marking a finding invalid")
		}
	}
	if actor == domain.FindingActorEngine && status != string(domain.FindingResolved) {
		return domain.Finding{}, fmt.Errorf("engine may only set status resolved, got %q", status)
	}

	s.mu.Lock()
	defer s.mu.Unlock()
	list := s.findings[workspaceID]
	for i := range list {
		if !list[i].MatchesRef(findingID) {
			continue
		}
		cur := list[i]
		if (actor == domain.FindingActorCoder || actor == domain.FindingActorReviewer) && cur.Status == string(domain.FindingIgnored) {
			return domain.Finding{}, fmt.Errorf("finding %q is ignored by the human; %s cannot change it", findingID, actor)
		}
		if actor == domain.FindingActorEngine && cur.Status != string(domain.FindingOpen) {
			return list[i], nil
		}
		// Idempotent same-status update (optionally refresh reason/attribution).
		if cur.Status == status {
			if reason != "" && reason != cur.StatusReason {
				list[i].StatusReason = reason
				list[i].UpdatedBy = actor
				list[i].UpdatedAt = time.Now().UTC().Format(time.RFC3339)
				if s.db != nil {
					_ = s.db.upsertFinding(list[i], i)
				}
				s.findings[workspaceID] = list
			}
			return list[i], nil
		}
		list[i].Status = status
		list[i].UpdatedBy = actor
		list[i].UpdatedAt = time.Now().UTC().Format(time.RFC3339)
		if status == string(domain.FindingOpen) {
			list[i].StatusReason = ""
		} else if reason != "" {
			list[i].StatusReason = reason
		} else if status != string(domain.FindingInvalid) {
			list[i].StatusReason = ""
		}
		if s.db != nil {
			_ = s.db.upsertFinding(list[i], i)
		}
		s.findings[workspaceID] = list
		return list[i], nil
	}
	return domain.Finding{}, fmt.Errorf("finding %q not found", findingID)
}

// OpenFindings returns still-open findings for a workspace (insertion order).
func (s *Store) OpenFindings(workspaceID string) []domain.Finding {
	s.mu.RLock()
	defer s.mu.RUnlock()
	var out []domain.Finding
	for _, f := range s.findings[workspaceID] {
		if f.Status == string(domain.FindingOpen) {
			out = append(out, f)
		}
	}
	return out
}

// FindingsByStatus returns findings matching status, or all when status is "" / "all".
func (s *Store) FindingsByStatus(workspaceID, status string) []domain.Finding {
	s.mu.RLock()
	defer s.mu.RUnlock()
	all := s.findings[workspaceID]
	if status == "" || status == "all" {
		return append([]domain.Finding(nil), all...)
	}
	var out []domain.Finding
	for _, f := range all {
		if f.Status == status {
			out = append(out, f)
		}
	}
	return out
}

// OpenFindingsCount returns how many of a loop's findings are still open.
func (s *Store) OpenFindingsCount(workspaceID string) int {
	s.mu.RLock()
	defer s.mu.RUnlock()
	n := 0
	for _, f := range s.findings[workspaceID] {
		if f.Status == string(domain.FindingOpen) {
			n++
		}
	}
	return n
}

// ---- Analysis (observatory) ----

// AddAnalysis persists an observatory run (newest-first in memory).
func (s *Store) AddAnalysis(a domain.AnalysisRun) domain.AnalysisRun {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.analysis = append([]domain.AnalysisRun{a}, s.analysis...)
	if len(s.analysis) > 50 {
		s.analysis = s.analysis[:50]
	}
	if s.db != nil {
		_ = s.db.upsertAnalysis(a)
	}
	return a
}

// LatestAnalysis returns the newest analysis for a project (optionally scoped to a loop).
func (s *Store) LatestAnalysis(projectID, workspaceID string) (domain.AnalysisRun, bool) {
	s.mu.RLock()
	defer s.mu.RUnlock()
	for _, a := range s.analysis {
		if a.ProjectID != projectID {
			continue
		}
		if workspaceID != "" && a.WorkspaceID != workspaceID {
			continue
		}
		return a, true
	}
	// Fall back to any project-level run when a loop-specific one is missing.
	if workspaceID != "" {
		for _, a := range s.analysis {
			if a.ProjectID == projectID && a.WorkspaceID == "" {
				return a, true
			}
		}
	}
	return domain.AnalysisRun{}, false
}

// ---- PRs ----

// PRs returns list rows: every field except Commits and Files, which can run
// to tens of megabytes per PR. PR(id) returns the full record.
func (s *Store) PRs() []domain.PR {
	s.mu.RLock()
	defer s.mu.RUnlock()
	out := make([]domain.PR, len(s.prs))
	for i := range s.prs {
		row := s.prs[i]
		row.Commits, row.Files = nil, nil
		out[i] = clonePR(row)
	}
	return out
}

func (s *Store) PR(id string) (domain.PR, bool) {
	s.mu.RLock()
	defer s.mu.RUnlock()
	for _, p := range s.prs {
		if p.ID == id {
			return clonePR(p), true
		}
	}
	return domain.PR{}, false
}

// PRByWorkspace returns the PR linked to a loop, if any (one PR per loop in v1 — the
// engine creates exactly one at pr_open and reworks it in place). Newest match in
// store order wins.
func (s *Store) PRByWorkspace(workspaceID string) (domain.PR, bool) {
	s.mu.RLock()
	defer s.mu.RUnlock()
	for i := len(s.prs) - 1; i >= 0; i-- {
		if s.prs[i].WorkspaceID == workspaceID {
			return clonePR(s.prs[i]), true
		}
	}
	return domain.PR{}, false
}

func prIsUnmerged(p domain.PR) bool {
	return p.State != "Merged" && p.Group != "merged" && p.State != "Closed"
}

// OpenPRByWorkspace returns an unmerged PR for the workspace, preferring GitHub
// over a leftover Local fallback. Used when opening/syncing so a merged GitHub
// row cannot hide a still-open Local PR, and vice versa.
func (s *Store) OpenPRByWorkspace(workspaceID string) (domain.PR, bool) {
	s.mu.RLock()
	defer s.mu.RUnlock()
	var local domain.PR
	var haveLocal bool
	for i := len(s.prs) - 1; i >= 0; i-- {
		p := s.prs[i]
		if p.WorkspaceID != workspaceID || !prIsUnmerged(p) {
			continue
		}
		if p.Source == domain.SourceGitHub {
			return clonePR(p), true
		}
		if !haveLocal {
			local = p
			haveLocal = true
		}
	}
	if haveLocal {
		return clonePR(local), true
	}
	return domain.PR{}, false
}

// DeletePR removes a PR by id. Missing ids are a no-op (ok=false).
func (s *Store) DeletePR(id string) bool {
	s.mu.Lock()
	defer s.mu.Unlock()
	for i, p := range s.prs {
		if p.ID != id {
			continue
		}
		if s.db != nil {
			if err := s.db.deleteDoc("prs", id); err != nil {
				return false
			}
		}
		s.prs = append(s.prs[:i], s.prs[i+1:]...)
		return true
	}
	return false
}

// AddPR persists a new PR (engine pr_open, M6). The DB write happens FIRST; the
// in-memory cache is appended only on success. An existing ID is updated in place
// so AddPR is idempotent (a rework re-creates the PR for the same loop under a
// stable id). Returns the stored copy.
func (s *Store) AddPR(p domain.PR) (domain.PR, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	for i := range s.prs {
		if s.prs[i].ID == p.ID {
			fillPRTimes(&p, s.prs[i].CreatedAt)
			if s.db != nil {
				if err := s.db.upsertDoc("prs", p.ID, p, i); err != nil {
					return domain.PR{}, err
				}
			}
			s.prs[i] = p
			s.noteLocalPRNumberLocked(p)
			return clonePR(p), nil
		}
	}
	fillPRTimes(&p, "")
	if s.db != nil {
		if err := s.db.upsertDoc("prs", p.ID, p, len(s.prs)); err != nil {
			return domain.PR{}, err
		}
	}
	s.prs = append(s.prs, p)
	s.noteLocalPRNumberLocked(p)
	return clonePR(p), nil
}

func fillPRTimes(p *domain.PR, existingCreated string) {
	now := time.Now().UTC().Format(time.RFC3339)
	if strings.TrimSpace(p.CreatedAt) == "" {
		p.CreatedAt = existingCreated
	}
	if strings.TrimSpace(p.CreatedAt) == "" {
		p.CreatedAt = now
	}
	if strings.TrimSpace(p.UpdatedAt) == "" {
		p.UpdatedAt = now
	}
}

// fillMissingPRTimes copies a known sibling stamp when only one of CreatedAt /
// UpdatedAt is present. Both-empty rows stay empty: the frontend already treats
// missing stamps as unknown, which is safer than claiming the upgrade instant
// is both creation and last-activity time.
func fillMissingPRTimes(p *domain.PR) bool {
	if p == nil {
		return false
	}
	c := strings.TrimSpace(p.CreatedAt)
	u := strings.TrimSpace(p.UpdatedAt)
	if c != "" && u != "" {
		return false
	}
	if c == "" && u != "" {
		p.CreatedAt = u
		return true
	}
	if u == "" && c != "" {
		p.UpdatedAt = c
		return true
	}
	return false
}

func (d *db) backfillPRTimes(s *Store) error {
	if d == nil || s == nil {
		return nil
	}
	var dirty []int
	for i := range s.prs {
		if fillMissingPRTimes(&s.prs[i]) {
			dirty = append(dirty, i)
		}
	}
	if len(dirty) == 0 {
		return nil
	}
	tx, err := d.sql.Begin()
	if err != nil {
		return fmt.Errorf("backfill PR times: %w", err)
	}
	defer tx.Rollback() //nolint:errcheck // no-op after Commit
	for _, i := range dirty {
		if err := upsertDocTx(tx, "prs", s.prs[i].ID, s.prs[i], i); err != nil {
			return err
		}
	}
	if err := tx.Commit(); err != nil {
		return fmt.Errorf("backfill PR times: %w", err)
	}
	return nil
}

// NextPRNumber returns the next sequential Local PR number from a persisted
// high-water mark. Deleting a workspace (or its PRs) does not reclaim numbers;
// they only ever increase. Empty store / never issued → 1. GitHub-assigned
// numbers live in their own namespace and do not consume this sequence.
func (s *Store) NextPRNumber() int {
	s.mu.Lock()
	defer s.mu.Unlock()
	n := s.prNumberSeq + 1
	s.notePRNumberLocked(n)
	return n
}

// notePRNumberLocked raises the Local high-water mark to at least n and persists it.
// Caller holds s.mu for writing. GitHub numbers must not be passed here — they
// are assigned by gh and would permanently jump every project's next Local PR.
func (s *Store) notePRNumberLocked(n int) {
	if n <= s.prNumberSeq {
		return
	}
	s.prNumberSeq = n
	if s.db != nil {
		_ = s.db.setPRNumberSeq(n)
	}
}

func (s *Store) noteLocalPRNumberLocked(p domain.PR) {
	if p.Source == domain.SourceGitHub {
		return
	}
	s.notePRNumberLocked(p.Number)
}

func (s *Store) UpdatePR(id string, fn func(*domain.PR)) (domain.PR, bool) {
	s.mu.Lock()
	defer s.mu.Unlock()
	for i := range s.prs {
		if s.prs[i].ID == id {
			fn(&s.prs[i])
			if s.db != nil {
				_ = s.db.upsertDoc("prs", id, s.prs[i], i)
			}
			s.noteLocalPRNumberLocked(s.prs[i])
			// Return a deep copy (like PR/PRs/PRByWorkspace) so a caller that hands the
			// result to Wails for JSON marshaling after the lock drops can't race a
			// concurrent UpdatePR appending to the same PR's nested slices.
			return clonePR(s.prs[i]), true
		}
	}
	return domain.PR{}, false
}

// ---- Escalations ----

const escalationKeySep = "\x1e"

func escalationKey(workspaceID, agentCode string) string {
	agentCode = strings.TrimSpace(agentCode)
	if agentCode == "" {
		return workspaceID
	}
	return workspaceID + escalationKeySep + agentCode
}

func (s *Store) Escalation(workspaceID string) (domain.Escalation, bool) {
	s.mu.RLock()
	defer s.mu.RUnlock()
	_, e, ok := s.lookupEscalationLocked(workspaceID, "")
	return e, ok
}

// HasOpenEscalation reports whether any walk on this workspace still has an
// unanswered question (workspace-keyed or chat-keyed).
func (s *Store) HasOpenEscalation(workspaceID string) bool {
	s.mu.RLock()
	defer s.mu.RUnlock()
	prefix := workspaceID + escalationKeySep
	for k, ev := range s.escalations {
		if ev.Status != "open" {
			continue
		}
		if ev.WorkspaceID == workspaceID || k == workspaceID || strings.HasPrefix(k, prefix) {
			return true
		}
	}
	return false
}

// AddEscalation records (or replaces) the escalation for a loop and persists it
// (PRD §13, §14). Chat-owned walks key by workspace+agent so two sibling walks
// can each keep an open question.
func (s *Store) AddEscalation(e domain.Escalation) domain.Escalation {
	s.mu.Lock()
	defer s.mu.Unlock()
	key := escalationKey(e.WorkspaceID, e.AgentCode)
	s.escalations[key] = e
	if s.db != nil {
		_ = s.db.upsertEscalation(key, e)
	}
	return e
}

// AnswerEscalation resolves the open escalation for a loop, persisting the answer
// and flipping status=answered (PRD §13). It is IDEMPOTENT at the root: a second
// answer (a double-submit, or AnswerEscalation called again after the loop already
// settled) is a NO-OP — if the escalation is already answered it returns the existing
// record with ok=false so the engine never re-delivers or re-dispatches a settled
// loop (the "resurrect a terminal loop" class). A missing escalation also returns
// ok=false.
func (s *Store) AnswerEscalation(workspaceID, choice, freeText string) (domain.Escalation, bool) {
	return s.AnswerEscalationFor(workspaceID, "", choice, freeText)
}

// AnswerEscalationFor resolves the escalation owned by agentCode. Empty agentCode
// is the workspace-scoped row (UI / legacy).
func (s *Store) AnswerEscalationFor(workspaceID, agentCode, choice, freeText string) (domain.Escalation, bool) {
	s.mu.Lock()
	defer s.mu.Unlock()
	key, e, ok := s.lookupEscalationLocked(workspaceID, agentCode)
	if !ok {
		return domain.Escalation{}, false
	}
	if e.Status == "answered" {
		return e, false
	}
	choice = strings.TrimSpace(choice)
	freeText = strings.TrimSpace(freeText)
	resolved := freeText
	if resolved == "" {
		resolved = choice
	}
	e.Answer = resolved
	e.Choice = choice
	e.FreeText = freeText
	e.Status = "answered"
	e.Applied = false
	s.escalations[key] = e
	if s.db != nil {
		_ = s.db.upsertEscalation(key, e)
	}
	return e, true
}

func (s *Store) lookupEscalationLocked(workspaceID, agentCode string) (key string, e domain.Escalation, ok bool) {
	agentCode = strings.TrimSpace(agentCode)
	if agentCode != "" {
		key = escalationKey(workspaceID, agentCode)
		if e, ok = s.escalations[key]; ok {
			return key, e, true
		}
		if e, ok = s.escalations[workspaceID]; ok {
			ac := strings.TrimSpace(e.AgentCode)
			if ac == "" || ac == agentCode {
				return workspaceID, e, true
			}
		}
		return "", domain.Escalation{}, false
	}
	prefix := workspaceID + escalationKeySep
	var openKey, anyKey string
	var open, any domain.Escalation
	hasOpen, hasAny := false, false
	for k, ev := range s.escalations {
		if ev.WorkspaceID != workspaceID && k != workspaceID && !strings.HasPrefix(k, prefix) {
			continue
		}
		if !hasAny || k == workspaceID {
			hasAny = true
			anyKey, any = k, ev
		}
		if ev.Status == "open" && (!hasOpen || k < openKey) {
			hasOpen = true
			openKey, open = k, ev
		}
	}
	if hasOpen {
		return openKey, open, true
	}
	if e, ok = s.escalations[workspaceID]; ok {
		return workspaceID, e, true
	}
	return anyKey, any, hasAny
}

// MarkEscalationApplied records that a loop goroutine consumed the persisted
// answer, so a later re-dispatch must not seed it again.
func (s *Store) MarkEscalationApplied(workspaceID string) {
	s.mu.Lock()
	defer s.mu.Unlock()
	key, e, ok := s.lookupEscalationLocked(workspaceID, "")
	if !ok || e.Status != "answered" || e.Applied {
		return
	}
	e.Applied = true
	s.escalations[key] = e
	if s.db != nil {
		_ = s.db.upsertEscalation(key, e)
	}
}

// ---- Inbox ----

func (s *Store) Inbox() []domain.InboxItem {
	s.mu.RLock()
	defer s.mu.RUnlock()
	return append([]domain.InboxItem(nil), s.inbox...)
}

func (s *Store) UnreadCount() int {
	s.mu.RLock()
	defer s.mu.RUnlock()
	n := 0
	for _, it := range s.inbox {
		if it.Unread {
			n++
		}
	}
	return n
}

// UnseenChatCount returns how many chat-facing agents (PA*/MA) have Unseen set.
func (s *Store) UnseenChatCount() int {
	s.mu.RLock()
	defer s.mu.RUnlock()
	n := 0
	for _, list := range s.agents {
		for _, a := range list {
			if a.Unseen {
				n++
			}
		}
	}
	return n
}

// AttentionCounts rolls up unseen chat tabs (per workspace / project) plus inbox
// unread for sidebar badges and the dock icon. Chat rows live in the inbox, so
// Dock is unseen + non-chat inbox unread + Ask Recur unseen (chat unread is
// already in Unseen). AppUnseen is counted once and is not Resource.NeedsYou.
// Archived workspaces are skipped — their chats are unreachable after Remove,
// so counting them would stick the dock/project badge forever.
func (s *Store) AttentionCounts() domain.AttentionCounts {
	s.mu.RLock()
	defer s.mu.RUnlock()
	archived := map[string]bool{}
	host := map[string]bool{}
	projOf := map[string]string{}
	for _, w := range s.workspaces {
		if w.Archived {
			archived[w.ID] = true
		}
		if w.IsPRChatHost() {
			host[w.ID] = true
		}
		projOf[w.ID] = w.ProjectID
	}
	byWs := map[string]int{}
	byProj := map[string]int{}
	unseen := 0
	for wsID, list := range s.agents {
		if archived[wsID] {
			continue
		}
		n := 0
		for _, a := range list {
			if a.Unseen {
				n++
			}
		}
		if n == 0 {
			continue
		}
		byWs[wsID] = n
		unseen += n
		// Hidden PR chat hosts have no sidebar row; inbox carries the badge.
		if host[wsID] {
			continue
		}
		if projID := projOf[wsID]; projID != "" {
			byProj[projID] += n
		}
	}
	inboxUnread := 0
	otherUnread := 0
	for _, it := range s.inbox {
		if !it.Unread {
			continue
		}
		inboxUnread++
		if it.Kind != "chat" {
			otherUnread++
		}
	}
	appUnseen := 0
	if s.db != nil {
		if list, err := s.db.sessionsForAppKind(domain.SessionKindApp); err == nil {
			for _, sess := range list {
				var cfg struct {
					Unseen bool `json:"unseen"`
					Closed bool `json:"closed"`
				}
				_ = json.Unmarshal([]byte(sess.ConfigJSON), &cfg)
				if cfg.Unseen && !cfg.Closed {
					appUnseen++
				}
			}
		}
	}
	return domain.AttentionCounts{
		Unseen:      unseen,
		InboxUnread: inboxUnread,
		Dock:        unseen + otherUnread + appUnseen,
		ByWorkspace: byWs,
		ByProject:   byProj,
		AppUnseen:   appUnseen,
	}
}

func (s *Store) MarkInboxRead(id string) {
	s.mu.Lock()
	defer s.mu.Unlock()
	for i := range s.inbox {
		if s.inbox[i].ID == id {
			s.inbox[i].Unread = false
			if s.db != nil {
				_ = s.db.upsertDoc("inbox", s.inbox[i].ID, s.inbox[i], i)
			}
		}
	}
}

func (s *Store) MarkAllInboxRead() {
	s.mu.Lock()
	defer s.mu.Unlock()
	for i := range s.inbox {
		s.inbox[i].Unread = false
		if s.db != nil {
			_ = s.db.upsertDoc("inbox", s.inbox[i].ID, s.inbox[i], i)
		}
	}
}

// AddInboxItem prepends an actionable row to the inbox and persists it (PRD §16.11,
// D5: actionable-only). Newest first, matching the seeded ordering. Used when a
// loop raises an escalation ("needs you"). De-dupes by ID (replaces in place).
func (s *Store) AddInboxItem(it domain.InboxItem) {
	s.mu.Lock()
	defer s.mu.Unlock()
	for i := range s.inbox {
		if s.inbox[i].ID == it.ID {
			s.inbox[i] = it
			if s.db != nil {
				_ = s.db.upsertDoc("inbox", it.ID, it, i)
			}
			return
		}
	}
	s.inbox = append([]domain.InboxItem{it}, s.inbox...)
	if s.db != nil {
		// Re-persist with refreshed sort indexes so the new head sorts first.
		for i := range s.inbox {
			_ = s.db.upsertDoc("inbox", s.inbox[i].ID, s.inbox[i], i)
		}
	}
}

// RemoveInboxByTarget drops every inbox row whose Target matches (e.g. when an
// escalation for a loop is answered, its "needs you" row is cleared). Persists the
// removal. Returns the number removed.
func (s *Store) RemoveInboxByTarget(target string) int {
	s.mu.Lock()
	defer s.mu.Unlock()
	kept := s.inbox[:0]
	removed := 0
	for _, it := range s.inbox {
		if it.Target == target {
			removed++
			if s.db != nil {
				_ = s.db.deleteDoc("inbox", it.ID)
			}
			continue
		}
		kept = append(kept, it)
	}
	s.inbox = kept
	return removed
}

// inboxTargetsWorkspace reports whether target deep-links into this workspace
// (loop: / workspace: / chat:ws:code). Shared by Remove and hard-delete so
// neither leaves stranded rows. PR targets are handled separately — Remove keeps them.
func inboxTargetsWorkspace(target, workspaceID string) bool {
	kind, ref, _ := strings.Cut(target, ":")
	if (kind == "loop" || kind == "workspace") && ref == workspaceID {
		return true
	}
	if kind == "chat" {
		ws, _, ok := strings.Cut(ref, ":")
		return ok && ws == workspaceID
	}
	return false
}

// inboxTargetsDeletedWorkspace is the hard-delete cascade predicate: workspace
// deep-links plus pr: rows for PRs being deleted with the workspace.
func inboxTargetsDeletedWorkspace(target, workspaceID string, prIDs map[string]bool) bool {
	if inboxTargetsWorkspace(target, workspaceID) {
		return true
	}
	kind, ref, _ := strings.Cut(target, ":")
	return kind == "pr" && prIDs[ref]
}

// ClearInboxForWorkspace drops inbox rows that deep-link into this workspace
// (loop: / workspace: / chat:ws:code), so Remove does not leave badges for a
// hidden archived workspace. PR rows are kept — Remove preserves the PR record
// and merge path.
// Returns how many rows were removed.
func (s *Store) ClearInboxForWorkspace(workspaceID string) int {
	s.mu.Lock()
	defer s.mu.Unlock()
	kept := s.inbox[:0]
	removed := 0
	for _, it := range s.inbox {
		if inboxTargetsWorkspace(it.Target, workspaceID) {
			removed++
			if s.db != nil {
				_ = s.db.deleteDoc("inbox", it.ID)
			}
			continue
		}
		kept = append(kept, it)
	}
	s.inbox = kept
	return removed
}

// RemoveInboxItem drops one inbox row by ID and persists the removal, reporting
// whether it was there. Used when a permission request leaves the queue, where
// the row is per-request rather than per-workspace, so removing by target would
// clear unrelated rows for the same workspace.
func (s *Store) RemoveInboxItem(id string) bool {
	s.mu.Lock()
	defer s.mu.Unlock()
	for i, it := range s.inbox {
		if it.ID != id {
			continue
		}
		s.inbox = append(s.inbox[:i], s.inbox[i+1:]...)
		if s.db != nil {
			_ = s.db.deleteDoc("inbox", id)
		}
		return true
	}
	return false
}

// RemoveChatInboxItems drops every chat-kind (or ib-chat-*) inbox row. Used
// when marking all read so leftover/orphan "finished a reply" rows do not
// linger as read items. Returns how many were removed.
func (s *Store) RemoveChatInboxItems() int {
	s.mu.Lock()
	defer s.mu.Unlock()
	kept := s.inbox[:0]
	removed := 0
	for _, it := range s.inbox {
		if it.Kind == "chat" || strings.HasPrefix(it.ID, "ib-chat-") {
			removed++
			if s.db != nil {
				_ = s.db.deleteDoc("inbox", it.ID)
			}
			continue
		}
		kept = append(kept, it)
	}
	s.inbox = kept
	return removed
}

// ---- Policy / Settings / Resource ----

func (s *Store) Policy() domain.Policy {
	s.mu.RLock()
	defer s.mu.RUnlock()
	return s.policy
}

func (s *Store) SetPolicy(p domain.Policy) {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.policy = p
	if s.db != nil {
		_ = s.db.upsertSingleton("policy", p)
	}
}

func (s *Store) Settings() domain.Settings {
	s.mu.RLock()
	defer s.mu.RUnlock()
	return s.settings
}

func (s *Store) SetSettings(set domain.Settings) {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.settings = set
	if s.db != nil {
		_ = s.db.upsertSingleton("settings", set)
	}
}

func (s *Store) Resource() domain.Resource {
	s.mu.RLock()
	defer s.mu.RUnlock()
	return s.resource
}

func (s *Store) SetResource(r domain.Resource) {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.resource = r
	if s.db != nil {
		_ = s.db.upsertSingleton("resource", r)
	}
}

// ---- deep-copy helpers ----
//
// The getters above hand callers fully-owned copies of loop/PR aggregates: the
// outer struct is copied by value, and every nested slice is reallocated. Without
// this, a caller mutating a returned slice (e.g. pr.Files) would race the store's
// own copy once the M2 engine goroutines start mutating these aggregates. The
// only sanctioned way to mutate stored loops/PRs is via UpdateWorkspace/UpdatePR under
// the store lock.

func cloneWorkspace(l domain.Workspace) domain.Workspace {
	if l.Plan != nil {
		p := *l.Plan
		p.Steps = append([]domain.PlanStep(nil), p.Steps...)
		l.Plan = &p
	}
	l.ParentStepIDs = append([]string(nil), l.ParentStepIDs...)
	if l.FanOut != nil {
		f := *l.FanOut
		f.After = append([]string(nil), f.After...)
		l.FanOut = &f
	}
	l.AgentCodes = append([]string(nil), l.AgentCodes...)
	l.ChatTabOrder = append([]string(nil), l.ChatTabOrder...)
	l.Convergence = append([]domain.ConvPoint(nil), l.Convergence...)
	l.Verify = append([]domain.VerifyStep(nil), l.Verify...)
	return l
}

func cloneAgent(a domain.Agent) domain.Agent {
	a.Steps = append([]domain.AgentStep(nil), a.Steps...)
	return a
}

func clonePR(p domain.PR) domain.PR {
	p.Summary = cloneSummary(p.Summary)
	p.Commits = cloneCommits(p.Commits)
	p.Files = cloneFiles(p.Files)
	p.Comments = append([]domain.PRComment(nil), p.Comments...)
	p.Verify = append([]domain.VerifyStep(nil), p.Verify...)
	p.Checks = append([]domain.Check(nil), p.Checks...)
	return p
}

func cloneSummary(in []domain.SummaryPara) []domain.SummaryPara {
	if in == nil {
		return nil
	}
	out := make([]domain.SummaryPara, len(in))
	for i, sp := range in {
		sp.Items = append([]string(nil), sp.Items...)
		out[i] = sp
	}
	return out
}

func cloneCommits(in []domain.Commit) []domain.Commit {
	if in == nil {
		return nil
	}
	out := make([]domain.Commit, len(in))
	for i, c := range in {
		c.Files = cloneFiles(c.Files)
		out[i] = c
	}
	return out
}

func cloneFiles(in []domain.FileDiff) []domain.FileDiff {
	if in == nil {
		return nil
	}
	out := make([]domain.FileDiff, len(in))
	for i, f := range in {
		f.Lines = append([]domain.DiffLine(nil), f.Lines...)
		out[i] = f
	}
	return out
}

func (s *Store) GitHubBranchRequirePR(projectID, repo, branch string) (require bool, at time.Time, ok bool) {
	s.mu.RLock()
	defer s.mu.RUnlock()
	e, ok := s.githubBranchRules[githubBranchRuleKey(projectID, repo, branch)]
	if !ok {
		return false, time.Time{}, false
	}
	return e.Require, e.At, true
}

func (s *Store) SetGitHubBranchRequirePR(projectID, repo, branch string, require bool) {
	projectID = strings.TrimSpace(projectID)
	repo = strings.TrimSpace(repo)
	branch = strings.TrimSpace(branch)
	if projectID == "" || repo == "" || branch == "" {
		return
	}
	e := githubBranchRule{Require: require, At: time.Now()}
	s.mu.Lock()
	if s.githubBranchRules == nil {
		s.githubBranchRules = map[string]githubBranchRule{}
	}
	s.githubBranchRules[githubBranchRuleKey(projectID, repo, branch)] = e
	db := s.db
	s.mu.Unlock()
	if db != nil {
		_ = db.upsertGitHubBranchRule(projectID, repo, branch, e)
	}
}
