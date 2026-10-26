package responses

import (
	"encoding/json"
	"fmt"
)

// ChatGPT's Codex route is streaming and stateless, with the same flat function
// tools used by OpenCode. Encrypted reasoning is preserved between tool rounds.
func chatGPTBody(raw []byte) ([]byte, error) {
	var body map[string]any
	if err := json.Unmarshal(raw, &body); err != nil {
		return nil, fmt.Errorf("ChatGPT request: %w", err)
	}
	body["stream"], body["store"] = true, false
	body["include"] = []string{"reasoning.encrypted_content"}
	for _, key := range []string{
		"background", "conversation", "max_output_tokens", "max_tool_calls", "metadata",
		"moderation", "multi_agent", "prompt", "prompt_cache_retention", "safety_identifier",
		"temperature", "top_logprobs", "top_p", "truncation", "user", "previous_response_id",
	} {
		delete(body, key)
	}
	if tools, ok := body["tools"].([]any); ok && len(tools) > 0 {
		for _, raw := range tools {
			if tool, ok := raw.(map[string]any); ok && tool["type"] == "function" {
				tool["strict"] = false
			}
		}
	} else {
		delete(body, "tool_choice")
	}
	return json.Marshal(body)
}
