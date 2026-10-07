package engine

import (
	"bytes"
	"encoding/base64"
	"encoding/json"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"recur/internal/domain"
)

func TestPrepareChatAttachmentsAndClaudeContent(t *testing.T) {
	root := t.TempDir()
	t.Setenv("RECUR_DATA_DIR", root)

	png := base64.StdEncoding.EncodeToString([]byte{
		0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, // PNG magic
		0x00, 0x00, 0x00, 0x00,
	})
	text := "look at this\n" + AttachmentMarker("img1") + "\nespecially the header"
	gotText, imgs, err := prepareChatAttachments("WS1", text, []domain.ChatAttachment{{
		ID: "img1", Name: "shot.png", Mime: "image/png", Data: png,
	}})
	if err != nil {
		t.Fatal(err)
	}
	if gotText != text {
		t.Fatalf("text = %q, want unchanged with marker", gotText)
	}
	if len(imgs) != 1 || imgs[0].ID != "img1" || imgs[0].Mime != "image/png" {
		t.Fatalf("imgs = %+v", imgs)
	}
	full := filepath.Join(root, imgs[0].Path)
	if _, err := os.Stat(full); err != nil {
		t.Fatalf("saved file: %v", err)
	}

	content := claudeUserContent(gotText, imgs)
	raw, err := json.Marshal(content)
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Contains(raw, []byte(`"type":"image"`)) {
		t.Fatalf("expected image block, got %s", raw)
	}
	if !bytes.Contains(raw, []byte(`look at this`)) || !bytes.Contains(raw, []byte(`especially the header`)) {
		t.Fatalf("expected interleaved text, got %s", raw)
	}
	// Body markers become image blocks; ids still appear in the instruction
	// prefix so submit_plan.attachments is actionable on the Claude path.
	if !bytes.Contains(raw, []byte(`⟦attachment:img1⟧`)) {
		t.Fatalf("expected attachment id in Claude instruction, got %s", raw)
	}
	if !bytes.Contains(raw, []byte(`submit_plan`)) {
		t.Fatalf("expected submit_plan hint in Claude instruction, got %s", raw)
	}
}

func TestStripAttachmentMarkers(t *testing.T) {
	in := "look at this\n" + AttachmentMarker("img1") + "\nplease"
	if got := stripAttachmentMarkers(in); got != "look at this\n\nplease" {
		t.Fatalf("got %q", got)
	}
	if got := stripAttachmentMarkers(AttachmentMarker("img1")); got != "" {
		t.Fatalf("marker-only = %q", got)
	}
}

func TestEnsureAttachmentMarkersAndWorkspaceImages(t *testing.T) {
	root := t.TempDir()
	t.Setenv("RECUR_DATA_DIR", root)
	png := base64.StdEncoding.EncodeToString([]byte{
		0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a,
		0x00, 0x00, 0x00, 0x00,
	})
	_, imgs, err := prepareChatAttachments("WS1", "look", []domain.ChatAttachment{{
		ID: "img1", Name: "shot.png", Mime: "image/png", Data: png,
	}})
	if err != nil {
		t.Fatal(err)
	}
	got := ensureAttachmentMarkers("look", imgs)
	if !strings.Contains(got, AttachmentMarker("img1")) {
		t.Fatalf("prompt = %q, want marker", got)
	}
	loaded := workspaceUserImages("WS1")
	if len(loaded) != 1 || loaded[0].ID != "img1" || loaded[0].Data == "" {
		t.Fatalf("workspaceUserImages = %+v", loaded)
	}
}

func TestApplyGoalImagesSetsPlannerSpec(t *testing.T) {
	eng, _, _, _ := newTestEngine(t, &captureRunner{
		linesFor: func(call int) [][]byte { return maestroFixture("plan-img", "OK.") },
	})
	projID := addProject(t, eng.Store, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "scratch", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	png := base64.StdEncoding.EncodeToString([]byte{
		0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a,
		0x00, 0x00, 0x00, 0x00,
	})
	_, imgs, err := prepareChatAttachments(ws.ID, "look at this", []domain.ChatAttachment{{
		ID: "img1", Name: "shot.png", Mime: "image/png", Data: png,
	}})
	if err != nil {
		t.Fatal(err)
	}
	eng.stampLoopGoalImages(ws.ID, maestroAgentCode, imgs)
	l := &Loop{ID: ws.ID, ChatCode: maestroAgentCode, Goal: "look at this", eng: eng}
	spec := AgentSpec{Prompt: l.Goal}
	l.applyGoalImages(&spec)
	if len(spec.Images) != 1 || spec.Images[0].ID != "img1" {
		t.Fatalf("Images = %+v", spec.Images)
	}
	if !strings.Contains(spec.Prompt, AttachmentMarker("img1")) {
		t.Fatalf("Prompt = %q, want marker so the model can bind the image", spec.Prompt)
	}
}

func TestUserContentBlocksOrder(t *testing.T) {
	imgs := []UserImage{{
		ID: "a", Name: "a.png", Mime: "image/png", Path: "workspaces/W/attachments/a.png",
	}}
	blocks := userContentBlocks("before "+AttachmentMarker("a")+" after", "", imgs)
	if len(blocks) != 3 {
		t.Fatalf("len=%d, want 3", len(blocks))
	}
	if p, ok := blocks[0].AsText(); !ok || !strings.Contains(p.Text, "before") {
		t.Fatalf("block0 = %+v", blocks[0])
	}
	if p, ok := blocks[1].AsImage(); !ok || p.ID != "a" {
		t.Fatalf("block1 = %+v", blocks[1])
	}
	if p, ok := blocks[2].AsText(); !ok || !strings.Contains(p.Text, "after") {
		t.Fatalf("block2 = %+v", blocks[2])
	}
}

func TestWrapAttachedImagesForCursor(t *testing.T) {
	m1 := AttachmentMarker("img_a")
	m2 := AttachmentMarker("img_b")
	in := "before " + m1 + " mid " + m2 + " after"
	imgs := []UserImage{
		{ID: "img_b", Mime: "image/png", Data: "BB"},
		{ID: "img_a", Mime: "image/png", Data: "AA"},
	}
	got := wrapAttachedImagesForCursor(in, imgs)
	if !strings.Contains(got, "<system_instruction>") {
		t.Fatalf("missing instruction: %q", got)
	}
	if !strings.Contains(got, m1+" is image 1 (first attached image)") {
		t.Fatalf("want m1 first (marker order): %q", got)
	}
	if !strings.Contains(got, m2+" is image 2 (second attached image)") {
		t.Fatalf("want m2 second: %q", got)
	}
	// Body keeps markers for in-text location.
	if !strings.Contains(got, in) {
		t.Fatalf("body/markers lost: %q", got)
	}
	if wrapAttachedImagesForCursor(in, nil) != in {
		t.Fatal("no-op without images")
	}
}

func TestFlattenUserMessageRestoresMarkers(t *testing.T) {
	it, ok := flattenUserMessage(domain.Message{
		Role: "user",
		Content: []domain.ContentBlock{
			domain.TextBlock("hi "),
			domain.ImageBlock("x", "x.png", "image/png", "workspaces/W/attachments/x.png"),
			domain.TextBlock(" there"),
		},
	})
	if !ok {
		t.Fatal("expected item")
	}
	if it.Text != "hi "+AttachmentMarker("x")+" there" {
		t.Fatalf("text=%q", it.Text)
	}
	if len(it.Attachments) != 1 || it.Attachments[0].URL != "/workspaces/W/attachments/x.png" {
		t.Fatalf("attachments=%+v", it.Attachments)
	}
}

func TestFlattenUserMessageCollapsesCoderDispatch(t *testing.T) {
	wire := coderFixPrompt("pong", []domain.Finding{
		{ID: "F1", Severity: "low", File: "pong/render.py", Line: 78, Message: "hint clips"},
	})
	it, ok := flattenUserMessage(domain.Message{
		Role:    "user",
		Content: []domain.ContentBlock{domain.TextBlock(wire)},
	})
	if !ok {
		t.Fatal("expected item")
	}
	if it.Text != coderFixDisplay {
		t.Fatalf("text=%q, want %q", it.Text, coderFixDisplay)
	}
	if !strings.Contains(wire, "mcp__recur__update_finding") {
		t.Fatal("wire should still carry the full dispatch for the model")
	}
}

func TestFlattenUserMessagePrefersDisplayText(t *testing.T) {
	it, ok := flattenUserMessage(domain.Message{
		Role: "user",
		Content: []domain.ContentBlock{
			domain.TextBlockDisplay("WIRE before ", "shown before "),
			domain.ImageBlock("x", "x.png", "image/png", "artifacts/W/attachments/x.png"),
			domain.TextBlockDisplay(" WIRE after", " shown after"),
		},
	})
	if !ok {
		t.Fatal("expected item")
	}
	want := "shown before " + AttachmentMarker("x") + " shown after"
	if it.Text != want {
		t.Fatalf("text=%q, want %q", it.Text, want)
	}
	if len(it.Attachments) != 1 || it.Attachments[0].ID != "x" {
		t.Fatalf("attachments=%+v", it.Attachments)
	}
}

// userContentBlocks puts the full composer on the first text block when
// attachment markers do not pair, and leaves later segments as wire. The
// bubble must not concatenate that wire — that is how a fork transcript
// leaked into the visible user message.
func TestFlattenUserMessageSkipsWireAfterDisplayText(t *testing.T) {
	marker := AttachmentMarker("shot")
	it, ok := flattenUserMessage(domain.Message{
		Role: "user",
		Content: []domain.ContentBlock{
			domain.TextBlockDisplay(
				forkPreambleLead+"\n<transcript>\nplease help.\n",
				"please help.\n"+marker+"\ncues from the inbox",
			),
			domain.ImageBlock("shot", "shot.png", "image/png", "artifacts/W/attachments/shot.png"),
			domain.TextBlock(" I liked the labels\n<tool_use name=\"Read\"><result>roles.go</result></tool_use>"),
		},
	})
	if !ok {
		t.Fatal("expected item")
	}
	if strings.Contains(it.Text, forkPreambleLead) || strings.Contains(it.Text, "<transcript>") ||
		strings.Contains(it.Text, "tool_use") || strings.Contains(it.Text, "roles.go") {
		t.Fatalf("bubble leaked fork wire: %q", it.Text)
	}
	if !strings.Contains(it.Text, "please help.") || !strings.Contains(it.Text, "cues from the inbox") {
		t.Fatalf("bubble missing composer: %q", it.Text)
	}
	if strings.Count(it.Text, marker) != 1 {
		t.Fatalf("duplicate image marker in %q", it.Text)
	}
}

func TestUserContentBlocksIgnoresForeignAttachmentMarkers(t *testing.T) {
	imgs := []UserImage{{
		ID: "shot", Name: "shot.png", Mime: "image/png", Path: "artifacts/W/attachments/shot.png",
	}}
	marker := AttachmentMarker("shot")
	composer := "please continue " + marker + " thanks"
	preamble := forkPreambleLead + " Continue from the <transcript> below.\n\n<transcript>\n" +
		`<message role="assistant"><tool_use name="Read"><result><![CDATA[` +
		`var re = "⟦attachment:other⟧"` +
		"]]></result></tool_use></message>\n</transcript>\n"
	blocks := userContentBlocks(preamble+composer, composer, imgs)
	it, ok := flattenUserMessage(domain.Message{Role: "user", Content: blocks})
	if !ok {
		t.Fatal("expected item")
	}
	if strings.Contains(it.Text, forkPreambleLead) || strings.Contains(it.Text, "<transcript>") ||
		strings.Contains(it.Text, "tool_use") || strings.Contains(it.Text, "attachment:other") {
		t.Fatalf("bubble leaked fork wire: %q", it.Text)
	}
	if it.Text != composer {
		t.Fatalf("text=%q, want composer %q", it.Text, composer)
	}
	if len(it.Attachments) != 1 || it.Attachments[0].ID != "shot" {
		t.Fatalf("attachments=%+v", it.Attachments)
	}
}

func TestFlattenUserMessageStripsForkPreambleWithoutDisplayText(t *testing.T) {
	wire := forkPreambleLead + " Continue from the <transcript> below.\n\n<transcript>\n" +
		`<message role="user"><text>old</text></message>` + "\n</transcript>\nplease continue"
	it, ok := flattenUserMessage(domain.Message{
		Role:    "user",
		Content: []domain.ContentBlock{domain.TextBlock(wire)},
	})
	if !ok {
		t.Fatal("expected item")
	}
	if it.Text != "please continue" {
		t.Fatalf("text=%q, want the composer after </transcript>", it.Text)
	}
}

func TestUserContentBlocksImageFirstForkHidesPreamble(t *testing.T) {
	imgs := []UserImage{{
		ID: "shot", Name: "shot.png", Mime: "image/png", Path: "artifacts/W/attachments/shot.png",
	}}
	marker := AttachmentMarker("shot")
	preamble := forkPreambleLead + " Continue from the <transcript> below.\n\n<transcript>\n" +
		`<message role="assistant"><text>old reply</text></message>` + "\n</transcript>\n"
	t.Run("screenshot only", func(t *testing.T) {
		blocks := userContentBlocks(preamble+marker, marker, imgs)
		it, ok := flattenUserMessage(domain.Message{Role: "user", Content: blocks})
		if !ok {
			t.Fatal("expected item")
		}
		if strings.Contains(it.Text, forkPreambleLead) || strings.Contains(it.Text, "<transcript>") ||
			strings.Contains(it.Text, "old reply") {
			t.Fatalf("flattenUserMessage image-first screenshot leaked fork wire: %q", it.Text)
		}
		if strings.Count(it.Text, marker) != 1 {
			t.Fatalf("flattenUserMessage image-first screenshot text=%q, want one marker", it.Text)
		}
		if len(it.Attachments) != 1 || it.Attachments[0].ID != "shot" {
			t.Fatalf("attachments=%+v", it.Attachments)
		}
		got, ok := reconstructChatPrompt(domain.Message{Role: "user", Content: blocks})
		if !ok {
			t.Fatal("reconstructChatPrompt image-first screenshot: no prompt")
		}
		if !strings.Contains(got.wire, forkPreambleLead) || !strings.Contains(got.wire, "old reply") {
			t.Fatalf("reconstructChatPrompt image-first screenshot wire=%q, want stored fork inject", got.wire)
		}
	})
	t.Run("marker then text", func(t *testing.T) {
		composer := marker + " cues from the inbox"
		blocks := userContentBlocks(preamble+composer, composer, imgs)
		it, ok := flattenUserMessage(domain.Message{Role: "user", Content: blocks})
		if !ok {
			t.Fatal("expected item")
		}
		if strings.Contains(it.Text, forkPreambleLead) || strings.Contains(it.Text, "<transcript>") ||
			strings.Contains(it.Text, "old reply") {
			t.Fatalf("flattenUserMessage image-first text leaked fork wire: %q", it.Text)
		}
		if it.Text != composer {
			t.Fatalf("flattenUserMessage image-first text=%q, want composer %q", it.Text, composer)
		}
		got, ok := reconstructChatPrompt(domain.Message{Role: "user", Content: blocks})
		if !ok {
			t.Fatal("reconstructChatPrompt image-first text: no prompt")
		}
		if !strings.Contains(got.wire, forkPreambleLead) || !strings.Contains(got.wire, "cues from the inbox") {
			t.Fatalf("reconstructChatPrompt image-first text wire=%q, want stored fork inject", got.wire)
		}
	})
}

func TestFlattenUserMessageHidesImageFirstForkWithoutDisplayText(t *testing.T) {
	marker := AttachmentMarker("shot")
	preamble := forkPreambleLead + " Continue from the <transcript> below.\n\n<transcript>\n" +
		`<message role="assistant"><text>old reply</text></message>` + "\n</transcript>\n"
	it, ok := flattenUserMessage(domain.Message{
		Role: "user",
		Content: []domain.ContentBlock{
			domain.TextBlockDisplay(preamble, ""),
			domain.ImageBlock("shot", "shot.png", "image/png", "artifacts/W/attachments/shot.png"),
			domain.TextBlockDisplay(" cues from the inbox", " cues from the inbox"),
		},
	})
	if !ok {
		t.Fatal("expected item")
	}
	if strings.Contains(it.Text, forkPreambleLead) || strings.Contains(it.Text, "<transcript>") ||
		strings.Contains(it.Text, "old reply") {
		t.Fatalf("flattenUserMessage stored empty display leaked fork wire: %q", it.Text)
	}
	want := marker + " cues from the inbox"
	if it.Text != want {
		t.Fatalf("flattenUserMessage stored empty display text=%q, want %q", it.Text, want)
	}
}

func TestFlattenUserMessageKeepsQuotedForkLeadAfterImage(t *testing.T) {
	marker := AttachmentMarker("shot")
	it, ok := flattenUserMessage(domain.Message{
		Role: "user",
		Content: []domain.ContentBlock{
			domain.TextBlock("note: " + forkPreambleLead),
			domain.ImageBlock("shot", "shot.png", "image/png", "artifacts/W/attachments/shot.png"),
			domain.TextBlock(" more after the screenshot"),
		},
	})
	if !ok {
		t.Fatal("expected item")
	}
	want := "note: " + forkPreambleLead + marker + " more after the screenshot"
	if it.Text != want {
		t.Fatalf("flattenUserMessage quoted fork lead text=%q, want %q", it.Text, want)
	}
}

func TestUserContentBlocksDisplayTextWithImages(t *testing.T) {
	imgs := []UserImage{{
		ID: "a", Name: "a.png", Mime: "image/png", Path: "artifacts/W/attachments/a.png",
	}}
	marker := AttachmentMarker("a")
	// Matching markers → pair per-segment display.
	blocks := userContentBlocks("W1 "+marker+" W2", "D1 "+marker+" D2", imgs)
	if len(blocks) != 3 {
		t.Fatalf("len=%d, want 3", len(blocks))
	}
	if p, ok := blocks[0].AsText(); !ok || p.Text != "W1 " || p.DisplayText != "D1 " {
		t.Fatalf("block0 = %+v", p)
	}
	if p, ok := blocks[2].AsText(); !ok || p.Text != " W2" || p.DisplayText != " D2" {
		t.Fatalf("block2 = %+v", p)
	}
	// No markers in display → full display on first text block only.
	blocks = userContentBlocks("W1 "+marker+" W2", "composer label", imgs)
	if p, ok := blocks[0].AsText(); !ok || p.DisplayText != "composer label" {
		t.Fatalf("first display=%+v", p)
	}
	if p, ok := blocks[2].AsText(); !ok || p.DisplayText != "" {
		t.Fatalf("second should have empty display: %+v", p)
	}
}

func TestFlattenUserMessageSkipsRetrySentinel(t *testing.T) {
	_, ok := flattenUserMessage(domain.Message{
		Role: "user",
		Content: []domain.ContentBlock{
			domain.TextBlockDisplay("do the thing", chatRetryDisplaySentinel),
		},
	})
	if ok {
		t.Fatal("retry sentinel user row must not flatten into a bubble")
	}
}

func TestFlattenUserMessageSkipsModeSwitchContinue(t *testing.T) {
	_, ok := flattenUserMessage(domain.Message{
		Role: "user",
		Content: []domain.ContentBlock{
			domain.TextBlockDisplay(formatModeSwitchResumePrompt(domain.ChatModeAgent, domain.ChatModePlan),
				formatModeSwitchDisplay(domain.ChatModePlan)),
		},
	})
	if ok {
		t.Fatal("mode-switch remount user row must not flatten into a bubble")
	}
}

func TestUserImageFromStoredReloadsBytes(t *testing.T) {
	root := t.TempDir()
	t.Setenv("RECUR_DATA_DIR", root)
	png := base64.StdEncoding.EncodeToString([]byte{
		0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a,
		0x00, 0x00, 0x00, 0x00,
	})
	_, imgs, err := prepareChatAttachments("WS1", AttachmentMarker("img1"), []domain.ChatAttachment{{
		ID: "img1", Name: "shot.png", Mime: "image/png", Data: png,
	}})
	if err != nil || len(imgs) != 1 {
		t.Fatalf("prepare: err=%v imgs=%+v", err, imgs)
	}
	got, ok := userImageFromStored(domain.ImagePayload{
		ID: imgs[0].ID, Name: imgs[0].Name, Mime: imgs[0].Mime, Path: imgs[0].Path,
	})
	if !ok {
		t.Fatal("expected reload")
	}
	if got.Data != imgs[0].Data || got.ID != "img1" {
		t.Fatalf("got %+v, want data/id from stored file", got)
	}
}

func TestReconstructChatPromptOmitsMissingImage(t *testing.T) {
	p, ok := reconstructChatPrompt(domain.Message{
		Role: "user",
		Content: []domain.ContentBlock{
			domain.TextBlock("look at this"),
			domain.ImageBlock("gone", "x.png", "image/png", "workspaces/W/attachments/missing.png"),
			domain.TextBlock(" please"),
		},
	})
	if !ok {
		t.Fatal("text remainder should still retry")
	}
	if strings.Contains(p.wire, AttachmentMarker("gone")) {
		t.Fatalf("wire still has marker: %q", p.wire)
	}
	if p.wire != "look at this please" {
		t.Fatalf("wire=%q", p.wire)
	}
	if len(p.imgs) != 0 {
		t.Fatalf("imgs=%+v", p.imgs)
	}

	if _, ok := reconstructChatPrompt(domain.Message{
		Role: "user",
		Content: []domain.ContentBlock{
			domain.ImageBlock("gone", "x.png", "image/png", "workspaces/W/attachments/missing.png"),
		},
	}); ok {
		t.Fatal("image-only with a missing file should not retry")
	}
}

func TestLastChatUserPromptDoesNotFallThrough(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &captureRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "scratch", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	code := pairAgentCode
	eng.ensurePairAgent(ws.ID, code, "cursor/composer-2.5")
	sess, ok := eng.ensureAgentSession(ws.ID, string(domain.RoleCoder), code, "Agent", "cursor")
	if !ok {
		t.Fatal("ensure session")
	}
	if _, err := eng.Store.AppendMessage(sess.ID, domain.Message{
		Role:    "user",
		Content: []domain.ContentBlock{domain.TextBlock("older prompt")},
	}); err != nil {
		t.Fatal(err)
	}
	if _, err := eng.Store.AppendMessage(sess.ID, domain.Message{
		Role: "user",
		Content: []domain.ContentBlock{
			domain.ImageBlock("gone", "x.png", "image/png", "workspaces/W/attachments/missing.png"),
		},
	}); err != nil {
		t.Fatal(err)
	}

	if p, ok := eng.lastChatUserPrompt(ws.ID, code); ok {
		t.Fatalf("should refuse the newest prompt, not fall through to %q", p.wire)
	}
	reply := eng.SendWorkspaceMessage(ws.ID, code, "ignored", "sonnet", nil, nil, "agent", chatRetryDisplaySentinel)
	if !strings.Contains(reply, "nothing to retry") {
		t.Fatalf("retry reply = %q, want nothing-to-retry", reply)
	}
}

func TestBeginStartWorkspacePersistsGoalImages(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &captureRunner{})
	projID := addProject(t, st, initRepo(t))
	slots := 0
	for eng.gov.TryAcquireLoop() {
		slots++
	}
	t.Cleanup(func() {
		for i := 0; i < slots; i++ {
			eng.gov.ReleaseLoop()
		}
	})
	png := base64.StdEncoding.EncodeToString([]byte{
		0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a,
		0x00, 0x00, 0x00, 0x00,
	})
	ws, err := eng.BeginStartWorkspaceWith(projID, "look at this", "", "", "", "", "", nil, []domain.ChatAttachment{{
		ID: "img1", Name: "shot.png", Mime: "image/png", Data: png,
	}})
	if err != nil {
		t.Fatalf("BeginStartWorkspaceWith: %v", err)
	}
	imgs := eng.loopGoalImages(ws.ID, maestroAgentCode)
	if len(imgs) != 1 || imgs[0].ID != "img1" {
		t.Fatalf("loopGoalImages(%s, MA) = %+v, want [img1]", ws.ID, imgs)
	}
	a, ok := st.Agent(ws.ID, maestroAgentCode)
	if !ok || a.Status != "idle" {
		t.Errorf("BeginStartWorkspaceWith MA.Status = %q (ok=%v), want idle", a.Status, ok)
	}
}

func TestLoopGoalImagesIgnoresSharedDirectory(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &captureRunner{
		linesFor: func(call int) [][]byte { return maestroFixture("shared-dir", "OK.") },
	})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "scratch", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	png := base64.StdEncoding.EncodeToString([]byte{
		0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a,
		0x00, 0x00, 0x00, 0x00,
	})
	_ = eng.SendWorkspaceMessage(ws.ID, "PA", "see "+AttachmentMarker("imgA"), "sonnet", nil, []domain.ChatAttachment{{
		ID: "imgA", Name: "a.png", Mime: "image/png", Data: png,
	}}, "agent", "")

	slots := 0
	for eng.gov.TryAcquireLoop() {
		slots++
	}
	t.Cleanup(func() {
		for i := 0; i < slots; i++ {
			eng.gov.ReleaseLoop()
		}
	})
	tabB := eng.CreateWorkspaceChat(ws.ID)
	reply := eng.SendWorkspaceMessage(ws.ID, tabB.Code, "text only goal", "", nil, nil, "loop", "")
	if strings.Contains(reply, "I couldn't engage") {
		t.Fatalf("engage = %q", reply)
	}
	imgs := eng.loopGoalImages(ws.ID, tabB.Code)
	if len(imgs) != 0 {
		t.Fatalf("loopGoalImages(%s) = %+v, want empty (sibling paste is not this loop's goal)", tabB.Code, imgs)
	}
}
