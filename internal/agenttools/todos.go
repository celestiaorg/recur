package agenttools

import (
	"encoding/json"
	"sync"
)

// TodoItem is one session-scoped todo row.
type TodoItem struct {
	ID      string `json:"id"`
	Content string `json:"content"`
	Status  string `json:"status"`
}

// TodoStore holds in-process todos keyed by Recur session id.
type TodoStore struct {
	mu sync.Mutex
	m  map[string][]TodoItem
}

// Get returns a copy of the session's todos.
func (s *TodoStore) Get(sessionID string) []TodoItem {
	if s == nil || sessionID == "" {
		return nil
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	src := s.m[sessionID]
	out := make([]TodoItem, len(src))
	copy(out, src)
	return out
}

// Put replaces the session's todos.
func (s *TodoStore) Put(sessionID string, items []TodoItem) {
	if s == nil || sessionID == "" {
		return
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	if s.m == nil {
		s.m = map[string][]TodoItem{}
	}
	cp := make([]TodoItem, len(items))
	copy(cp, items)
	s.m[sessionID] = cp
}

func mergeTodos(cur, incoming []TodoItem) []TodoItem {
	byID := map[string]int{}
	out := append([]TodoItem(nil), cur...)
	for i, t := range out {
		if t.ID != "" {
			byID[t.ID] = i
		}
	}
	for _, t := range incoming {
		if t.ID == "" {
			continue
		}
		if i, ok := byID[t.ID]; ok {
			if t.Content != "" {
				out[i].Content = t.Content
			}
			if t.Status != "" {
				out[i].Status = t.Status
			}
		} else {
			if t.Status == "" {
				t.Status = "pending"
			}
			byID[t.ID] = len(out)
			out = append(out, t)
		}
	}
	return out
}

func todosJSON(items []TodoItem) string {
	b, err := json.Marshal(items)
	if err != nil {
		return "[]"
	}
	return string(b)
}
