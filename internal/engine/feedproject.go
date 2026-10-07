package engine

import (
	"encoding/json"
	"html"
	"sort"

	"recur/internal/domain"
)

// The activity feed is a PROJECTION: agent activity — tool
// rows and narration notes — derives from the typed session transcripts
// (message_content), and loop-level facts — outcomes and deviations — come from
// the durable events table. This retires reconstructing those agent rows from
// pre-rendered loop.event HTML strings; sessions are the source of truth.
//
// The live feed still streams typed FeedItems on the Bus as a loop runs (unchanged
// UX); this projection is the RELOAD/reconstruct path, so "served from sessions"
// holds without disturbing live streaming. A loop with no sessions (a pre-mirror
// loop from before this model existed) falls back to the legacy events replay.

// feedRoles are the session kinds (roles) whose turns emit activity-feed rows —
// the specialists whose turns run through Loop.onAgentEvent. The Maestro (planning
// + chat) mirrors transcripts too, but never rendered tool/note rows into the
// feed, so projecting them would invent rows the live feed never showed.
var feedRoles = map[string]bool{
	string(domain.RoleCoder):    true,
	string(domain.RoleReviewer): true,
	string(domain.RoleQA):       true,
}

// timedFeedItem carries a feed item with the millisecond timestamp used to merge
// the two projection sources into one chronological stream.
type timedFeedItem struct {
	ts   int64
	item domain.FeedItem
}

// ReconstructFeed rebuilds a loop's activity feed for the reload path. For a loop
// that has mirrored sessions it is a projection (agent rows from message_content +
// loop facts from events, merged newest-first); for a pre-mirror loop it falls
// back to replaying the durable loop.event rows (the original behavior).
func (e *Engine) ReconstructFeed(workspaceID string) []domain.FeedItem {
	sessions := e.Store.SessionsForLoop(workspaceID)
	if len(sessions) == 0 {
		return e.reconstructFromEvents(workspaceID)
	}

	var all []timedFeedItem
	all = append(all, e.feedRowsFromSessions(sessions)...)
	all = append(all, e.loopFactRows(workspaceID)...)
	if len(all) == 0 {
		// Sessions exist but produced nothing (e.g. only a maestro chat) — fall back
		// so a loop's engine-authored milestones still reconstruct.
		return e.reconstructFromEvents(workspaceID)
	}

	// Merge chronologically. Ties (rapid same-millisecond events, common only under
	// scripted tests) keep insertion order: session rows were appended before fact
	// rows, matching the live order where a phase outcome follows the turn's tools.
	sort.SliceStable(all, func(i, j int) bool { return all[i].ts < all[j].ts })
	out := make([]domain.FeedItem, 0, len(all))
	for i := len(all) - 1; i >= 0; i-- { // newest-first, like the live feed
		out = append(out, all[i].item)
	}
	return out
}

// feedRowsFromSessions projects tool rows and narration notes out of the mirrored
// specialist transcripts, reproducing exactly what Loop.onAgentEvent emitted live:
// a tool_use block → a "Name: <code>summary</code>" tool row (its tool_result
// patched in), an assistant text block → a note. The row identity (avatar code +
// display name) comes from the session's stored config.
func (e *Engine) feedRowsFromSessions(sessions []domain.Session) []timedFeedItem {
	var out []timedFeedItem
	for _, sess := range sessions {
		if !feedRoles[sess.Kind] {
			continue
		}
		var cfg agentConfig
		_ = json.Unmarshal([]byte(sess.ConfigJSON), &cfg)
		click := cfg.Code
		if click == "MA" { // Maestro rows aren't click-throughs (matches emitWorkspaceEventItem)
			click = ""
		}
		toolIdx := map[string]int{} // internal tool id → index in out
		msgs, err := e.Store.SessionTranscript(sess.ID)
		if err != nil {
			continue
		}
		for _, msg := range msgs {
			for _, b := range msg.Content {
				switch b.Type {
				case domain.BlockText:
					if msg.Role != "assistant" {
						continue // the user prompt is not a feed row
					}
					p, _ := b.AsText()
					t := truncate(p.Text, feedInputLimit)
					if t == "" {
						continue
					}
					out = append(out, timedFeedItem{ts: b.CreatedAt, item: domain.FeedItem{
						ID:         "s-" + b.ID,
						AgentCode:  cfg.Code,
						Name:       cfg.Name,
						Text:       html.EscapeString(t),
						Time:       "now",
						Kind:       domain.FeedKindNote,
						ClickAgent: click,
					}})
				case domain.BlockToolUse:
					p, _ := b.AsToolUse()
					// Blocks hold the provider's RAW name and args; the live feed
					// already showed the common vocabulary (onAgentEvent uses
					// displayToolFor). Derive it here so a reload matches live —
					// otherwise Cursor delete renders as `delete: {"path":…}`.
					d := displayTool(msg.Provider, p.Name, p.Input)
					text := html.EscapeString(d.Name)
					if summary := summarizeToolInput(d.Name, d.Input); summary != "" {
						text += ": <code>" + html.EscapeString(summary) + "</code>"
					}
					out = append(out, timedFeedItem{ts: b.CreatedAt, item: domain.FeedItem{
						ID:         "s-" + b.ID,
						AgentCode:  cfg.Code,
						Name:       cfg.Name,
						Text:       text,
						Time:       "now",
						Kind:       domain.FeedKindTool,
						ToolUseID:  p.ProviderToolUseID,
						ClickAgent: click,
					}})
					toolIdx[p.ID] = len(out) - 1
				case domain.BlockToolResult:
					p, _ := b.AsToolResult()
					if idx, ok := toolIdx[p.ToolUseID]; ok {
						out[idx].item.ToolResult = truncate(p.Result, feedInputLimit)
						out[idx].item.IsToolError = p.IsError
					}
				}
			}
		}
	}
	return out
}

// loopFactRows pulls the loop-level facts — outcomes and deviations — from the
// durable events table (the source the events table remains authoritative for).
// Tool/note rows are intentionally skipped here; those come
// from the session projection.
func (e *Engine) loopFactRows(workspaceID string) []timedFeedItem {
	evs, err := e.Store.EventsForLoop(workspaceID)
	if err != nil {
		return nil
	}
	var out []timedFeedItem
	seen := map[string]int{}
	for _, ev := range evs {
		if ev.Type != "workspace.event" {
			continue
		}
		fi, ok := feedItemFromEvent(ev.PayloadJSON)
		if !ok {
			continue
		}
		if fi.Kind != domain.FeedKindOutcome && fi.Kind != domain.FeedKindDeviation && !fi.IsDeviation {
			continue
		}
		if idx, dup := seen[fi.ID]; dup {
			// A re-emitted item (tool-result patch, card progress) replaces its
			// payload but keeps its FIRST position, so a wave card does not jump
			// to the bottom each time a child settles.
			out[idx].item = fi
			continue
		}
		seen[fi.ID] = len(out)
		out = append(out, timedFeedItem{ts: ev.TS, item: fi})
	}
	return out
}

// reconstructFromEvents is the pre-session (legacy) reload path: replay every
// loop.event FeedItem oldest→newest, dedup by id keeping the latest payload, and
// return newest-first. Retained for loops created before the session mirror
// existed (they have no sessions to project from).
func (e *Engine) reconstructFromEvents(workspaceID string) []domain.FeedItem {
	evs, err := e.Store.EventsForLoop(workspaceID)
	if err != nil {
		return nil
	}
	byID := map[string]domain.FeedItem{}
	order := make([]string, 0)
	for _, ev := range evs {
		if ev.Type != "workspace.event" {
			continue
		}
		fi, ok := feedItemFromEvent(ev.PayloadJSON)
		if !ok {
			continue
		}
		if _, seen := byID[fi.ID]; !seen {
			order = append(order, fi.ID)
		}
		byID[fi.ID] = fi
	}
	if len(order) == 0 {
		return nil
	}
	items := make([]domain.FeedItem, 0, len(order))
	for i := len(order) - 1; i >= 0; i-- {
		items = append(items, byID[order[i]])
	}
	return items
}

// feedItemFromEvent extracts the FeedItem embedded in a loop.event payload.
func feedItemFromEvent(payloadJSON string) (domain.FeedItem, bool) {
	var payload struct {
		Event domain.FeedItem `json:"event"`
	}
	if err := json.Unmarshal([]byte(payloadJSON), &payload); err != nil {
		return domain.FeedItem{}, false
	}
	if payload.Event.ID == "" {
		return domain.FeedItem{}, false
	}
	return payload.Event, true
}
