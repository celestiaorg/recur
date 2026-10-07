package app

import "recur/internal/domain"

// BriefingService is the Wails-bound surface for the Inbox and Ask Recur.
// Copy and routes come from the engine projection so the UI and coordinator
// cannot disagree.
type BriefingService struct{ base }

// AppChatSelection is the last Ask Recur model picker. Mode is always ask.
type AppChatSelection struct {
	Model string            `json:"model"`
	Flags map[string]string `json:"flags,omitempty"`
}

func (s *BriefingService) GetBriefing() domain.BriefingSnapshot {
	return s.eng.BriefingSnapshot()
}

func (s *BriefingService) MarkWorkspaceViewed(workspaceID string, seenThroughActivityAt int64) {
	s.eng.MarkWorkspaceViewed(workspaceID, seenThroughActivityAt)
}

// ListAppChats returns the open Ask Recur tabs (primary first).
func (s *BriefingService) ListAppChats() []domain.WorkspaceChatTab {
	return s.eng.ListAppChats()
}

// NewAppChat opens a fresh Ask Recur tab and returns it.
func (s *BriefingService) NewAppChat() domain.WorkspaceChatTab {
	return s.eng.NewAppChat()
}

// CloseAppChat closes a secondary tab (the primary APP tab is not closable).
// Returns the remaining open tabs.
func (s *BriefingService) CloseAppChat(code string) []domain.WorkspaceChatTab {
	return s.eng.CloseAppChat(code)
}

func (s *BriefingService) AppChatItems(code string) []domain.ChatItem {
	return s.eng.AppChatItems(code)
}

func (s *BriefingService) AppChatSelection(code string) AppChatSelection {
	model, flags := s.eng.AppChatSelection(code)
	return AppChatSelection{Model: model, Flags: flags}
}

func (s *BriefingService) RememberAppChatSelection(code, model string, flags map[string]string) {
	s.eng.RememberAppChatSelection(code, model, flags)
}

func (s *BriefingService) SendAppMessage(code, text, model string, flags map[string]string) string {
	return s.eng.SendAppMessage(code, text, model, flags)
}

func (s *BriefingService) CancelAppMessage(code string) {
	s.eng.CancelAppMessage(code)
}

func (s *BriefingService) SetAppChatFocus(visible bool, code string) {
	s.eng.SetAppChatFocus(visible, code)
}

func (s *BriefingService) MarkAppChatSeen(code string) {
	s.eng.MarkAppChatSeen(code)
}
