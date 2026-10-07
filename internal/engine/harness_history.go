package engine

import (
	"encoding/json"
	"strings"

	"recur/internal/domain"
	"recur/internal/responses"
)

const maxHistoryBytes = 8 << 20

func harnessInstructions(spec AgentSpec) string {
	return joinSystemParts(recurCodingPrompt, spec.SystemPrompt, spec.AppendSystemPrompt, spec.TurnAppendPrompt)
}

func joinSystemParts(parts ...string) string {
	var b strings.Builder
	for _, p := range parts {
		p = strings.TrimSpace(p)
		if p == "" {
			continue
		}
		if b.Len() > 0 {
			b.WriteString("\n\n")
		}
		b.WriteString(p)
	}
	return b.String()
}

func harnessHistory(msgs []domain.Message, spec AgentSpec) []responses.InputItem {
	start := 0
	if spec.HistoryAfterID != "" {
		for i, m := range msgs {
			if m.ID == spec.HistoryAfterID {
				start = i + 1
				break
			}
		}
	}
	var out []responses.InputItem
	callIDs := map[string]string{} // Recur tool-use id → provider call_id
	for _, m := range msgs[start:] {
		if skipHarnessCurrentUser(m, spec) {
			continue
		}
		if m.StreamStatus == domain.StreamPending || m.StreamStatus == domain.StreamStreaming {
			continue
		}
		out = append(out, itemsFromMessage(m, callIDs)...)
	}
	out = append(out, currentUserItem(spec.Prompt, spec.Images))
	return capHistory(out)
}

// skipHarnessCurrentUser drops the in-progress / retried user row so spec.Prompt
// is not appended twice. Hidden continuations (switch_mode remount, restart)
// keep that row: their Prompt is only a continue note, not the original request.
func skipHarnessCurrentUser(m domain.Message, spec AgentSpec) bool {
	if spec.UserMessageID == "" || m.ID != spec.UserMessageID {
		return false
	}
	if spec.SkipRecordUser && hiddenContinuation(spec) {
		return false
	}
	return true
}

func hiddenContinuation(spec AgentSpec) bool {
	return isModeSwitchContinueDisplay(spec.DisplayText) || spec.DisplayText == chatResumeDisplay
}

func itemsFromMessage(m domain.Message, callIDs map[string]string) []responses.InputItem {
	var out []responses.InputItem
	var text strings.Builder
	role := m.Role
	if role == "" {
		role = "user"
	}
	flushText := func() {
		if text.Len() == 0 {
			return
		}
		if role == "assistant" {
			out = append(out, responses.AssistantText(text.String()))
		} else {
			out = append(out, responses.UserText(text.String()))
		}
		text.Reset()
	}
	for _, b := range m.Content {
		switch b.Type {
		case domain.BlockText:
			p, ok := b.AsText()
			if !ok || strings.TrimSpace(p.Text) == "" {
				continue
			}
			if text.Len() > 0 {
				text.WriteByte('\n')
			}
			text.WriteString(p.Text)
		case domain.BlockThinking:
			// Omit chain-of-thought from replay.
		case domain.BlockImage:
			flushText()
			p, ok := b.AsImage()
			if !ok {
				continue
			}
			img, ok := userImageFromStored(p)
			if !ok {
				continue
			}
			mime := img.Mime
			if mime == "" {
				mime = "image/png"
			}
			out = append(out, responses.InputItem{
				Role: "user",
				Content: []responses.InputContent{{
					Type:     "input_image",
					ImageURL: "data:" + mime + ";base64," + img.Data,
				}},
			})
		case domain.BlockToolUse:
			flushText()
			p, ok := b.AsToolUse()
			if !ok {
				continue
			}
			callID := p.ProviderToolUseID
			if callID == "" {
				callID = p.ID
			}
			callIDs[p.ID] = callID
			args := ""
			if len(p.Input) > 0 {
				args = string(p.Input)
			}
			out = append(out, responses.FunctionCallItem(callID, p.Name, args))
		case domain.BlockToolResult:
			flushText()
			p, ok := b.AsToolResult()
			if !ok {
				continue
			}
			callID := callIDs[p.ToolUseID]
			if callID == "" {
				callID = p.ToolUseID
			}
			out = append(out, responses.FunctionOutputItem(callID, p.Result))
		default:
			if p, ok := b.AsText(); ok && p.Text != "" {
				if text.Len() > 0 {
					text.WriteByte('\n')
				}
				text.WriteString(p.Text)
			}
		}
	}
	flushText()
	return out
}

func currentUserItem(prompt string, images []UserImage) responses.InputItem {
	item := responses.InputItem{Role: "user"}
	if strings.TrimSpace(prompt) != "" {
		item.Content = append(item.Content, responses.InputContent{Type: "input_text", Text: prompt})
	}
	for _, img := range images {
		if img.Data == "" {
			continue
		}
		mime := img.Mime
		if mime == "" {
			mime = "image/png"
		}
		item.Content = append(item.Content, responses.InputContent{
			Type:     "input_image",
			ImageURL: "data:" + mime + ";base64," + img.Data,
		})
	}
	if len(item.Content) == 0 {
		item.Content = []responses.InputContent{{Type: "input_text", Text: prompt}}
	}
	return item
}

func capHistory(in []responses.InputItem) []responses.InputItem {
	raw, err := json.Marshal(in)
	if err != nil || len(raw) <= maxHistoryBytes {
		return in
	}
	if len(in) <= 1 {
		return in
	}
	keep := in[len(in)-1]
	head := in[:len(in)-1]
	for len(head) > 0 && len(raw) > maxHistoryBytes {
		head = head[nextHistoryGroup(head):]
		cand := append(append([]responses.InputItem{}, head...), keep)
		raw, err = json.Marshal(cand)
		if err != nil {
			return cand
		}
		if len(raw) <= maxHistoryBytes {
			return cand
		}
	}
	return []responses.InputItem{keep}
}

// nextHistoryGroup is how many leading items must drop together so we never
// split a function_call from its output, a parallel call batch, or an
// assistant turn from its calls.
func nextHistoryGroup(in []responses.InputItem) int {
	if len(in) == 0 {
		return 0
	}
	if in[0].Type == "function_call_output" {
		return 1
	}
	if in[0].Type != "function_call" {
		i := 1
		for i < len(in) && (in[i].Type == "function_call" || in[i].Type == "function_call_output") {
			i++
		}
		return i
	}
	need := map[string]struct{}{in[0].CallID: {}}
	i := 1
	for i < len(in) {
		it := in[i]
		switch it.Type {
		case "function_call":
			need[it.CallID] = struct{}{}
			i++
		case "function_call_output":
			if _, ok := need[it.CallID]; !ok {
				return i
			}
			delete(need, it.CallID)
			i++
			if len(need) == 0 {
				return i
			}
		default:
			return i
		}
	}
	return i
}
