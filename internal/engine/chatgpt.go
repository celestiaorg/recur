package engine

import (
	"recur/internal/chatgpt"
	"recur/internal/store"
)

// ChatGPTClient shares OAuth state and rotating-token refresh between Settings
// and every native-harness turn. Construction itself makes no network requests.
func (e *Engine) ChatGPTClient() (*chatgpt.Client, error) {
	e.chatGPTMu.Lock()
	defer e.chatGPTMu.Unlock()
	if e.chatGPT == nil {
		dir, err := store.DataDir()
		if err != nil {
			return nil, err
		}
		e.chatGPT = chatgpt.New(dir)
	}
	return e.chatGPT, nil
}

func (e *Engine) CloseChatGPT() {
	e.chatGPTMu.Lock()
	c := e.chatGPT
	e.chatGPTMu.Unlock()
	if c != nil {
		c.Close()
	}
}
