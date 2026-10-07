package domain

import (
	"path/filepath"
	"strconv"
	"strings"
	"unicode/utf8"
)

// TranscriptOption configures TranscriptXML.
type TranscriptOption interface {
	applyTranscript(*transcriptOptions)
}

type transcriptOptions struct {
	omitThinking bool
	clipTools    bool
	maxBytes     int
	imageRoot    string
}

type omitThinkingOption bool

func (o omitThinkingOption) applyTranscript(opts *transcriptOptions) {
	opts.omitThinking = bool(o)
}

type maxBytesOption int

func (o maxBytesOption) applyTranscript(opts *transcriptOptions) {
	opts.maxBytes = int(o)
}

type clipToolsOption bool

func (o clipToolsOption) applyTranscript(opts *transcriptOptions) {
	opts.clipTools = bool(o)
}

// TranscriptOmitThinking drops <thinking> blocks. Prior-model chain-of-thought
// can confuse a different model on fork or provider switch.
func TranscriptOmitThinking() TranscriptOption {
	return omitThinkingOption(true)
}

// TranscriptMaxBytes bounds the document. Oldest complete <message> elements
// are dropped first; remaining block bodies are capped so one huge tool
// result cannot blow the budget. n <= 0 means unlimited.
func TranscriptMaxBytes(n int) TranscriptOption {
	return maxBytesOption(n)
}

// TranscriptClipTools caps tool-use input and tool-result bodies at 8 KiB so
// one huge Read/Bash payload cannot dominate a fork replay. User and
// assistant prose is left intact. Pair with TranscriptMaxBytes to also
// drop oldest <message> elements.
func TranscriptClipTools() TranscriptOption {
	return clipToolsOption(true)
}

type imageRootOption string

func (o imageRootOption) applyTranscript(opts *transcriptOptions) {
	opts.imageRoot = string(o)
}

// TranscriptImageRoot resolves relative image paths against root so a replay
// reader can open the attachment with its own file tools.
func TranscriptImageRoot(root string) TranscriptOption {
	return imageRootOption(root)
}

func applyTranscriptOptions(opts []TranscriptOption) transcriptOptions {
	var o transcriptOptions
	for _, opt := range opts {
		opt.applyTranscript(&o)
	}
	return o
}

// TranscriptXML renders one message as a structured <message> element for
// provider-neutral replay (session fork, mid-chat provider switch). Tool
// results that live on the same message are nested inside the matching
// <tool_use>; ids are pairing-only and are not emitted. AsText only decodes
// type=text, so a text-only flatten would drop tools, thinking, and images.
func (m Message) TranscriptXML(opts ...TranscriptOption) string {
	opt := applyTranscriptOptions(opts)
	results, uses := indexToolIO([]Message{m})
	return m.transcriptXML(transcriptBodyCap(opt), results, uses, opt)
}

// TranscriptXML renders msgs as a <transcript> document, oldest first. Tool
// results on later user-carrier rows are folded into the assistant <tool_use>
// they answer. Consecutive assistant turns (the usual tool-loop shape) merge
// into one <message> so the next model sees a single run. A user row that
// mixes a tool result with real text (Claude mid-turn steer on the next
// tool_result) folds the result and emits the text as a user <message>,
// which also ends the assistant merge. Empty when nothing renders.
func TranscriptXML(msgs []Message, opts ...TranscriptOption) string {
	return transcriptXML(msgs, opts...)
}

const transcriptBlockCap = 8 << 10

func transcriptBodyCap(opt transcriptOptions) int {
	if opt.maxBytes > 0 || opt.clipTools {
		return transcriptBlockCap
	}
	return 0
}

func transcriptXML(msgs []Message, opts ...TranscriptOption) string {
	if len(msgs) == 0 {
		return ""
	}
	opt := applyTranscriptOptions(opts)
	capBody := transcriptBodyCap(opt)
	maxBytes := opt.maxBytes
	results, uses := indexToolIO(msgs)
	raw := make([]transcriptPart, 0, len(msgs))
	for _, m := range msgs {
		body := m.transcriptBody(capBody, results, uses, opt)
		if body == "" {
			continue
		}
		raw = append(raw, transcriptPart{role: m.Role, body: body})
	}
	parts := make([]string, 0, len(raw))
	for _, p := range foldAssistantTurns(raw) {
		parts = append(parts, wrapTranscriptMessage(p.role, p.body))
	}
	if len(parts) == 0 {
		return ""
	}
	const prefix = "<transcript>\n"
	const suffix = "</transcript>\n"
	if maxBytes <= 0 {
		return prefix + strings.Join(parts, "") + suffix
	}

	used := len(prefix) + len(suffix)
	start := len(parts)
	for start > 0 {
		next := len(parts[start-1])
		note := 0
		if start > 1 {
			note = omittedTagLen(start - 1)
		}
		if used+next+note > maxBytes && start < len(parts) {
			break
		}
		start--
		used += next
	}
	var b strings.Builder
	b.Grow(used + 64)
	b.WriteString(prefix)
	if start > 0 {
		writeOmitted(&b, start)
	}
	for _, p := range parts[start:] {
		b.WriteString(p)
	}
	b.WriteString(suffix)
	return b.String()
}

func indexToolIO(msgs []Message) (results map[string]ToolResultPayload, uses map[string]bool) {
	results = map[string]ToolResultPayload{}
	uses = map[string]bool{}
	for _, m := range msgs {
		for _, c := range m.Content {
			if p, ok := c.AsToolUse(); ok && p.ID != "" {
				uses[p.ID] = true
			}
			if p, ok := c.AsToolResult(); ok && p.ToolUseID != "" {
				results[p.ToolUseID] = p
			}
		}
	}
	return results, uses
}

func omittedTagLen(n int) int {
	return len(`<omitted n=""/>`) + len(strconv.Itoa(n)) + 1
}

func writeOmitted(b *strings.Builder, n int) {
	b.WriteString("<omitted")
	writeAttr(b, "n", strconv.Itoa(n))
	b.WriteString("/>\n")
}

type transcriptPart struct {
	role string
	body string
}

// foldAssistantTurns merges adjacent assistant parts. After tool-result
// carriers are omitted, a multi-round tool loop is consecutive assistant
// bodies that read better as one message. A leftover user body (steer text
// that shared a row with a folded result) stays and splits the merge.
func foldAssistantTurns(parts []transcriptPart) []transcriptPart {
	if len(parts) == 0 {
		return nil
	}
	out := make([]transcriptPart, 0, len(parts))
	for _, p := range parts {
		if n := len(out); n > 0 && out[n-1].role == "assistant" && p.role == "assistant" {
			out[n-1].body += p.body
			continue
		}
		out = append(out, p)
	}
	return out
}

func wrapTranscriptMessage(role, body string) string {
	var b strings.Builder
	b.WriteString("<message")
	writeAttr(&b, "role", role)
	b.WriteString(">\n")
	b.WriteString(body)
	b.WriteString("</message>\n")
	return b.String()
}

func (m Message) transcriptXML(blockCap int, results map[string]ToolResultPayload, uses map[string]bool, opt transcriptOptions) string {
	body := m.transcriptBody(blockCap, results, uses, opt)
	if body == "" {
		return ""
	}
	return wrapTranscriptMessage(m.Role, body)
}

func (m Message) transcriptBody(blockCap int, results map[string]ToolResultPayload, uses map[string]bool, opt transcriptOptions) string {
	if len(m.Content) == 0 {
		return ""
	}
	var body strings.Builder
	for _, c := range m.Content {
		c.writeTranscript(&body, blockCap, results, uses, opt)
	}
	return body.String()
}

func (c ContentBlock) writeTranscript(b *strings.Builder, blockCap int, results map[string]ToolResultPayload, uses map[string]bool, opt transcriptOptions) {
	proseCap := blockCap
	if opt.clipTools && opt.maxBytes <= 0 {
		proseCap = 0
	}
	switch c.Type {
	case BlockText:
		p, ok := c.AsText()
		if !ok {
			c.writeUnknown(b, blockCap)
			return
		}
		text := p.Text
		if strings.TrimSpace(text) == "" {
			text = p.DisplayText
		}
		if strings.TrimSpace(text) == "" {
			return
		}
		writeElem(b, "text", clipTranscript(text, proseCap), nil, false)

	case BlockThinking:
		if opt.omitThinking {
			return
		}
		p, ok := c.AsThinking()
		if !ok {
			c.writeUnknown(b, blockCap)
			return
		}
		var attrs [][2]string
		if p.EstimatedTokens > 0 && strings.TrimSpace(p.Thinking) == "" {
			attrs = append(attrs, [2]string{"tokens", strconv.FormatInt(p.EstimatedTokens, 10)})
		}
		if strings.TrimSpace(p.Thinking) == "" && len(attrs) == 0 {
			return
		}
		writeElem(b, "thinking", clipTranscript(p.Thinking, proseCap), attrs, false)

	case BlockToolUse:
		p, ok := c.AsToolUse()
		if !ok {
			c.writeUnknown(b, blockCap)
			return
		}
		var res *ToolResultPayload
		if r, ok := results[p.ID]; ok {
			res = &r
		}
		writeToolUse(b, p.Name, string(p.Input), res, blockCap)

	case BlockToolResult:
		p, ok := c.AsToolResult()
		if !ok {
			c.writeUnknown(b, blockCap)
			return
		}
		// Folded into the matching <tool_use> when that call is in this transcript.
		if uses[p.ToolUseID] {
			return
		}
		writeToolUse(b, "", "", &p, blockCap)

	case BlockImage:
		p, ok := c.AsImage()
		if !ok {
			c.writeUnknown(b, blockCap)
			return
		}
		b.WriteString("<image")
		writeAttr(b, "name", p.Name)
		writeAttr(b, "mime", p.Mime)
		path := p.Path
		if opt.imageRoot != "" && path != "" && !filepath.IsAbs(path) {
			path = filepath.Join(opt.imageRoot, path)
		}
		writeAttr(b, "path", path)
		b.WriteString("/>\n")

	case BlockCompact:
		p, ok := c.AsCompact()
		if !ok {
			c.writeUnknown(b, blockCap)
			return
		}
		attrs := [][2]string{{"status", p.Status}}
		if p.Trigger != "" {
			attrs = append(attrs, [2]string{"trigger", p.Trigger})
		}
		if p.PreTokens > 0 {
			attrs = append(attrs, [2]string{"pre_tokens", strconv.FormatInt(p.PreTokens, 10)})
		}
		writeElem(b, "compact", clipTranscript(p.Summary, proseCap), attrs, false)

	case BlockFallback:
		p, ok := c.AsFallback()
		if !ok {
			c.writeUnknown(b, blockCap)
			return
		}
		attrs := [][2]string{{"from", p.From}, {"to", p.To}}
		writeElem(b, "fallback", "", attrs, false)

	default:
		c.writeUnknown(b, blockCap)
	}
}

func writeToolUse(b *strings.Builder, name, input string, result *ToolResultPayload, blockCap int) {
	b.WriteString("<tool_use")
	writeAttr(b, "name", name)
	b.WriteString(">\n")
	if strings.TrimSpace(input) != "" && input != "{}" {
		writeElem(b, "input", clipTranscript(input, blockCap), nil, true)
	}
	if result != nil {
		var attrs [][2]string
		if result.IsError {
			attrs = append(attrs, [2]string{"is_error", "true"})
		}
		if result.Truncated {
			attrs = append(attrs, [2]string{"truncated", "true"})
		}
		writeElem(b, "result", clipTranscript(result.Result, blockCap), attrs, true)
	}
	b.WriteString("</tool_use>\n")
}

func (c ContentBlock) writeUnknown(b *strings.Builder, blockCap int) {
	if c.Type == "" {
		return
	}
	writeElem(b, "block", clipTranscript(string(c.ContentJSON), blockCap), [][2]string{{"type", c.Type}}, true)
}

// writeElem writes <tag attrs>body</tag>. Opaque payloads (tool input/result)
// always use CDATA; other bodies use CDATA only when they contain markup.
func writeElem(b *strings.Builder, tag, body string, attrs [][2]string, cdata bool) {
	b.WriteByte('<')
	b.WriteString(tag)
	for _, a := range attrs {
		writeAttr(b, a[0], a[1])
	}
	if body == "" {
		b.WriteString("/>\n")
		return
	}
	if cdata || needsCDATA(body) {
		b.WriteByte('>')
		writeCDATA(b, body)
		b.WriteString("</")
		b.WriteString(tag)
		b.WriteString(">\n")
		return
	}
	b.WriteByte('>')
	b.WriteString(body)
	b.WriteString("</")
	b.WriteString(tag)
	b.WriteString(">\n")
}

func writeAttr(b *strings.Builder, key, val string) {
	if val == "" {
		return
	}
	b.WriteByte(' ')
	b.WriteString(key)
	b.WriteString(`="`)
	b.WriteString(xmlAttrEscape(val))
	b.WriteByte('"')
}

func needsCDATA(s string) bool {
	return strings.ContainsAny(s, "<>&") || strings.Contains(s, "]]>")
}

// writeCDATA wraps s, splitting on ]]> so the terminator cannot appear in a
// single section (the only CDATA escape).
func writeCDATA(b *strings.Builder, s string) {
	for {
		i := strings.Index(s, "]]>")
		if i < 0 {
			b.WriteString("<![CDATA[")
			b.WriteString(s)
			b.WriteString("]]>")
			return
		}
		b.WriteString("<![CDATA[")
		b.WriteString(s[:i+2])
		b.WriteString("]]>")
		s = s[i+2:]
	}
}

func xmlAttrEscape(s string) string {
	if !strings.ContainsAny(s, `&<"`) {
		return s
	}
	var b strings.Builder
	b.Grow(len(s) + 8)
	for _, r := range s {
		switch r {
		case '&':
			b.WriteString("&amp;")
		case '<':
			b.WriteString("&lt;")
		case '"':
			b.WriteString("&quot;")
		default:
			b.WriteRune(r)
		}
	}
	return b.String()
}

func clipTranscript(s string, cap int) string {
	if cap <= 0 || len(s) <= cap {
		return s
	}
	s = s[:cap]
	for !utf8.ValidString(s) && len(s) > 0 {
		s = s[:len(s)-1]
	}
	return s + "\n…(truncated)…"
}
