package domain

import (
	"encoding/json"
	"strings"
	"testing"
)

func TestTranscriptXMLNestsToolResult(t *testing.T) {
	got := TranscriptXML([]Message{
		{Role: "user", Content: []ContentBlock{TextBlock("hello")}},
		{Role: "assistant", Content: []ContentBlock{
			ThinkingBlock("look at the tree", "", 0),
			ToolUseBlock("tu-1", "toolu_1", "Bash", json.RawMessage(`{"command":"ls"}`)),
		}},
		{Role: "user", Content: []ContentBlock{ToolResultBlock("tu-1", "file.txt", false)}},
		{Role: "assistant", Content: []ContentBlock{TextBlock("here is the listing")}},
	})
	want := `<transcript>
<message role="user">
<text>hello</text>
</message>
<message role="assistant">
<thinking>look at the tree</thinking>
<tool_use name="Bash">
<input><![CDATA[{"command":"ls"}]]></input>
<result><![CDATA[file.txt]]></result>
</tool_use>
<text>here is the listing</text>
</message>
</transcript>
`
	assertTranscriptXML(t, got, want)
}

func TestTranscriptXMLPreambleTextThenToolThenReply(t *testing.T) {
	// Maestro/coder mirror shape: assistant speaks, then calls a tool, then
	// answers on a later message after the user-carrier result.
	got := TranscriptXML([]Message{
		{Role: "user", Content: []ContentBlock{TextBlock("is auth ok?")}},
		{Role: "assistant", Content: []ContentBlock{
			TextBlock("Let me check."),
			ToolUseBlock("tu-1", "toolu_read1", "Read", json.RawMessage(`{"file_path":"auth.go"}`)),
		}},
		{Role: "user", Content: []ContentBlock{ToolResultBlock("tu-1", "package auth", false)}},
		{Role: "assistant", Content: []ContentBlock{TextBlock("Auth looks fine.")}},
	})
	want := `<transcript>
<message role="user">
<text>is auth ok?</text>
</message>
<message role="assistant">
<text>Let me check.</text>
<tool_use name="Read">
<input><![CDATA[{"file_path":"auth.go"}]]></input>
<result><![CDATA[package auth]]></result>
</tool_use>
<text>Auth looks fine.</text>
</message>
</transcript>
`
	assertTranscriptXML(t, got, want)
}

func TestTranscriptXMLParallelToolUses(t *testing.T) {
	got := TranscriptXML([]Message{
		{Role: "user", Content: []ContentBlock{TextBlock("read both")}},
		{Role: "assistant", Content: []ContentBlock{
			ToolUseBlock("tu-a", "toolu_a", "Read", json.RawMessage(`{"file_path":"a.go"}`)),
			ToolUseBlock("tu-b", "toolu_b", "Read", json.RawMessage(`{"file_path":"b.go"}`)),
		}},
		{Role: "user", Content: []ContentBlock{
			ToolResultBlock("tu-a", "package a", false),
			ToolResultBlock("tu-b", "package b", false),
		}},
		{Role: "assistant", Content: []ContentBlock{TextBlock("both look fine")}},
	})
	want := `<transcript>
<message role="user">
<text>read both</text>
</message>
<message role="assistant">
<tool_use name="Read">
<input><![CDATA[{"file_path":"a.go"}]]></input>
<result><![CDATA[package a]]></result>
</tool_use>
<tool_use name="Read">
<input><![CDATA[{"file_path":"b.go"}]]></input>
<result><![CDATA[package b]]></result>
</tool_use>
<text>both look fine</text>
</message>
</transcript>
`
	assertTranscriptXML(t, got, want)
}

func TestTranscriptXMLMultiRoundTools(t *testing.T) {
	got := TranscriptXML([]Message{
		{Role: "user", Content: []ContentBlock{TextBlock("fix the test")}},
		{Role: "assistant", Content: []ContentBlock{
			ToolUseBlock("tu-1", "toolu_1", "Read", json.RawMessage(`{"file_path":"x_test.go"}`)),
		}},
		{Role: "user", Content: []ContentBlock{ToolResultBlock("tu-1", "func TestX", false)}},
		{Role: "assistant", Content: []ContentBlock{
			ToolUseBlock("tu-2", "toolu_2", "Bash", json.RawMessage(`{"command":"go test"}`)),
		}},
		{Role: "user", Content: []ContentBlock{ToolResultBlock("tu-2", "PASS", false)}},
		{Role: "assistant", Content: []ContentBlock{TextBlock("tests pass")}},
	})
	want := `<transcript>
<message role="user">
<text>fix the test</text>
</message>
<message role="assistant">
<tool_use name="Read">
<input><![CDATA[{"file_path":"x_test.go"}]]></input>
<result><![CDATA[func TestX]]></result>
</tool_use>
<tool_use name="Bash">
<input><![CDATA[{"command":"go test"}]]></input>
<result><![CDATA[PASS]]></result>
</tool_use>
<text>tests pass</text>
</message>
</transcript>
`
	assertTranscriptXML(t, got, want)
}

func TestTranscriptXMLMultiRoundToolsWithInterleavedText(t *testing.T) {
	got := TranscriptXML([]Message{
		{Role: "user", Content: []ContentBlock{TextBlock("fix the test")}},
		{Role: "assistant", Content: []ContentBlock{
			ToolUseBlock("tu-1", "toolu_1", "Read", json.RawMessage(`{"file_path":"x_test.go"}`)),
		}},
		{Role: "user", Content: []ContentBlock{ToolResultBlock("tu-1", "func TestX", false)}},
		{Role: "assistant", Content: []ContentBlock{
			TextBlock("now I'll check the next file"),
			ToolUseBlock("tu-2", "toolu_2", "Bash", json.RawMessage(`{"command":"go test"}`)),
		}},
		{Role: "user", Content: []ContentBlock{ToolResultBlock("tu-2", "PASS", false)}},
		{Role: "assistant", Content: []ContentBlock{TextBlock("tests pass")}},
	})
	want := `<transcript>
<message role="user">
<text>fix the test</text>
</message>
<message role="assistant">
<tool_use name="Read">
<input><![CDATA[{"file_path":"x_test.go"}]]></input>
<result><![CDATA[func TestX]]></result>
</tool_use>
<text>now I'll check the next file</text>
<tool_use name="Bash">
<input><![CDATA[{"command":"go test"}]]></input>
<result><![CDATA[PASS]]></result>
</tool_use>
<text>tests pass</text>
</message>
</transcript>
`
	assertTranscriptXML(t, got, want)
}

func TestTranscriptXMLTwoMultiRoundTurns(t *testing.T) {
	got := TranscriptXML([]Message{
		{Role: "user", Content: []ContentBlock{TextBlock("fix the test")}},
		{Role: "assistant", Content: []ContentBlock{
			ToolUseBlock("tu-1", "toolu_1", "Read", json.RawMessage(`{"file_path":"x_test.go"}`)),
		}},
		{Role: "user", Content: []ContentBlock{ToolResultBlock("tu-1", "func TestX", false)}},
		{Role: "assistant", Content: []ContentBlock{
			TextBlock("now I'll run it"),
			ToolUseBlock("tu-2", "toolu_2", "Bash", json.RawMessage(`{"command":"go test"}`)),
		}},
		{Role: "user", Content: []ContentBlock{ToolResultBlock("tu-2", "PASS", false)}},
		{Role: "assistant", Content: []ContentBlock{TextBlock("tests pass")}},
		{Role: "user", Content: []ContentBlock{TextBlock("also lint it")}},
		{Role: "assistant", Content: []ContentBlock{
			ToolUseBlock("tu-3", "toolu_3", "Read", json.RawMessage(`{"file_path":"x.go"}`)),
		}},
		{Role: "user", Content: []ContentBlock{ToolResultBlock("tu-3", "package x", false)}},
		{Role: "assistant", Content: []ContentBlock{
			TextBlock("linting"),
			ToolUseBlock("tu-4", "toolu_4", "Bash", json.RawMessage(`{"command":"go vet"}`)),
		}},
		{Role: "user", Content: []ContentBlock{ToolResultBlock("tu-4", "ok", false)}},
		{Role: "assistant", Content: []ContentBlock{TextBlock("lint clean")}},
	})
	want := `<transcript>
<message role="user">
<text>fix the test</text>
</message>
<message role="assistant">
<tool_use name="Read">
<input><![CDATA[{"file_path":"x_test.go"}]]></input>
<result><![CDATA[func TestX]]></result>
</tool_use>
<text>now I'll run it</text>
<tool_use name="Bash">
<input><![CDATA[{"command":"go test"}]]></input>
<result><![CDATA[PASS]]></result>
</tool_use>
<text>tests pass</text>
</message>
<message role="user">
<text>also lint it</text>
</message>
<message role="assistant">
<tool_use name="Read">
<input><![CDATA[{"file_path":"x.go"}]]></input>
<result><![CDATA[package x]]></result>
</tool_use>
<text>linting</text>
<tool_use name="Bash">
<input><![CDATA[{"command":"go vet"}]]></input>
<result><![CDATA[ok]]></result>
</tool_use>
<text>lint clean</text>
</message>
</transcript>
`
	assertTranscriptXML(t, got, want)
}

func TestTranscriptXMLSteerOnToolResultBreaksAssistantMerge(t *testing.T) {
	// Claude can attach a mid-turn steer to the next tool_result user row.
	// Fold the result into <tool_use>, keep the commentary as a real user
	// turn, and stop merging assistants across it.
	got := TranscriptXML([]Message{
		{Role: "user", Content: []ContentBlock{TextBlock("fix the test")}},
		{Role: "assistant", Content: []ContentBlock{
			ToolUseBlock("tu-1", "toolu_1", "Read", json.RawMessage(`{"file_path":"x_test.go"}`)),
		}},
		{Role: "user", Content: []ContentBlock{
			ToolResultBlock("tu-1", "func TestX", false),
			TextBlock("actually, skip the test file"),
		}},
		{Role: "assistant", Content: []ContentBlock{
			TextBlock("ok, skipping"),
			ToolUseBlock("tu-2", "toolu_2", "Read", json.RawMessage(`{"file_path":"x.go"}`)),
		}},
		{Role: "user", Content: []ContentBlock{ToolResultBlock("tu-2", "package x", false)}},
		{Role: "assistant", Content: []ContentBlock{TextBlock("looking at x.go instead")}},
	})
	want := `<transcript>
<message role="user">
<text>fix the test</text>
</message>
<message role="assistant">
<tool_use name="Read">
<input><![CDATA[{"file_path":"x_test.go"}]]></input>
<result><![CDATA[func TestX]]></result>
</tool_use>
</message>
<message role="user">
<text>actually, skip the test file</text>
</message>
<message role="assistant">
<text>ok, skipping</text>
<tool_use name="Read">
<input><![CDATA[{"file_path":"x.go"}]]></input>
<result><![CDATA[package x]]></result>
</tool_use>
<text>looking at x.go instead</text>
</message>
</transcript>
`
	assertTranscriptXML(t, got, want)
}

func TestTranscriptXMLToolErrorThenRetry(t *testing.T) {
	got := TranscriptXML([]Message{
		{Role: "user", Content: []ContentBlock{TextBlock("run tests")}},
		{Role: "assistant", Content: []ContentBlock{
			ToolUseBlock("tu-1", "toolu_1", "Bash", json.RawMessage(`{"command":"go test ./..."}`)),
		}},
		{Role: "user", Content: []ContentBlock{ToolResultBlock("tu-1", "exit 1: no go.mod", true)}},
		{Role: "assistant", Content: []ContentBlock{
			ToolUseBlock("tu-2", "toolu_2", "Bash", json.RawMessage(`{"command":"go test ./app"}`)),
		}},
		{Role: "user", Content: []ContentBlock{TruncatedToolResultBlock("tu-2", "ok\n"+strings.Repeat(".", 40), false, true)}},
		{Role: "assistant", Content: []ContentBlock{TextBlock("green")}},
	})
	want := `<transcript>
<message role="user">
<text>run tests</text>
</message>
<message role="assistant">
<tool_use name="Bash">
<input><![CDATA[{"command":"go test ./..."}]]></input>
<result is_error="true"><![CDATA[exit 1: no go.mod]]></result>
</tool_use>
<tool_use name="Bash">
<input><![CDATA[{"command":"go test ./app"}]]></input>
<result truncated="true"><![CDATA[ok
........................................]]></result>
</tool_use>
<text>green</text>
</message>
</transcript>
`
	assertTranscriptXML(t, got, want)
}

func TestTranscriptXMLInterruptedToolUse(t *testing.T) {
	got := TranscriptXML([]Message{
		{Role: "user", Content: []ContentBlock{TextBlock("list files")}},
		{Role: "assistant", Content: []ContentBlock{
			ThinkingBlock("ls the tree", "", 0),
			ToolUseBlock("tu-1", "toolu_1", "Bash", json.RawMessage(`{"command":"ls"}`)),
		}},
	})
	want := `<transcript>
<message role="user">
<text>list files</text>
</message>
<message role="assistant">
<thinking>ls the tree</thinking>
<tool_use name="Bash">
<input><![CDATA[{"command":"ls"}]]></input>
</tool_use>
</message>
</transcript>
`
	assertTranscriptXML(t, got, want)
}

func TestTranscriptXMLImageRootResolvesRelativePath(t *testing.T) {
	msgs := []Message{{Role: "user", Content: []ContentBlock{
		ImageBlock("img-1", "shot.png", "image/png", "workspaces/w/attachments/img-1.png"),
		ImageBlock("img-2", "abs.png", "image/png", "/tmp/abs.png"),
	}}}
	got := TranscriptXML(msgs, TranscriptImageRoot("/data"))
	for _, want := range []string{`path="/data/workspaces/w/attachments/img-1.png"`, `path="/tmp/abs.png"`} {
		if !strings.Contains(got, want) {
			t.Errorf("missing %s in:\n%s", want, got)
		}
	}
}

func TestTranscriptXMLUserImageAndCaption(t *testing.T) {
	got := TranscriptXML([]Message{
		{Role: "user", Content: []ContentBlock{
			TextBlock("what's wrong here"),
			ImageBlock("img-1", "shot.png", "image/png", "workspaces/w/attachments/img-1.png"),
		}},
		{Role: "assistant", Content: []ContentBlock{TextBlock("the button is clipped")}},
	})
	want := `<transcript>
<message role="user">
<text>what's wrong here</text>
<image name="shot.png" mime="image/png" path="workspaces/w/attachments/img-1.png"/>
</message>
<message role="assistant">
<text>the button is clipped</text>
</message>
</transcript>
`
	if strings.Contains(got, `id=`) {
		t.Errorf("TranscriptXML() leaked image id:\n%s", got)
	}
	assertTranscriptXML(t, got, want)
}

func TestTranscriptXMLPrefersWireTextOverDisplay(t *testing.T) {
	got := TranscriptXML([]Message{
		{Role: "user", Content: []ContentBlock{
			TextBlockDisplay("review @internal/x_test.go", "review the flaky suite"),
		}},
		{Role: "assistant", Content: []ContentBlock{TextBlock("I'll start with the suite.")}},
	})
	if strings.Contains(got, "review the flaky suite") {
		t.Errorf("TranscriptXML() used UI display_text, want wire prompt:\n%s", got)
	}
	if !strings.Contains(got, "<text>review @internal/x_test.go</text>") {
		t.Errorf("TranscriptXML() missing wire prompt:\n%s", got)
	}
}

func TestTranscriptXMLCompactThenContinue(t *testing.T) {
	// A real user turn between assistants must keep them as separate messages.
	got := TranscriptXML([]Message{
		{Role: "assistant", Content: []ContentBlock{
			CompactBlock(CompactDone, "auto", "older turns folded", 167219),
		}},
		{Role: "user", Content: []ContentBlock{TextBlock("continue")}},
		{Role: "assistant", Content: []ContentBlock{TextBlock("picking up from the compact")}},
	})
	want := `<transcript>
<message role="assistant">
<compact status="done" trigger="auto" pre_tokens="167219">older turns folded</compact>
</message>
<message role="user">
<text>continue</text>
</message>
<message role="assistant">
<text>picking up from the compact</text>
</message>
</transcript>
`
	assertTranscriptXML(t, got, want)
}

func TestTranscriptXMLForkReplayKeepsToolsDropsThinking(t *testing.T) {
	msgs := []Message{
		{Role: "user", Content: []ContentBlock{TextBlock("hello")}},
		{Role: "assistant", Content: []ContentBlock{
			ThinkingBlock("look at the tree", "", 0),
			ToolUseBlock("tu-1", "toolu_1", "Bash", json.RawMessage(`{"command":"ls"}`)),
		}},
		{Role: "user", Content: []ContentBlock{ToolResultBlock("tu-1", "file.txt", false)}},
		{Role: "assistant", Content: []ContentBlock{
			ThinkingBlock("tell them the listing", "", 0),
			TextBlock("here is the listing"),
		}},
	}
	got := TranscriptXML(msgs, TranscriptOmitThinking())
	want := `<transcript>
<message role="user">
<text>hello</text>
</message>
<message role="assistant">
<tool_use name="Bash">
<input><![CDATA[{"command":"ls"}]]></input>
<result><![CDATA[file.txt]]></result>
</tool_use>
<text>here is the listing</text>
</message>
</transcript>
`
	assertTranscriptXML(t, got, want)
}

func TestMessageTranscriptXMLIncludesEveryBlockType(t *testing.T) {
	m := Message{
		Role:     "assistant",
		ModelID:  "claude-sonnet-4-6",
		Provider: ProviderClaudeCode,
		Content: []ContentBlock{
			ThinkingBlock("look at the tree", "sig", 0),
			ToolUseBlock("tu-1", "toolu_1", "Bash", json.RawMessage(`{"command":"ls"}`)),
			TextBlock("here is the listing"),
			ImageBlock("img-1", "shot.png", "image/png", "workspaces/w/attachments/img-1.png"),
			CompactBlock(CompactDone, "auto", "older turns folded", 1200),
			FallbackBlock("claude-fable-5", "claude-opus-4-8"),
			RawBlock("evidence", json.RawMessage(`{"path":"x.png"}`)),
		},
	}
	got := m.TranscriptXML()
	for _, needle := range []string{
		`id=`,
		`tool_use_id=`,
		`<tool_result`,
		`model=`,
		`provider=`,
	} {
		if strings.Contains(got, needle) {
			t.Errorf("TranscriptXML() leaked pairing id %q\n%s", needle, got)
		}
	}
	for _, want := range []string{
		`<message role="assistant">`,
		"<thinking>look at the tree</thinking>",
		`<tool_use name="Bash">`,
		`<input><![CDATA[{"command":"ls"}]]></input>`,
		"<text>here is the listing</text>",
		`<image name="shot.png" mime="image/png" path="workspaces/w/attachments/img-1.png"/>`,
		`<compact status="done" trigger="auto" pre_tokens="1200">older turns folded</compact>`,
		`<fallback from="claude-fable-5" to="claude-opus-4-8"/>`,
		`<block type="evidence"><![CDATA[{"path":"x.png"}]]></block>`,
		"</message>",
	} {
		if !strings.Contains(got, want) {
			t.Errorf("TranscriptXML() missing %q\n%s", want, got)
		}
	}
}

func TestMessageTranscriptXMLToolResultAndRedactedThinking(t *testing.T) {
	m := Message{
		Role: "user",
		Content: []ContentBlock{
			ThinkingBlock("", "", 80),
			TruncatedToolResultBlock("tu-1", "diff --git a/x", true, true),
		},
	}
	got := m.TranscriptXML()
	if !strings.Contains(got, `<thinking tokens="80"/>`) {
		t.Errorf("TranscriptXML() redacted thinking = %q, want tokens attr", got)
	}
	if !strings.Contains(got, `<result is_error="true" truncated="true"><![CDATA[diff --git a/x]]></result>`) {
		t.Errorf("TranscriptXML() orphan result = %q", got)
	}
	if strings.Contains(got, `tool_use_id=`) || strings.Contains(got, `<tool_result`) {
		t.Errorf("TranscriptXML() leaked result pairing markup: %q", got)
	}
}

func TestMessageTranscriptXMLUsesCDATAForMarkup(t *testing.T) {
	m := Message{
		Role:    "user",
		Content: []ContentBlock{TextBlock(`use <text> & "quotes"`)},
	}
	got := m.TranscriptXML()
	want := `<text><![CDATA[use <text> & "quotes"]]></text>`
	if !strings.Contains(got, want) {
		t.Errorf("TranscriptXML() = %q, want CDATA %q", got, want)
	}
}

func TestMessageTranscriptXMLSplitsCDATATerminator(t *testing.T) {
	m := Message{
		Role:    "user",
		Content: []ContentBlock{TextBlock("a]]>b")},
	}
	got := m.TranscriptXML()
	want := `<text><![CDATA[a]]]]><![CDATA[>b]]></text>`
	if !strings.Contains(got, want) {
		t.Errorf("TranscriptXML() = %q, want split CDATA %q", got, want)
	}
}

func TestMessageTranscriptXMLEmpty(t *testing.T) {
	if got := (Message{Role: "assistant"}).TranscriptXML(); got != "" {
		t.Errorf("TranscriptXML() empty message = %q, want empty", got)
	}
}

func TestTranscriptXMLKeepsOrphanToolResult(t *testing.T) {
	msgs := []Message{
		{Role: "user", Content: []ContentBlock{TextBlock("hello")}},
		{Role: "user", Content: []ContentBlock{ToolResultBlock("tu-1", "huge diff", false)}},
		{Role: "assistant", Content: []ContentBlock{TextBlock("hi back")}},
	}
	got := TranscriptXML(msgs)
	if !strings.HasPrefix(got, "<transcript>\n") || !strings.HasSuffix(got, "</transcript>\n") {
		t.Errorf("TranscriptXML() wrap = %q", got)
	}
	if !strings.Contains(got, `<result><![CDATA[huge diff]]></result>`) {
		t.Errorf("TranscriptXML() should keep orphan tool results, got %q", got)
	}
	user := strings.Index(got, "hello")
	tool := strings.Index(got, "huge diff")
	asst := strings.Index(got, "hi back")
	if user < 0 || tool < 0 || asst < 0 || !(user < tool && tool < asst) {
		t.Errorf("TranscriptXML() order user=%d tool=%d asst=%d\n%s", user, tool, asst, got)
	}
}

func TestTranscriptXMLMaxBytesDropsOldest(t *testing.T) {
	msgs := []Message{
		{Role: "user", Content: []ContentBlock{TextBlock("turn-one-" + strings.Repeat("a", 80))}},
		{Role: "user", Content: []ContentBlock{TextBlock("turn-two-" + strings.Repeat("b", 80))}},
		{Role: "user", Content: []ContentBlock{TextBlock("turn-three")}},
	}
	full := TranscriptXML(msgs)
	got := TranscriptXML(msgs, TranscriptMaxBytes(len(full)/2))
	if got == "" {
		t.Fatal("TranscriptXML(MaxBytes) = empty")
	}
	if strings.Contains(got, "turn-one-") && strings.Contains(got, "turn-two-") && strings.Contains(got, "turn-three") {
		t.Errorf("TranscriptXML(MaxBytes) kept every message under a tight budget: %q", got)
	}
	if !strings.Contains(got, "turn-three") {
		t.Errorf("TranscriptXML(MaxBytes) should keep the newest message, got %q", got)
	}
	if !strings.Contains(got, "<omitted") {
		t.Errorf("TranscriptXML(MaxBytes) should mark dropped prefix, got %q", got)
	}
}

func TestTranscriptXMLOmitThinking(t *testing.T) {
	msgs := []Message{
		{Role: "user", Content: []ContentBlock{TextBlock("hello")}},
		{Role: "assistant", Content: []ContentBlock{
			ThinkingBlock("look at the tree", "", 0),
			TextBlock("here is the listing"),
		}},
		{Role: "assistant", Content: []ContentBlock{
			ThinkingBlock("only thoughts", "", 0),
		}},
	}
	got := TranscriptXML(msgs, TranscriptOmitThinking())
	if strings.Contains(got, "<thinking") || strings.Contains(got, "look at the tree") || strings.Contains(got, "only thoughts") {
		t.Errorf("TranscriptXML(OmitThinking) kept thinking: %q", got)
	}
	if !strings.Contains(got, "<text>hello</text>") || !strings.Contains(got, "<text>here is the listing</text>") {
		t.Errorf("TranscriptXML(OmitThinking) dropped visible text: %q", got)
	}
	if strings.Count(got, "<message") != 2 {
		t.Errorf("TranscriptXML(OmitThinking) should drop thinking-only messages, got %q", got)
	}

	kept := TranscriptXML(msgs)
	if !strings.Contains(kept, "<thinking>look at the tree</thinking>") {
		t.Errorf("TranscriptXML() default should keep thinking, got %q", kept)
	}
}

func TestTranscriptXMLClipToolsCapsToolBodiesNotProse(t *testing.T) {
	huge := strings.Repeat("x", 20<<10)
	prose := "keep-this-user-turn-" + strings.Repeat("y", 12<<10)
	msgs := []Message{
		{Role: "user", Content: []ContentBlock{TextBlock(prose)}},
		{Role: "assistant", Content: []ContentBlock{
			ToolUseBlock("t1", "", "Read", []byte(`{"path":"big.go"}`)),
		}},
		{Role: "user", Content: []ContentBlock{ToolResultBlock("t1", huge, false)}},
	}
	got := TranscriptXML(msgs, TranscriptClipTools())
	if !strings.Contains(got, prose) {
		t.Fatalf("TranscriptXML(ClipTools) dropped user prose")
	}
	if strings.Contains(got, huge) {
		t.Fatalf("TranscriptXML(ClipTools) left the full tool result intact")
	}
	if !strings.Contains(got, "…(truncated)…") {
		t.Fatalf("TranscriptXML(ClipTools) missing truncation marker: %q", got)
	}
	if strings.Contains(got, "<omitted") {
		t.Fatalf("TranscriptXML(ClipTools) should not drop messages: %q", got)
	}
}

func TestTranscriptXMLMaxBytesZeroMatchesUnlimited(t *testing.T) {
	msgs := []Message{
		{Role: "user", Content: []ContentBlock{TextBlock("hello")}},
	}
	if got, want := TranscriptXML(msgs, TranscriptMaxBytes(0)), TranscriptXML(msgs); got != want {
		t.Errorf("TranscriptXML(MaxBytes(0)) = %q, want %q", got, want)
	}
}

func assertTranscriptXML(t *testing.T, got, want string) {
	t.Helper()
	if got != want {
		t.Errorf("TranscriptXML() =\n%s\nwant\n%s", got, want)
	}
}
