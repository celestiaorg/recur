package store

import (
	"database/sql"
	"encoding/json"
	"fmt"
	"strings"
	"sync"
	"time"

	"github.com/google/uuid"

	"recur/internal/domain"
)

// Sessions are the v2 transcript store. Unlike the v1
// aggregates they are NOT mirrored into an in-memory map — a transcript can be
// large and long-lived, so it lives only in SQLite and is queried on demand. The
// single db connection serializes writes; sessionMu additionally serializes the
// count-then-insert index/seq allocations so two writers to the same session
// can't collide on an idx. Reads need neither lock (append-only tables + pooled
// single connection), matching AppendEvent's rationale.

// sessionMu guards the idx/seq allocation critical sections for session writes.
// It is a package-level field added to Store via the methods below; kept separate
// from Store.mu so a streaming turn's per-line mirror writes never block the
// store-wide lock every UI read takes.
//
// (Declared here rather than on the Store struct to keep this slice self-contained;
// Store is a single process-wide value so a package-level mutex is equivalent for
// serialization and avoids editing the struct in store.go.)
var sessionMu sync.Mutex

// CreateSession inserts a new session, filling in an id, timestamps, and default
// status/context-mode where the caller left them zero. Returns the stored copy.
func (s *Store) CreateSession(sess domain.Session) (domain.Session, error) {
	if s.db == nil {
		return domain.Session{}, fmt.Errorf("session store requires a database")
	}
	if sess.ID == "" {
		sess.ID = uuid.Must(uuid.NewV7()).String()
	}
	now := time.Now().UnixMilli()
	if sess.CreatedAt == 0 {
		sess.CreatedAt = now
	}
	sess.UpdatedAt = now
	if sess.Status == "" {
		sess.Status = "idle"
	}
	if sess.ContextMode == "" {
		sess.ContextMode = domain.ContextModeProvider
	}
	if sess.ConfigJSON == "" {
		sess.ConfigJSON = "{}"
	}
	if err := s.db.insertSession(sess); err != nil {
		return domain.Session{}, err
	}
	return sess, nil
}

// GetSession returns a session header (no transcript) by id.
func (s *Store) GetSession(id string) (domain.Session, bool) {
	if s.db == nil {
		return domain.Session{}, false
	}
	sess, err := s.db.getSession(id)
	if err != nil {
		return domain.Session{}, false
	}
	return sess, true
}

// SessionForWorkspaceKind returns the newest session of a given kind bound to a loop,
// if any (e.g. the maestro session for a loop). Used to reuse a session across
// turns rather than creating one per turn.
func (s *Store) SessionForWorkspaceKind(workspaceID, kind string) (domain.Session, bool) {
	if s.db == nil {
		return domain.Session{}, false
	}
	sess, err := s.db.sessionForLoopKind(workspaceID, kind)
	if err != nil {
		return domain.Session{}, false
	}
	return sess, true
}

// SessionForAppKind returns the newest session of a given kind with a NULL
// workspace_id (the persistent Ask Recur coordinator).
func (s *Store) SessionForAppKind(kind string) (domain.Session, bool) {
	if s.db == nil || kind == "" {
		return domain.Session{}, false
	}
	sess, err := s.db.sessionForAppKind(kind)
	if err != nil {
		return domain.Session{}, false
	}
	return sess, true
}

// SessionsForAppKind returns every session of a kind with a NULL workspace_id,
// oldest first (the Ask Recur tab strip).
func (s *Store) SessionsForAppKind(kind string) []domain.Session {
	if s.db == nil || kind == "" {
		return nil
	}
	list, err := s.db.sessionsForAppKind(kind)
	if err != nil {
		return nil
	}
	return list
}

// SessionHeadersByWorkspace returns session headers (no transcripts) grouped
// by workspace_id, newest-updated first, capped per workspace.
func (s *Store) SessionHeadersByWorkspace() map[string][]domain.Session {
	if s.db == nil {
		return nil
	}
	m, err := s.db.sessionHeadersByWorkspace(8)
	if err != nil {
		return nil
	}
	return m
}

// LatestEventTSByWorkspace returns MAX(events.ts) per workspace_id.
func (s *Store) LatestEventTSByWorkspace() map[string]int64 {
	if s.db == nil {
		return nil
	}
	m, err := s.db.latestEventTSByWorkspace()
	if err != nil {
		return nil
	}
	return m
}

// StreamingAppSessions returns app-scoped sessions (workspace_id NULL) left
// in status=streaming after a crash — Recover idles them.
func (s *Store) StreamingAppSessions() []domain.Session {
	if s.db == nil {
		return nil
	}
	list, err := s.db.streamingAppSessions()
	if err != nil {
		return nil
	}
	return list
}

// ReassignWorkspaceSessions moves every session from fromID onto toID so PR
// chat-host transcripts survive linking a real worktree (same rows, new owner).
// remap rewrites config_json.code (and matching ownerCode) before the move so
// two transcripts that shared a tab code stay distinct. Empty dest sessions
// whose code is kept (not remapped) are dropped so SessionForLoopAgent sees
// the hoisted row.
func (s *Store) ReassignWorkspaceSessions(fromID, toID string, remap map[string]string) error {
	fromID = strings.TrimSpace(fromID)
	toID = strings.TrimSpace(toID)
	if fromID == "" || toID == "" || fromID == toID {
		return nil
	}
	if s.db == nil {
		return nil
	}
	sessionMu.Lock()
	defer sessionMu.Unlock()
	return s.db.reassignSessions(fromID, toID, remap)
}

// SessionForLoopAgent returns the session for a (loop, agent code) — the key the
// mirror uses so structurally-distinct agents that share a role (the normal and
// the security Reviewer both role=reviewer) get separate transcripts. The code is
// stored in config_json.code.
func (s *Store) SessionForLoopAgent(workspaceID, code string) (domain.Session, bool) {
	if s.db == nil || code == "" {
		return domain.Session{}, false
	}
	sess, err := s.db.sessionForLoopAgent(workspaceID, code)
	if err != nil {
		return domain.Session{}, false
	}
	return sess, true
}

// SessionForChatAgent returns the specialist session for this conductor tab.
func (s *Store) SessionForChatAgent(workspaceID, owner, code string) (domain.Session, bool) {
	if s.db == nil || code == "" {
		return domain.Session{}, false
	}
	sess, err := s.db.sessionForChatAgent(workspaceID, owner, code)
	if err != nil {
		return domain.Session{}, false
	}
	return sess, true
}

// SessionsForLoop returns every session bound to a loop, oldest-created first —
// the set the activity-feed projection reads.
func (s *Store) SessionsForLoop(workspaceID string) []domain.Session {
	if s.db == nil {
		return nil
	}
	sessions, err := s.db.sessionsForLoop(workspaceID)
	if err != nil {
		return nil
	}
	return sessions
}

// UpdateSessionRef records the provider session ref (e.g. Claude's rotating
// session id) and touches updated_at. Best-effort; a provider ref is a resume
// convenience, not durable truth.
func (s *Store) UpdateSessionRef(id, ref string) error {
	if s.db == nil || ref == "" {
		return nil
	}
	return s.db.updateSessionRef(id, ref)
}

// UpdateSessionProvider records which provider last ran (or is selected for)
// this session so fork/resume see the dock's current provider.
func (s *Store) UpdateSessionProvider(id, provider string) error {
	if s.db == nil || id == "" || strings.TrimSpace(provider) == "" {
		return nil
	}
	return s.db.updateSessionProvider(id, provider)
}

// SetSessionStatus updates a session's status (idle | streaming | error).
func (s *Store) SetSessionStatus(id, status string) error {
	if s.db == nil {
		return nil
	}
	return s.db.setSessionStatus(id, status)
}

// UpdateSessionConfigJSON rewrites a session's config_json (e.g. last chat model
// selection) and touches updated_at.
func (s *Store) UpdateSessionConfigJSON(id, configJSON string) error {
	if s.db == nil || id == "" {
		return nil
	}
	if configJSON == "" {
		configJSON = "{}"
	}
	return s.db.updateSessionConfigJSON(id, configJSON)
}

// WriteSessionContext inserts a compaction watermark and
// points session.active_context_id at it. Full history is untouched; this is
// the summary + last compacted message used for fork inject and app-mode
// request assembly. Re-compaction inserts a new row and repoints.
func (s *Store) WriteSessionContext(sessionID, summary, lastCompactedMessageID string) (domain.SessionContext, error) {
	if s.db == nil {
		return domain.SessionContext{}, fmt.Errorf("session store requires a database")
	}
	summary = strings.TrimSpace(summary)
	if sessionID == "" || summary == "" || lastCompactedMessageID == "" {
		return domain.SessionContext{}, fmt.Errorf("session context: session, summary, and watermark are required")
	}
	sessionMu.Lock()
	defer sessionMu.Unlock()
	return s.db.writeSessionContext(sessionID, summary, lastCompactedMessageID)
}

// ActiveSessionContext returns the compaction row session.active_context_id
// points at, if any.
func (s *Store) ActiveSessionContext(sessionID string) (domain.SessionContext, bool) {
	if s.db == nil || sessionID == "" {
		return domain.SessionContext{}, false
	}
	ctx, err := s.db.activeSessionContext(sessionID)
	if err != nil {
		return domain.SessionContext{}, false
	}
	return ctx, true
}

// PatchSessionConfigTitle sets config_json.title without replacing the rest of
// the object (mode/model/flags must survive async tab-title upgrades).
func (s *Store) PatchSessionConfigTitle(id, title string) error {
	if s.db == nil || id == "" {
		return nil
	}
	return s.db.patchSessionConfigTitle(id, title)
}

// PatchSessionConfigSimplify sets config_json.simplify without replacing the
// rest of the object (mode/model/flags/title must survive a concurrent dock write).
func (s *Store) PatchSessionConfigSimplify(id string, on bool) error {
	if s.db == nil || id == "" {
		return nil
	}
	return s.db.patchSessionConfigSimplify(id, on)
}

// PatchSessionConfigSimplifyIfUnset freezes simplify only when the key is
// still missing, so a concurrent RememberWorkspaceChatSimplify wins.
func (s *Store) PatchSessionConfigSimplifyIfUnset(id string, on bool) error {
	if s.db == nil || id == "" {
		return nil
	}
	return s.db.patchSessionConfigSimplifyIfUnset(id, on)
}

// PatchSessionConfigMerge merges a JSON object into config_json (RFC 7396)
// without deleting keys the patch omits — dock model/mode must not clobber a
// concurrent simplify json_set. Nested objects (flags) merge too; replace
// those with PatchSessionConfigFlags.
func (s *Store) PatchSessionConfigMerge(id, patchJSON string) error {
	if s.db == nil || id == "" || strings.TrimSpace(patchJSON) == "" {
		return nil
	}
	return s.db.patchSessionConfigMerge(id, patchJSON)
}

// PatchSessionConfigDock merges a JSON object into config_json then replaces
// flags in the same UPDATE (json_patch then json_set). A dock model write
// must not pair with another selection's flags, and must not clobber simplify.
// Empty flagsJSON leaves flags untouched (mode-only persist).
func (s *Store) PatchSessionConfigDock(id, patchJSON, flagsJSON string) error {
	if s.db == nil || id == "" {
		return nil
	}
	if strings.TrimSpace(patchJSON) == "" && strings.TrimSpace(flagsJSON) == "" {
		return nil
	}
	return s.db.patchSessionConfigDock(id, patchJSON, flagsJSON)
}

// PatchSessionConfigFlags replaces config_json.flags with the given JSON
// object (json_set, not json_patch) so a smaller map does not retain stale keys.
func (s *Store) PatchSessionConfigFlags(id, flagsJSON string) error {
	if s.db == nil || id == "" {
		return nil
	}
	if strings.TrimSpace(flagsJSON) == "" {
		flagsJSON = "{}"
	}
	return s.db.patchSessionConfigFlags(id, flagsJSON)
}

// AppendMessage inserts a full message (with its content blocks) at the end of a
// session in one transaction, assigning ids and the session-relative idx. Returns
// the stored message (ids filled in). Use CreateMessage + AppendContentBlock for
// incremental streaming instead.
func (s *Store) AppendMessage(sessionID string, m domain.Message) (domain.Message, error) {
	if s.db == nil {
		return domain.Message{}, fmt.Errorf("session store requires a database")
	}
	sessionMu.Lock()
	defer sessionMu.Unlock()
	return s.db.appendMessage(sessionID, m)
}

// CreateMessage inserts an empty message (no content yet) at the end of a session
// and returns its id — the streaming entry point. Blocks are added with
// AppendContentBlock as they arrive, and the message is finalized with
// CompleteMessage.
func (s *Store) CreateMessage(sessionID string, m domain.Message) (string, error) {
	if s.db == nil {
		return "", fmt.Errorf("session store requires a database")
	}
	sessionMu.Lock()
	defer sessionMu.Unlock()
	if m.ID == "" {
		m.ID = uuid.Must(uuid.NewV7()).String()
	}
	if m.OriginSessionID == "" {
		m.OriginSessionID = sessionID
	}
	if m.CreatedAt == 0 {
		m.CreatedAt = time.Now().UnixMilli()
	}
	if m.StreamStatus == "" {
		m.StreamStatus = domain.StreamStreaming
	}
	tx, err := s.db.sql.Begin()
	if err != nil {
		return "", err
	}
	defer tx.Rollback() //nolint:errcheck
	idx, err := nextSessionIdxTx(tx, sessionID)
	if err != nil {
		return "", err
	}
	if err := insertMessageTx(tx, m); err != nil {
		return "", err
	}
	if err := insertSessionMessageTx(tx, sessionID, m.ID, idx); err != nil {
		return "", err
	}
	if err := tx.Commit(); err != nil {
		return "", err
	}
	return m.ID, nil
}

// DropSessionMessages unlinks and deletes messages from a session (refusal
// fallback retraction). Idx gaps are left as-is; transcript order is still idx.
func (s *Store) DropSessionMessages(sessionID string, messageIDs []string) error {
	if s.db == nil || sessionID == "" || len(messageIDs) == 0 {
		return nil
	}
	sessionMu.Lock()
	defer sessionMu.Unlock()
	return s.db.dropSessionMessages(sessionID, messageIDs)
}

// DeleteSession drops a session and any messages no other session (a fork) still
// references. session_context rows cascade with the session.
func (s *Store) DeleteSession(sessionID string) error {
	if s.db == nil || sessionID == "" {
		return nil
	}
	sessionMu.Lock()
	defer sessionMu.Unlock()
	return s.db.deleteSession(sessionID)
}

func (d *db) deleteSession(sessionID string) error {
	tx, err := d.sql.Begin()
	if err != nil {
		return err
	}
	defer tx.Rollback() //nolint:errcheck
	if err := deleteSessionTx(tx, sessionID); err != nil {
		return err
	}
	return tx.Commit()
}

func deleteSessionTx(tx *sql.Tx, sessionID string) error {
	// Messages only this session still joins. message_content / session_message
	// rows cascade from message. Then DELETE session drops leftover join rows
	// (forks still hold those messages) and session_context.
	if _, err := tx.Exec(`
		DELETE FROM message
		 WHERE id IN (SELECT message_id FROM session_message WHERE session_id = ?)
		   AND NOT EXISTS (
		     SELECT 1 FROM session_message sm
		      WHERE sm.message_id = message.id AND sm.session_id != ?
		   )`, sessionID, sessionID); err != nil {
		return fmt.Errorf("delete session messages %s: %w", sessionID, err)
	}
	if _, err := tx.Exec(`DELETE FROM session WHERE id = ?`, sessionID); err != nil {
		return fmt.Errorf("delete session %s: %w", sessionID, err)
	}
	return nil
}

// closedChatCfg is the subset of session config_json used to match a chat and
// the specialist sessions it owns.
type closedChatCfg struct {
	Code      string `json:"code"`
	OwnerCode string `json:"ownerCode"`
}

// DeleteClosedChat permanently drops the pair/MA session whose config code
// matches, plus specialist sessions that name it as ownerCode. Messages still
// referenced by a fork are kept. ChatTabOrder is stripped in the same
// transaction as the session deletes so a persistence failure cannot leave a
// half-deleted chat.
func (s *Store) DeleteClosedChat(workspaceID, code string) error {
	if s.db == nil {
		return fmt.Errorf("session store requires a database")
	}
	if workspaceID == "" || code == "" {
		return fmt.Errorf("delete closed chat: workspace and code are required")
	}
	sessionMu.Lock()
	defer sessionMu.Unlock()
	s.mu.Lock()
	defer s.mu.Unlock()

	var ws *domain.Workspace
	sortIdx := 0
	for i := range s.workspaces {
		if s.workspaces[i].ID == workspaceID {
			ws = &s.workspaces[i]
			sortIdx = i
			break
		}
	}
	if ws == nil {
		return fmt.Errorf("workspace %q not found", workspaceID)
	}
	for _, a := range s.agents[workspaceID] {
		if a.Code == code {
			return fmt.Errorf("chat is still open")
		}
	}

	tx, err := s.db.sql.Begin()
	if err != nil {
		return fmt.Errorf("delete closed chat: %w", err)
	}
	defer tx.Rollback() //nolint:errcheck

	sessions, err := sessionsForLoopQuery(tx, workspaceID)
	if err != nil {
		return err
	}
	var owned, self []string
	for _, sess := range sessions {
		var cfg closedChatCfg
		_ = json.Unmarshal([]byte(sess.ConfigJSON), &cfg)
		if cfg.OwnerCode == code {
			owned = append(owned, sess.ID)
		} else if cfg.Code == code {
			self = append(self, sess.ID)
		}
	}
	for _, id := range append(owned, self...) {
		if err := deleteSessionTx(tx, id); err != nil {
			return err
		}
	}
	nextOrder := make([]string, 0, len(ws.ChatTabOrder))
	for _, c := range ws.ChatTabOrder {
		if c != code {
			nextOrder = append(nextOrder, c)
		}
	}
	persisted := cloneWorkspace(*ws)
	persisted.ChatTabOrder = nextOrder
	if err := upsertDocTx(tx, "workspaces", workspaceID, persisted, sortIdx); err != nil {
		return err
	}
	if err := tx.Commit(); err != nil {
		return fmt.Errorf("delete closed chat: %w", err)
	}
	ws.ChatTabOrder = nextOrder
	return nil
}

// TruncateSessionAfter unlinks every message after messageID from the session
// (chat "reset to this point"). Message rows are deleted only when no other
// session — a fork — still references them. Returns the unlinked ids.
func (s *Store) TruncateSessionAfter(sessionID, messageID string) ([]string, error) {
	if s.db == nil || sessionID == "" || messageID == "" {
		return nil, nil
	}
	sessionMu.Lock()
	defer sessionMu.Unlock()
	return s.db.truncateSessionAfter(sessionID, messageID)
}

func (d *db) truncateSessionAfter(sessionID, messageID string) ([]string, error) {
	tx, err := d.sql.Begin()
	if err != nil {
		return nil, err
	}
	defer tx.Rollback() //nolint:errcheck
	rows, err := tx.Query(
		`SELECT message_id FROM session_message WHERE session_id = ?
		   AND idx > (SELECT idx FROM session_message WHERE session_id = ? AND message_id = ?)`,
		sessionID, sessionID, messageID)
	if err != nil {
		return nil, err
	}
	var ids []string
	for rows.Next() {
		var id string
		if err := rows.Scan(&id); err != nil {
			rows.Close()
			return nil, err
		}
		ids = append(ids, id)
	}
	rows.Close()
	for _, id := range ids {
		if _, err := tx.Exec(`DELETE FROM session_message WHERE session_id = ? AND message_id = ?`, sessionID, id); err != nil {
			return nil, err
		}
		var refs int
		if err := tx.QueryRow(`SELECT COUNT(*) FROM session_message WHERE message_id = ?`, id).Scan(&refs); err != nil {
			return nil, err
		}
		if refs > 0 {
			continue
		}
		if _, err := tx.Exec(`DELETE FROM message_content WHERE message_id = ?`, id); err != nil {
			return nil, err
		}
		if _, err := tx.Exec(`DELETE FROM message WHERE id = ?`, id); err != nil {
			return nil, err
		}
	}
	return ids, tx.Commit()
}

// AppendContentBlock appends one content block to a message at the next block idx,
// assigning a block id. Safe to call repeatedly as a message streams.
func (s *Store) AppendContentBlock(messageID string, b domain.ContentBlock) error {
	if s.db == nil {
		return nil
	}
	sessionMu.Lock()
	defer sessionMu.Unlock()
	if b.ID == "" {
		b.ID = uuid.Must(uuid.NewV7()).String()
	}
	return s.db.appendContentBlock(messageID, b)
}

func (d *db) dropSessionMessages(sessionID string, messageIDs []string) error {
	tx, err := d.sql.Begin()
	if err != nil {
		return err
	}
	defer tx.Rollback() //nolint:errcheck
	for _, id := range messageIDs {
		id = strings.TrimSpace(id)
		if id == "" {
			continue
		}
		if _, err := tx.Exec(`DELETE FROM session_message WHERE session_id = ? AND message_id = ?`, sessionID, id); err != nil {
			return fmt.Errorf("drop session_message %s/%s: %w", sessionID, id, err)
		}
		if _, err := tx.Exec(`DELETE FROM message_content WHERE message_id = ?`, id); err != nil {
			return fmt.Errorf("drop message_content %s: %w", id, err)
		}
		if _, err := tx.Exec(`DELETE FROM message WHERE id = ?`, id); err != nil {
			return fmt.Errorf("drop message %s: %w", id, err)
		}
	}
	return tx.Commit()
}

// PatchToolUseInput replaces the arguments on a tool_use block identified by its
// internal id. Used when a provider attaches full args only on tool completion.
func (s *Store) PatchToolUseInput(internalToolUseID string, input json.RawMessage) error {
	if s.db == nil || internalToolUseID == "" || len(input) == 0 {
		return nil
	}
	sessionMu.Lock()
	defer sessionMu.Unlock()
	return s.db.patchToolUseInput(internalToolUseID, input)
}

// PatchFallbackFrom fills in a missing `from` on the latest fallback block of a
// message. Used when a supersedes hop persisted with only the serving model and
// a later model_refusal_fallback audit names the declining model.
func (s *Store) PatchFallbackFrom(messageID, from string) error {
	from = strings.TrimSpace(from)
	if s.db == nil || messageID == "" || from == "" {
		return nil
	}
	sessionMu.Lock()
	defer sessionMu.Unlock()
	return s.db.patchFallbackFrom(messageID, from)
}

// LatestToolUseByNameSuffix returns the newest tool_use in a session whose name
// ends with suffix (e.g. "__ask_user"), plus its provider tool-use id.
func (s *Store) LatestToolUseByNameSuffix(sessionID, suffix string) (domain.ToolUsePayload, bool) {
	if s.db == nil || sessionID == "" || suffix == "" {
		return domain.ToolUsePayload{}, false
	}
	return s.db.latestToolUseByNameSuffix(sessionID, suffix)
}

// CompleteMessage marks a message finished, stamping status, completion time,
// usage, and cost. Pass StreamError to record a failed turn.
func (s *Store) CompleteMessage(messageID, status string, usage domain.Message) error {
	if s.db == nil {
		return nil
	}
	return s.db.completeMessage(messageID, status, usage)
}

// SessionTranscript returns a session's messages in order, each with its content
// blocks — the single load path the renderer replays. The
// normalized transcript is the single source of truth; there is no separate raw
// provider-line store.
func (s *Store) SessionTranscript(sessionID string) ([]domain.Message, error) {
	if s.db == nil {
		return nil, nil
	}
	return s.db.sessionMessages(sessionID)
}

// SessionHasMessages is a cheap emptiness check (COUNT on session_message) so
// listing closed chats does not materialize every transcript.
func (s *Store) SessionHasMessages(sessionID string) bool {
	if s.db == nil || sessionID == "" {
		return false
	}
	return s.db.sessionHasMessages(sessionID)
}

// ForkSession creates a child session that shares the parent's messages up to
// and including forkMessageID. Ordering lives on the
// join table, so this is a row copy of session_message — messages themselves
// are not duplicated. kind and configJSON override the parent's values on the
// child (empty kind keeps the parent's). The child starts idle with no
// provider resume ref; the caller decides whether the next send forks the
// provider session or injects history.
func (s *Store) ForkSession(parentID, forkMessageID, kind, configJSON string) (domain.Session, error) {
	return s.forkSessionTo(parentID, forkMessageID, "", kind, configJSON)
}

// ForkSessionToWorkspace is ForkSession with the child attached to destWorkspaceID
// (a new workspace that should continue the copied transcript).
func (s *Store) ForkSessionToWorkspace(parentID, forkMessageID, destWorkspaceID, kind, configJSON string) (domain.Session, error) {
	if strings.TrimSpace(destWorkspaceID) == "" {
		return domain.Session{}, fmt.Errorf("fork session: destination workspace is required")
	}
	return s.forkSessionTo(parentID, forkMessageID, destWorkspaceID, kind, configJSON)
}

func (s *Store) forkSessionTo(parentID, forkMessageID, destWorkspaceID, kind, configJSON string) (domain.Session, error) {
	if s.db == nil {
		return domain.Session{}, fmt.Errorf("session store requires a database")
	}
	if parentID == "" || forkMessageID == "" {
		return domain.Session{}, fmt.Errorf("fork session: parent and fork message are required")
	}
	sessionMu.Lock()
	defer sessionMu.Unlock()
	return s.db.forkSession(parentID, forkMessageID, destWorkspaceID, kind, configJSON)
}

// SessionMessageIsTip reports whether messageID is the last message in the
// session (the current tip). Recur chat forks inject a transcript into a
// fresh provider session and do not use this for --fork-session.
func (s *Store) SessionMessageIsTip(sessionID, messageID string) bool {
	if s.db == nil || sessionID == "" || messageID == "" {
		return false
	}
	return s.db.sessionMessageIsTip(sessionID, messageID)
}

// SessionContainsMessage reports whether messageID is a join-row in sessionID.
// Used to validate a fork point before creating a destination workspace.
func (s *Store) SessionContainsMessage(sessionID, messageID string) bool {
	if s.db == nil || sessionID == "" || messageID == "" {
		return false
	}
	return s.db.sessionContainsMessage(sessionID, messageID)
}

// SessionFirstUserPrompt returns the first human chat-turn text (display_text
// when set, else wire text) without loading tool blocks. Tab titles and
// "has this chat started?" checks must not pull a 200MB+ message_content scan
// the way SessionTranscript does — switching workspaces lists every open and
// closed tab. Empty when the session has no user prompt yet (tool_result
// carrier rows are skipped).
func (s *Store) SessionFirstUserPrompt(sessionID string) string {
	if s.db == nil || sessionID == "" {
		return ""
	}
	return s.db.sessionFirstUserPrompt(sessionID)
}

// LastAssistantText returns the last assistant text block in this workspace's
// latest session (newest session by updated_at, then newest message, then newest
// text content). Prefers display_text (the UI copy) over wire text. Empty when
// that session has no assistant text.
func (s *Store) LastAssistantText(workspaceID string) string {
	if s.db == nil || strings.TrimSpace(workspaceID) == "" {
		return ""
	}
	return s.db.lastAssistantText(workspaceID)
}

// ---- db layer ----

func (d *db) reassignSessions(fromID, toID string, remap map[string]string) error {
	tx, err := d.sql.Begin()
	if err != nil {
		return fmt.Errorf("reassign sessions %q → %q: %w", fromID, toID, err)
	}
	defer tx.Rollback() //nolint:errcheck

	from, err := sessionsForLoopQuery(tx, fromID)
	if err != nil {
		return err
	}
	dest, err := sessionsForLoopQuery(tx, toID)
	if err != nil {
		return err
	}

	fromCodes := map[string]bool{}
	for _, s := range from {
		var cfg closedChatCfg
		_ = json.Unmarshal([]byte(s.ConfigJSON), &cfg)
		if cfg.Code != "" {
			fromCodes[cfg.Code] = true
		}
	}
	for _, s := range dest {
		var cfg closedChatCfg
		_ = json.Unmarshal([]byte(s.ConfigJSON), &cfg)
		if cfg.Code == "" || !fromCodes[cfg.Code] || remap[cfg.Code] != "" {
			continue
		}
		if sessionHasMessagesQuery(tx, s.ID) {
			continue
		}
		if err := deleteSessionTx(tx, s.ID); err != nil {
			return err
		}
	}

	for _, s := range from {
		var cfg closedChatCfg
		_ = json.Unmarshal([]byte(s.ConfigJSON), &cfg)
		newCode, newOwner := cfg.Code, cfg.OwnerCode
		if next := remap[cfg.Code]; next != "" {
			newCode = next
		}
		if next := remap[cfg.OwnerCode]; next != "" {
			newOwner = next
		}
		q := `UPDATE session SET workspace_id = ?`
		args := []any{toID}
		if newCode != cfg.Code || newOwner != cfg.OwnerCode {
			expr := `json_set(COALESCE(NULLIF(config_json, ''), '{}')`
			if newCode != cfg.Code {
				expr += `, '$.code', ?`
				args = append(args, newCode)
			}
			if newOwner != cfg.OwnerCode {
				expr += `, '$.ownerCode', ?`
				args = append(args, newOwner)
			}
			expr += `)`
			q += `, config_json = ` + expr
		}
		q += ` WHERE id = ?`
		args = append(args, s.ID)
		if _, err := tx.Exec(q, args...); err != nil {
			return fmt.Errorf("reassign sessions %q → %q: %w", fromID, toID, err)
		}
	}
	if err := tx.Commit(); err != nil {
		return fmt.Errorf("reassign sessions %q → %q: %w", fromID, toID, err)
	}
	return nil
}

func (d *db) insertSession(sess domain.Session) error {
	tx, err := d.sql.Begin()
	if err != nil {
		return err
	}
	defer tx.Rollback() //nolint:errcheck
	if err := insertSessionTx(tx, sess); err != nil {
		return err
	}
	return tx.Commit()
}

const sessionCols = `id, workspace_id, kind, parent_session_id, fork_message_id,
	context_mode, provider, provider_session_ref, config_json, active_context_id,
	status, created_at, updated_at`

func scanSession(row interface{ Scan(...any) error }) (domain.Session, error) {
	var s domain.Session
	var ws, parent, fork, ref, active sql.NullString
	err := row.Scan(&s.ID, &ws, &s.Kind, &parent, &fork,
		&s.ContextMode, &s.Provider, &ref, &s.ConfigJSON, &active,
		&s.Status, &s.CreatedAt, &s.UpdatedAt)
	if err != nil {
		return domain.Session{}, err
	}
	s.WorkspaceID, s.ParentSessionID = ws.String, parent.String
	s.ForkMessageID, s.ProviderSessionRef, s.ActiveContextID = fork.String, ref.String, active.String
	return s, nil
}

func (d *db) getSession(id string) (domain.Session, error) {
	row := d.sql.QueryRow(`SELECT `+sessionCols+` FROM session WHERE id = ?`, id)
	return scanSession(row)
}

func (d *db) sessionForLoopKind(workspaceID, kind string) (domain.Session, error) {
	row := d.sql.QueryRow(`SELECT `+sessionCols+` FROM session
		WHERE workspace_id = ? AND kind = ? ORDER BY created_at DESC LIMIT 1`, workspaceID, kind)
	return scanSession(row)
}

func (d *db) sessionForLoopAgent(workspaceID, code string) (domain.Session, error) {
	row := d.sql.QueryRow(`SELECT `+sessionCols+` FROM session
		WHERE workspace_id = ? AND json_extract(config_json, '$.code') = ?
		ORDER BY created_at DESC LIMIT 1`, workspaceID, code)
	return scanSession(row)
}

func (d *db) sessionForChatAgent(workspaceID, owner, code string) (domain.Session, error) {
	row := d.sql.QueryRow(`SELECT `+sessionCols+` FROM session
		WHERE workspace_id = ? AND json_extract(config_json, '$.code') = ?
		AND ifnull(json_extract(config_json, '$.ownerCode'), '') = ?
		ORDER BY created_at DESC LIMIT 1`, workspaceID, code, owner)
	return scanSession(row)
}

func (d *db) sessionsForLoop(workspaceID string) ([]domain.Session, error) {
	return sessionsForLoopQuery(d.sql, workspaceID)
}

type sessionQuerier interface {
	Query(query string, args ...any) (*sql.Rows, error)
}

func sessionsForLoopQuery(q sessionQuerier, workspaceID string) ([]domain.Session, error) {
	rows, err := q.Query(`SELECT `+sessionCols+` FROM session
		WHERE workspace_id = ? ORDER BY created_at, id`, workspaceID)
	if err != nil {
		return nil, fmt.Errorf("query sessions for loop %q: %w", workspaceID, err)
	}
	defer rows.Close()
	var out []domain.Session
	for rows.Next() {
		sess, err := scanSession(rows)
		if err != nil {
			return nil, err
		}
		out = append(out, sess)
	}
	return out, rows.Err()
}

func (d *db) sessionForAppKind(kind string) (domain.Session, error) {
	row := d.sql.QueryRow(`SELECT `+sessionCols+` FROM session
		WHERE workspace_id IS NULL AND kind = ? ORDER BY created_at DESC LIMIT 1`, kind)
	return scanSession(row)
}

func (d *db) sessionsForAppKind(kind string) ([]domain.Session, error) {
	rows, err := d.sql.Query(`SELECT `+sessionCols+` FROM session
		WHERE workspace_id IS NULL AND kind = ? ORDER BY created_at ASC, id ASC`, kind)
	if err != nil {
		return nil, fmt.Errorf("query app sessions: %w", err)
	}
	defer rows.Close()
	var out []domain.Session
	for rows.Next() {
		sess, err := scanSession(rows)
		if err != nil {
			return nil, err
		}
		out = append(out, sess)
	}
	return out, rows.Err()
}

func (d *db) sessionHeadersByWorkspace(capPer int) (map[string][]domain.Session, error) {
	if capPer <= 0 {
		capPer = 8
	}
	rows, err := d.sql.Query(`SELECT ` + sessionCols + ` FROM session
		WHERE workspace_id IS NOT NULL AND workspace_id != ''
		ORDER BY workspace_id, updated_at DESC, id DESC`)
	if err != nil {
		return nil, fmt.Errorf("query session headers: %w", err)
	}
	defer rows.Close()
	out := map[string][]domain.Session{}
	for rows.Next() {
		sess, err := scanSession(rows)
		if err != nil {
			return nil, err
		}
		list := out[sess.WorkspaceID]
		if len(list) >= capPer {
			continue
		}
		out[sess.WorkspaceID] = append(list, sess)
	}
	return out, rows.Err()
}

func (d *db) latestEventTSByWorkspace() (map[string]int64, error) {
	rows, err := d.sql.Query(`SELECT workspace_id, MAX(ts) FROM events
		WHERE workspace_id IS NOT NULL AND workspace_id != '' GROUP BY workspace_id`)
	if err != nil {
		return nil, fmt.Errorf("query latest event ts: %w", err)
	}
	defer rows.Close()
	out := map[string]int64{}
	for rows.Next() {
		var id string
		var ts int64
		if err := rows.Scan(&id, &ts); err != nil {
			return nil, err
		}
		out[id] = ts
	}
	return out, rows.Err()
}

func (d *db) streamingAppSessions() ([]domain.Session, error) {
	rows, err := d.sql.Query(`SELECT ` + sessionCols + ` FROM session
		WHERE workspace_id IS NULL AND status = 'streaming' ORDER BY updated_at DESC`)
	if err != nil {
		return nil, fmt.Errorf("query streaming app sessions: %w", err)
	}
	defer rows.Close()
	var out []domain.Session
	for rows.Next() {
		sess, err := scanSession(rows)
		if err != nil {
			return nil, err
		}
		out = append(out, sess)
	}
	return out, rows.Err()
}

func (d *db) writeSessionContext(sessionID, summary, lastCompactedMessageID string) (domain.SessionContext, error) {
	now := time.Now().UnixMilli()
	ctx := domain.SessionContext{
		ID:                     uuid.Must(uuid.NewV7()).String(),
		SessionID:              sessionID,
		Summary:                summary,
		LastCompactedMessageID: lastCompactedMessageID,
		CreatedAt:              now,
	}
	tx, err := d.sql.Begin()
	if err != nil {
		return domain.SessionContext{}, err
	}
	defer tx.Rollback() //nolint:errcheck
	if _, err := tx.Exec(
		`INSERT INTO session_context (id, session_id, summary, last_compacted_message_id, created_at)
		 VALUES (?,?,?,?,?)`,
		ctx.ID, ctx.SessionID, ctx.Summary, ctx.LastCompactedMessageID, ctx.CreatedAt,
	); err != nil {
		return domain.SessionContext{}, fmt.Errorf("insert session_context: %w", err)
	}
	if _, err := tx.Exec(
		`UPDATE session SET active_context_id = ?, updated_at = ? WHERE id = ?`,
		ctx.ID, now, sessionID,
	); err != nil {
		return domain.SessionContext{}, fmt.Errorf("set active_context_id: %w", err)
	}
	if err := tx.Commit(); err != nil {
		return domain.SessionContext{}, err
	}
	return ctx, nil
}

func (d *db) activeSessionContext(sessionID string) (domain.SessionContext, error) {
	row := d.sql.QueryRow(
		`SELECT c.id, c.session_id, c.summary, c.last_compacted_message_id, c.created_at
		 FROM session s
		 JOIN session_context c ON c.id = s.active_context_id
		 WHERE s.id = ?`,
		sessionID,
	)
	var c domain.SessionContext
	err := row.Scan(&c.ID, &c.SessionID, &c.Summary, &c.LastCompactedMessageID, &c.CreatedAt)
	if err != nil {
		return domain.SessionContext{}, err
	}
	return c, nil
}

func (d *db) updateSessionRef(id, ref string) error {
	_, err := d.sql.Exec(`UPDATE session SET provider_session_ref = ?, updated_at = ? WHERE id = ?`,
		ref, time.Now().UnixMilli(), id)
	if err != nil {
		return fmt.Errorf("update session ref %q: %w", id, err)
	}
	return nil
}

func (d *db) updateSessionProvider(id, provider string) error {
	_, err := d.sql.Exec(`UPDATE session SET provider = ?, updated_at = ? WHERE id = ?`,
		provider, time.Now().UnixMilli(), id)
	if err != nil {
		return fmt.Errorf("update session provider %q: %w", id, err)
	}
	return nil
}

func (d *db) setSessionStatus(id, status string) error {
	_, err := d.sql.Exec(`UPDATE session SET status = ?, updated_at = ? WHERE id = ?`,
		status, time.Now().UnixMilli(), id)
	if err != nil {
		return fmt.Errorf("set session status %q: %w", id, err)
	}
	return nil
}

func (d *db) updateSessionConfigJSON(id, configJSON string) error {
	_, err := d.sql.Exec(`UPDATE session SET config_json = ?, updated_at = ? WHERE id = ?`,
		configJSON, time.Now().UnixMilli(), id)
	if err != nil {
		return fmt.Errorf("update session config %q: %w", id, err)
	}
	return nil
}

func (d *db) patchSessionConfigTitle(id, title string) error {
	_, err := d.sql.Exec(
		`UPDATE session SET config_json = json_set(COALESCE(NULLIF(config_json, ''), '{}'), '$.title', ?), updated_at = ? WHERE id = ?`,
		title, time.Now().UnixMilli(), id,
	)
	if err != nil {
		return fmt.Errorf("patch session title %q: %w", id, err)
	}
	return nil
}

func (d *db) patchSessionConfigSimplify(id string, on bool) error {
	_, err := d.sql.Exec(
		`UPDATE session SET config_json = json_set(COALESCE(NULLIF(config_json, ''), '{}'), '$.simplify', iif(?, json('true'), json('false'))), updated_at = ? WHERE id = ?`,
		on, time.Now().UnixMilli(), id,
	)
	if err != nil {
		return fmt.Errorf("patch session simplify %q: %w", id, err)
	}
	return nil
}

func (d *db) patchSessionConfigSimplifyIfUnset(id string, on bool) error {
	_, err := d.sql.Exec(
		`UPDATE session SET config_json = json_set(COALESCE(NULLIF(config_json, ''), '{}'), '$.simplify', iif(?, json('true'), json('false'))), updated_at = ?
		 WHERE id = ? AND json_extract(COALESCE(NULLIF(config_json, ''), '{}'), '$.simplify') IS NULL`,
		on, time.Now().UnixMilli(), id,
	)
	if err != nil {
		return fmt.Errorf("patch session simplify if unset %q: %w", id, err)
	}
	return nil
}

func (d *db) patchSessionConfigMerge(id, patchJSON string) error {
	_, err := d.sql.Exec(
		`UPDATE session SET config_json = json_patch(COALESCE(NULLIF(config_json, ''), '{}'), json(?)), updated_at = ? WHERE id = ?`,
		patchJSON, time.Now().UnixMilli(), id,
	)
	if err != nil {
		return fmt.Errorf("patch session config merge %q: %w", id, err)
	}
	return nil
}

func (d *db) patchSessionConfigFlags(id, flagsJSON string) error {
	_, err := d.sql.Exec(
		`UPDATE session SET config_json = json_set(COALESCE(NULLIF(config_json, ''), '{}'), '$.flags', json(?)), updated_at = ? WHERE id = ?`,
		flagsJSON, time.Now().UnixMilli(), id,
	)
	if err != nil {
		return fmt.Errorf("patch session flags %q: %w", id, err)
	}
	return nil
}

func (d *db) patchSessionConfigDock(id, patchJSON, flagsJSON string) error {
	patchJSON = strings.TrimSpace(patchJSON)
	flagsJSON = strings.TrimSpace(flagsJSON)
	if patchJSON == "" {
		patchJSON = "{}"
	}
	if flagsJSON == "" {
		return d.patchSessionConfigMerge(id, patchJSON)
	}
	_, err := d.sql.Exec(
		`UPDATE session SET config_json = json_set(json_patch(COALESCE(NULLIF(config_json, ''), '{}'), json(?)), '$.flags', json(?)), updated_at = ? WHERE id = ?`,
		patchJSON, flagsJSON, time.Now().UnixMilli(), id,
	)
	if err != nil {
		return fmt.Errorf("patch session dock %q: %w", id, err)
	}
	return nil
}

// nextSessionIdxTx returns the next session-relative message position.
func nextSessionIdxTx(tx *sql.Tx, sessionID string) (int, error) {
	var idx sql.NullInt64
	if err := tx.QueryRow(`SELECT MAX(idx) FROM session_message WHERE session_id = ?`, sessionID).Scan(&idx); err != nil {
		return 0, fmt.Errorf("next session idx: %w", err)
	}
	if !idx.Valid {
		return 0, nil
	}
	return int(idx.Int64) + 1, nil
}

func (d *db) appendMessage(sessionID string, m domain.Message) (domain.Message, error) {
	if m.ID == "" {
		m.ID = uuid.Must(uuid.NewV7()).String()
	}
	if m.OriginSessionID == "" {
		m.OriginSessionID = sessionID
	}
	if m.CreatedAt == 0 {
		m.CreatedAt = time.Now().UnixMilli()
	}
	if m.StreamStatus == "" {
		m.StreamStatus = domain.StreamComplete
	}
	tx, err := d.sql.Begin()
	if err != nil {
		return domain.Message{}, err
	}
	defer tx.Rollback() //nolint:errcheck
	idx, err := nextSessionIdxTx(tx, sessionID)
	if err != nil {
		return domain.Message{}, err
	}
	if err := insertMessageTx(tx, m); err != nil {
		return domain.Message{}, err
	}
	if err := insertSessionMessageTx(tx, sessionID, m.ID, idx); err != nil {
		return domain.Message{}, err
	}
	for bi, b := range m.Content {
		if b.ID == "" {
			b.ID = uuid.Must(uuid.NewV7()).String()
			m.Content[bi].ID = b.ID
		}
		if err := insertContentBlockTx(tx, m.ID, bi, b); err != nil {
			return domain.Message{}, err
		}
	}
	if err := tx.Commit(); err != nil {
		return domain.Message{}, err
	}
	return m, nil
}

func insertMessageTx(x execer, m domain.Message) error {
	_, err := x.Exec(`INSERT INTO message
		(id, origin_session_id, role, model_id, provider, stream_status,
		 started_at, completed_at, input_tokens, output_tokens,
		 cache_creation_tokens, cache_read_tokens, created_at)
		VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?)`,
		m.ID, m.OriginSessionID, m.Role, nullStr(m.ModelID), nullStr(m.Provider),
		m.StreamStatus, nullInt(m.StartedAt), nullInt(m.CompletedAt),
		nullInt(m.InputTokens), nullInt(m.OutputTokens),
		nullInt(m.CacheCreationTokens), nullInt(m.CacheReadTokens),
		m.CreatedAt)
	if err != nil {
		return fmt.Errorf("insert message %q: %w", m.ID, err)
	}
	return nil
}

func insertSessionMessageTx(x execer, sessionID, messageID string, idx int) error {
	_, err := x.Exec(`INSERT INTO session_message (session_id, message_id, idx) VALUES (?,?,?)`,
		sessionID, messageID, idx)
	if err != nil {
		return fmt.Errorf("insert session_message %s/%s: %w", sessionID, messageID, err)
	}
	return nil
}

func insertContentBlockTx(x execer, messageID string, idx int, b domain.ContentBlock) error {
	if b.ID == "" {
		b.ID = uuid.Must(uuid.NewV7()).String()
	}
	if len(b.ContentJSON) == 0 {
		b.ContentJSON = []byte(`{}`)
	}
	_, err := x.Exec(`INSERT INTO message_content (id, message_id, idx, type, content_json, created_at)
		VALUES (?,?,?,?,?,?)`,
		b.ID, messageID, idx, b.Type, string(b.ContentJSON), time.Now().UnixMilli())
	if err != nil {
		return fmt.Errorf("insert content block %q: %w", b.ID, err)
	}
	return nil
}

func (d *db) appendContentBlock(messageID string, b domain.ContentBlock) error {
	var idx sql.NullInt64
	if err := d.sql.QueryRow(`SELECT MAX(idx) FROM message_content WHERE message_id = ?`, messageID).Scan(&idx); err != nil {
		return fmt.Errorf("next content idx: %w", err)
	}
	next := 0
	if idx.Valid {
		next = int(idx.Int64) + 1
	}
	return insertContentBlockTx(d.sql, messageID, next, b)
}

func (d *db) patchToolUseInput(internalToolUseID string, input json.RawMessage) error {
	var id, payload string
	err := d.sql.QueryRow(
		`SELECT id, content_json FROM message_content
		 WHERE type = ? AND json_extract(content_json, '$.id') = ?
		 ORDER BY created_at DESC LIMIT 1`,
		domain.BlockToolUse, internalToolUseID,
	).Scan(&id, &payload)
	if err == sql.ErrNoRows {
		return nil
	}
	if err != nil {
		return fmt.Errorf("lookup tool_use %q: %w", internalToolUseID, err)
	}
	var tu domain.ToolUsePayload
	if err := json.Unmarshal([]byte(payload), &tu); err != nil {
		return fmt.Errorf("decode tool_use %q: %w", internalToolUseID, err)
	}
	tu.Input = append(json.RawMessage(nil), input...)
	b, err := json.Marshal(tu)
	if err != nil {
		return err
	}
	if _, err := d.sql.Exec(`UPDATE message_content SET content_json = ? WHERE id = ?`, string(b), id); err != nil {
		return fmt.Errorf("patch tool_use %q: %w", internalToolUseID, err)
	}
	return nil
}

func (d *db) patchFallbackFrom(messageID, from string) error {
	var id, payload string
	err := d.sql.QueryRow(
		`SELECT id, content_json FROM message_content
		 WHERE message_id = ? AND type = ?
		 ORDER BY idx DESC LIMIT 1`,
		messageID, domain.BlockFallback,
	).Scan(&id, &payload)
	if err == sql.ErrNoRows {
		return nil
	}
	if err != nil {
		return fmt.Errorf("lookup fallback %s: %w", messageID, err)
	}
	var p domain.FallbackPayload
	if err := json.Unmarshal([]byte(payload), &p); err != nil {
		return fmt.Errorf("decode fallback %s: %w", messageID, err)
	}
	if strings.TrimSpace(p.From) != "" {
		return nil
	}
	p.From = from
	b, err := json.Marshal(p)
	if err != nil {
		return err
	}
	if _, err := d.sql.Exec(`UPDATE message_content SET content_json = ? WHERE id = ?`, string(b), id); err != nil {
		return fmt.Errorf("patch fallback %s: %w", messageID, err)
	}
	return nil
}

func (d *db) latestToolUseByNameSuffix(sessionID, suffix string) (domain.ToolUsePayload, bool) {
	var payload string
	// session_message links messages to sessions (message has no session_id).
	// Exact suffix match — LIKE '_' is a wildcard, so avoid it for "__ask_user".
	err := d.sql.QueryRow(
		`SELECT mc.content_json FROM message_content mc
		 JOIN session_message sm ON sm.message_id = mc.message_id
		 WHERE sm.session_id = ? AND mc.type = ?
		   AND substr(json_extract(mc.content_json, '$.name'), -?) = ?
		 ORDER BY mc.created_at DESC LIMIT 1`,
		sessionID, domain.BlockToolUse, len(suffix), suffix,
	).Scan(&payload)
	if err != nil {
		return domain.ToolUsePayload{}, false
	}
	var tu domain.ToolUsePayload
	if json.Unmarshal([]byte(payload), &tu) != nil || tu.ID == "" {
		return domain.ToolUsePayload{}, false
	}
	return tu, true
}

func (d *db) completeMessage(messageID, status string, u domain.Message) error {
	_, err := d.sql.Exec(`UPDATE message SET
		stream_status = ?, completed_at = ?,
		input_tokens = ?, output_tokens = ?,
		cache_creation_tokens = ?, cache_read_tokens = ?,
		duration_ms = ?, context_window = ?
		WHERE id = ?`,
		status, time.Now().UnixMilli(),
		nullInt(u.InputTokens), nullInt(u.OutputTokens),
		nullInt(u.CacheCreationTokens), nullInt(u.CacheReadTokens),
		nullInt(u.DurationMS), nullInt(u.ContextWindow), messageID)
	if err != nil {
		return fmt.Errorf("complete message %q: %w", messageID, err)
	}
	return nil
}

func (d *db) forkSession(parentID, forkMessageID, destWorkspaceID, kind, configJSON string) (domain.Session, error) {
	parent, err := d.getSession(parentID)
	if err != nil {
		return domain.Session{}, fmt.Errorf("fork session: load parent: %w", err)
	}
	var forkIdx sql.NullInt64
	err = d.sql.QueryRow(
		`SELECT idx FROM session_message WHERE session_id = ? AND message_id = ?`,
		parentID, forkMessageID,
	).Scan(&forkIdx)
	if err == sql.ErrNoRows || !forkIdx.Valid {
		return domain.Session{}, fmt.Errorf("fork session: message %q is not in session %q", forkMessageID, parentID)
	}
	if err != nil {
		return domain.Session{}, fmt.Errorf("fork session: lookup fork point: %w", err)
	}
	if strings.TrimSpace(kind) == "" {
		kind = parent.Kind
	}
	if configJSON == "" {
		configJSON = parent.ConfigJSON
	}
	if configJSON == "" {
		configJSON = "{}"
	}
	workspaceID := parent.WorkspaceID
	if dest := strings.TrimSpace(destWorkspaceID); dest != "" {
		workspaceID = dest
	}
	now := time.Now().UnixMilli()
	child := domain.Session{
		ID:              uuid.Must(uuid.NewV7()).String(),
		WorkspaceID:     workspaceID,
		Kind:            kind,
		ParentSessionID: parent.ID,
		ForkMessageID:   forkMessageID,
		ContextMode:     parent.ContextMode,
		Provider:        parent.Provider,
		ConfigJSON:      configJSON,
		Status:          "idle",
		CreatedAt:       now,
		UpdatedAt:       now,
	}
	tx, err := d.sql.Begin()
	if err != nil {
		return domain.Session{}, err
	}
	defer tx.Rollback() //nolint:errcheck
	if err := insertSessionTx(tx, child); err != nil {
		return domain.Session{}, err
	}
	_, err = tx.Exec(
		`INSERT INTO session_message (session_id, message_id, idx)
		 SELECT ?, message_id, idx FROM session_message
		 WHERE session_id = ? AND idx <= ?`,
		child.ID, parentID, forkIdx.Int64,
	)
	if err != nil {
		return domain.Session{}, fmt.Errorf("fork session: copy messages: %w", err)
	}
	if parent.ActiveContextID != "" {
		copied, err := copySessionContextTx(tx, parent.ActiveContextID, child.ID, now)
		if err != nil {
			return domain.Session{}, err
		}
		child.ActiveContextID = copied
	}
	if err := tx.Commit(); err != nil {
		return domain.Session{}, err
	}
	return child, nil
}

// copySessionContextTx clones the parent's active compaction row onto the child
// when the watermark message was copied. Empty return means the fork point is
// before the compact, so the child has no active context.
func copySessionContextTx(tx *sql.Tx, parentContextID, childSessionID string, now int64) (string, error) {
	var summary, watermark string
	err := tx.QueryRow(
		`SELECT summary, last_compacted_message_id FROM session_context WHERE id = ?`,
		parentContextID,
	).Scan(&summary, &watermark)
	if err == sql.ErrNoRows {
		return "", nil
	}
	if err != nil {
		return "", fmt.Errorf("fork session: load parent context: %w", err)
	}
	var n int
	if err := tx.QueryRow(
		`SELECT COUNT(*) FROM session_message WHERE session_id = ? AND message_id = ?`,
		childSessionID, watermark,
	).Scan(&n); err != nil {
		return "", fmt.Errorf("fork session: check context watermark: %w", err)
	}
	if n == 0 {
		return "", nil
	}
	ctxID := uuid.Must(uuid.NewV7()).String()
	if _, err := tx.Exec(
		`INSERT INTO session_context (id, session_id, summary, last_compacted_message_id, created_at)
		 VALUES (?,?,?,?,?)`,
		ctxID, childSessionID, summary, watermark, now,
	); err != nil {
		return "", fmt.Errorf("fork session: copy context: %w", err)
	}
	if _, err := tx.Exec(
		`UPDATE session SET active_context_id = ? WHERE id = ?`,
		ctxID, childSessionID,
	); err != nil {
		return "", fmt.Errorf("fork session: set child context: %w", err)
	}
	return ctxID, nil
}

func insertSessionTx(tx *sql.Tx, sess domain.Session) error {
	_, err := tx.Exec(`INSERT INTO session
		(id, workspace_id, kind, parent_session_id, fork_message_id,
		 context_mode, provider, provider_session_ref, config_json,
		 active_context_id, status, created_at, updated_at)
		VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?)`,
		sess.ID, nullStr(sess.WorkspaceID), sess.Kind,
		nullStr(sess.ParentSessionID), nullStr(sess.ForkMessageID),
		sess.ContextMode, sess.Provider, nullStr(sess.ProviderSessionRef),
		sess.ConfigJSON, nullStr(sess.ActiveContextID), sess.Status,
		sess.CreatedAt, sess.UpdatedAt)
	if err != nil {
		return fmt.Errorf("insert session %q: %w", sess.ID, err)
	}
	return nil
}

func (d *db) sessionMessageIsTip(sessionID, messageID string) bool {
	var n int
	err := d.sql.QueryRow(
		`SELECT COUNT(*) FROM session_message
		 WHERE session_id = ? AND message_id = ?
		   AND idx = (SELECT MAX(idx) FROM session_message WHERE session_id = ?)`,
		sessionID, messageID, sessionID,
	).Scan(&n)
	return err == nil && n == 1
}

func (d *db) sessionContainsMessage(sessionID, messageID string) bool {
	var n int
	err := d.sql.QueryRow(
		`SELECT COUNT(*) FROM session_message WHERE session_id = ? AND message_id = ?`,
		sessionID, messageID,
	).Scan(&n)
	return err == nil && n > 0
}

func sessionHasMessagesQuery(q interface{ QueryRow(string, ...any) *sql.Row }, sessionID string) bool {
	var n int
	err := q.QueryRow(`SELECT COUNT(*) FROM session_message WHERE session_id = ?`, sessionID).Scan(&n)
	return err == nil && n > 0
}

func (d *db) sessionHasMessages(sessionID string) bool {
	return sessionHasMessagesQuery(d.sql, sessionID)
}

func (d *db) sessionFirstUserPrompt(sessionID string) string {
	// First role=user message that is a chat prompt (has text/image, no
	// tool_result/tool_use/thinking). Only those text payloads are read —
	// never the tool-result blobs that dominate long transcripts.
	rows, err := d.sql.Query(`SELECT mc.content_json
		FROM message_content mc
		WHERE mc.message_id = (
			SELECT m.id
			FROM message m
			JOIN session_message sm ON sm.message_id = m.id
			WHERE sm.session_id = ?
			  AND m.role = 'user'
			  AND NOT EXISTS (
				SELECT 1 FROM message_content x
				WHERE x.message_id = m.id
				  AND x.type IN (?, ?, ?)
			  )
			  AND EXISTS (
				SELECT 1 FROM message_content y
				WHERE y.message_id = m.id
				  AND y.type IN (?, ?)
			  )
			ORDER BY sm.idx
			LIMIT 1
		)
		AND mc.type = ?
		ORDER BY mc.idx`,
		sessionID,
		domain.BlockToolResult, domain.BlockToolUse, domain.BlockThinking,
		domain.BlockText, domain.BlockImage,
		domain.BlockText,
	)
	if err != nil {
		return ""
	}
	defer rows.Close()
	var text strings.Builder
	for rows.Next() {
		var payload string
		if err := rows.Scan(&payload); err != nil {
			return ""
		}
		var p domain.TextPayload
		if json.Unmarshal([]byte(payload), &p) != nil {
			continue
		}
		shown := p.DisplayText
		if shown == "" {
			shown = p.Text
		}
		if strings.TrimSpace(shown) == "" {
			continue
		}
		text.WriteString(shown)
	}
	if err := rows.Err(); err != nil {
		return ""
	}
	return text.String()
}

func (d *db) lastAssistantText(workspaceID string) string {
	var payload string
	err := d.sql.QueryRow(`SELECT mc.content_json
		FROM message_content mc
		JOIN message m ON m.id = mc.message_id
		JOIN session_message sm ON sm.message_id = m.id
		WHERE sm.session_id = (
			SELECT id FROM session
			WHERE workspace_id = ?
			ORDER BY updated_at DESC, created_at DESC, id DESC
			LIMIT 1
		)
		AND m.role = 'assistant'
		AND mc.type = ?
		AND (
			length(trim(coalesce(json_extract(mc.content_json, '$.text'), ''))) > 0
			OR length(trim(coalesce(json_extract(mc.content_json, '$.display_text'), ''))) > 0
		)
		ORDER BY sm.idx DESC, mc.idx DESC
		LIMIT 1`,
		workspaceID, domain.BlockText,
	).Scan(&payload)
	if err != nil {
		return ""
	}
	var p domain.TextPayload
	if json.Unmarshal([]byte(payload), &p) != nil {
		return ""
	}
	text := strings.TrimSpace(p.DisplayText)
	if text == "" {
		text = strings.TrimSpace(p.Text)
	}
	return text
}

func (d *db) sessionMessages(sessionID string) ([]domain.Message, error) {
	// One JOIN instead of 1+N: switching a workspace loads the open chat
	// transcript, and a quiet-looking tab can still have hundreds of tool
	// blocks in message_content (a 200MB+ table in long-lived Recur DBs).
	rows, err := d.sql.Query(`SELECT `+messageCols+`,
			mc.id, mc.type, mc.content_json, mc.created_at
		FROM message m
		JOIN session_message sm ON sm.message_id = m.id
		LEFT JOIN message_content mc ON mc.message_id = m.id
		WHERE sm.session_id = ?
		ORDER BY sm.idx, mc.idx`, sessionID)
	if err != nil {
		return nil, fmt.Errorf("query session messages: %w", err)
	}
	defer rows.Close()
	var out []domain.Message
	for rows.Next() {
		var blockID, blockType, payload sql.NullString
		var blockAt sql.NullInt64
		m, err := scanMessage(rows, &blockID, &blockType, &payload, &blockAt)
		if err != nil {
			return nil, err
		}
		if len(out) == 0 || out[len(out)-1].ID != m.ID {
			out = append(out, m)
		}
		if !blockID.Valid {
			continue
		}
		last := &out[len(out)-1]
		last.Content = append(last.Content, domain.ContentBlock{
			ID:          blockID.String,
			Type:        blockType.String,
			ContentJSON: []byte(payload.String),
			CreatedAt:   blockAt.Int64,
		})
	}
	return out, rows.Err()
}

const messageCols = `m.id, m.origin_session_id, m.role, m.model_id, m.provider,
	m.stream_status, m.started_at, m.completed_at, m.input_tokens, m.output_tokens,
	m.cache_creation_tokens, m.cache_read_tokens, m.duration_ms,
	m.context_window, m.created_at`

func scanMessage(row interface{ Scan(...any) error }, extra ...any) (domain.Message, error) {
	var m domain.Message
	var model, provider sql.NullString
	var started, completed, in, out, cc, cr, dur, win sql.NullInt64
	dest := []any{&m.ID, &m.OriginSessionID, &m.Role, &model, &provider,
		&m.StreamStatus, &started, &completed, &in, &out, &cc, &cr, &dur, &win, &m.CreatedAt}
	dest = append(dest, extra...)
	if err := row.Scan(dest...); err != nil {
		return domain.Message{}, fmt.Errorf("scan message: %w", err)
	}
	m.ModelID, m.Provider = model.String, provider.String
	m.StartedAt, m.CompletedAt = started.Int64, completed.Int64
	m.InputTokens, m.OutputTokens = in.Int64, out.Int64
	m.CacheCreationTokens, m.CacheReadTokens = cc.Int64, cr.Int64
	m.DurationMS = dur.Int64
	m.ContextWindow = win.Int64
	return m, nil
}

// ---- null helpers (empty/zero → SQL NULL so optional columns stay NULL) ----

func nullStr(s string) any {
	if s == "" {
		return nil
	}
	return s
}

func nullInt(n int64) any {
	if n == 0 {
		return nil
	}
	return n
}
