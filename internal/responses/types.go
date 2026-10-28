package responses

import "encoding/json"

// Request is the JSON body for POST /v1/responses.
type Request struct {
	Model             string      `json:"model"`
	Instructions      string      `json:"instructions,omitempty"`
	Input             []InputItem `json:"input"`
	Tools             []Tool      `json:"tools,omitempty"`
	ToolChoice        string      `json:"tool_choice,omitempty"`
	Stream            bool        `json:"stream,omitempty"`
	Store             bool        `json:"store"`
	ParallelToolCalls bool        `json:"parallel_tool_calls,omitempty"`
	MaxOutputTokens   int         `json:"max_output_tokens,omitempty"`
	Temperature       *float64    `json:"temperature,omitempty"`
	Reasoning         *Reasoning  `json:"reasoning,omitempty"`
	Text              *TextConfig `json:"text,omitempty"`
}

// Reasoning is the Responses reasoning configuration object.
type Reasoning struct {
	Effort  string `json:"effort,omitempty"`
	Summary string `json:"summary,omitempty"`
}

// TextConfig is the Responses text configuration object.
type TextConfig struct {
	Verbosity string `json:"verbosity,omitempty"`
}

// Tool is a function tool advertised to the model.
type Tool struct {
	Type        string          `json:"type"`
	Name        string          `json:"name"`
	Description string          `json:"description,omitempty"`
	Parameters  json.RawMessage `json:"parameters,omitempty"`
}

// InputItem is one Responses input item (message, function_call, or output).
type InputItem struct {
	// Raw preserves provider output items, including encrypted reasoning, when
	// the stateless ChatGPT route needs them on the next tool round.
	Raw       json.RawMessage `json:"-"`
	Type      string          `json:"type,omitempty"`
	Role      string          `json:"role,omitempty"`
	Content   []InputContent  `json:"content,omitempty"`
	CallID    string          `json:"call_id,omitempty"`
	Name      string          `json:"name,omitempty"`
	Arguments string          `json:"arguments,omitempty"`
	Output    string          `json:"output,omitempty"`
	ID        string          `json:"id,omitempty"`
}

func (i InputItem) MarshalJSON() ([]byte, error) {
	if len(i.Raw) > 0 {
		return i.Raw, nil
	}
	type plain InputItem
	return json.Marshal(plain(i))
}

// InputContent is one part of a message item.
type InputContent struct {
	Type     string `json:"type"`
	Text     string `json:"text,omitempty"`
	ImageURL string `json:"image_url,omitempty"`
}

// FunctionCall is one completed function_call output item from the model.
type FunctionCall struct {
	ID        string
	CallID    string
	Name      string
	Arguments string
}

// Usage is token accounting from a completed response.
type Usage struct {
	InputTokens     int64
	OutputTokens    int64
	CacheReadTokens int64
	TotalTokens     int64
}

// Event is one parsed stream (or final JSON) event.
type Event struct {
	Output       []json.RawMessage
	Type         string
	ResponseID   string
	Delta        string
	ItemID       string
	Call         *FunctionCall
	Usage        *Usage
	Status       string
	ErrorMessage string
	OutputText   string
}

// FunctionTool builds a function tool descriptor.
func FunctionTool(name, description string, schema json.RawMessage) Tool {
	if len(schema) == 0 {
		schema = json.RawMessage(`{"type":"object","properties":{}}`)
	}
	return Tool{
		Type:        "function",
		Name:        name,
		Description: description,
		Parameters:  schema,
	}
}

// UserText is a user message with plain text.
func UserText(text string) InputItem {
	return InputItem{
		Role:    "user",
		Content: []InputContent{{Type: "input_text", Text: text}},
	}
}

// AssistantText is an assistant message with output text.
func AssistantText(text string) InputItem {
	return InputItem{
		Role:    "assistant",
		Content: []InputContent{{Type: "output_text", Text: text}},
	}
}

// FunctionCallItem is a prior model function_call to replay in input.
func FunctionCallItem(callID, name, arguments string) InputItem {
	return InputItem{
		Type:      "function_call",
		CallID:    callID,
		Name:      name,
		Arguments: arguments,
	}
}

// FunctionOutputItem is the tool result for a prior function_call.
func FunctionOutputItem(callID, output string) InputItem {
	return InputItem{
		Type:   "function_call_output",
		CallID: callID,
		Output: output,
	}
}
