package engine

import (
	"encoding/base64"
	"fmt"
	"os"
	"path/filepath"
	"regexp"
	"strings"
	"unicode"

	"recur/internal/domain"
	"recur/internal/store"
)

// Attachment marker appended after composer text (chip order) on send.
// Claude splits these into interleaved text/image content blocks; Cursor
// receives text + images[] ordered by marker appearance.
const attachmentMarkerPrefix = "⟦attachment:"
const attachmentMarkerSuffix = "⟧"

var attachmentMarkerRE = regexp.MustCompile(`⟦attachment:([^⟧]+)⟧`)

// displayHiddenSentinel is display_text for a stored wire segment that must not
// appear in the chat bubble (image-first fork inject). Reconstruct/Retry read
// Text; flatten treats this label as used-and-empty so later host wire stays hidden.
const displayHiddenSentinel = "\u2064"

// maxChatAttachmentBytes caps one pasted image (matches observatory preview).
const maxChatAttachmentBytes = 8 * 1024 * 1024

// chatImageMIME maps lowercase extensions / MIME types we accept for chat paste.
var chatImageMIME = map[string]string{
	"image/png":  ".png",
	"image/jpeg": ".jpg",
	"image/jpg":  ".jpg",
	"image/gif":  ".gif",
	"image/webp": ".webp",
}

// AttachmentMarker returns the in-text marker for a chat attachment id.
func AttachmentMarker(id string) string {
	return attachmentMarkerPrefix + id + attachmentMarkerSuffix
}

// stripAttachmentMarkers drops ⟦attachment:id⟧ runs from titles and loop goals.
func stripAttachmentMarkers(s string) string {
	s = attachmentMarkerRE.ReplaceAllString(s, "")
	return strings.TrimSpace(s)
}

// ensureAttachmentMarkers appends any missing ⟦attachment:id⟧ runs so Claude
// and Cursor can pair spec.Images with the prompt body.
func ensureAttachmentMarkers(text string, imgs []UserImage) string {
	for _, img := range imgs {
		m := AttachmentMarker(img.ID)
		if strings.Contains(text, m) {
			continue
		}
		if text != "" && !strings.HasSuffix(text, "\n") {
			text += "\n"
		}
		text += m
	}
	return text
}

// workspaceUserImages reloads every saved chat paste for this workspace.
func workspaceUserImages(workspaceID string) []UserImage {
	workspaceID = strings.TrimSpace(workspaceID)
	if workspaceID == "" {
		return nil
	}
	root, err := store.DataDir()
	if err != nil {
		return nil
	}
	dir, err := store.AttachmentsDir(root, workspaceID)
	if err != nil {
		return nil
	}
	entries, err := os.ReadDir(dir)
	if err != nil {
		return nil
	}
	var out []UserImage
	for _, e := range entries {
		if e.IsDir() {
			continue
		}
		name := e.Name()
		ext := strings.ToLower(filepath.Ext(name))
		mime := ""
		for m, x := range chatImageMIME {
			if x == ext {
				mime = m
				break
			}
		}
		if mime == "" {
			continue
		}
		if mime == "image/jpg" {
			mime = "image/jpeg"
		}
		id := sanitizeAttachmentID(strings.TrimSuffix(name, ext))
		if id == "" {
			continue
		}
		rel := filepath.ToSlash(filepath.Join("workspaces", workspaceID, "attachments", name))
		img, ok := userImageFromStored(domain.ImagePayload{ID: id, Name: name, Mime: mime, Path: rel})
		if ok {
			out = append(out, img)
		}
	}
	return out
}

// userImagesByIDs reloads the given attachment ids from this workspace's paste dir.
func userImagesByIDs(workspaceID string, ids []string) []UserImage {
	if len(ids) == 0 {
		return nil
	}
	out := make([]UserImage, 0, len(ids))
	for _, id := range ids {
		if img, ok := userImageByID(workspaceID, id); ok {
			out = append(out, img)
		}
	}
	return out
}

func userImageByID(workspaceID, id string) (UserImage, bool) {
	workspaceID = strings.TrimSpace(workspaceID)
	id = sanitizeAttachmentID(id)
	if workspaceID == "" || id == "" {
		return UserImage{}, false
	}
	root, err := store.DataDir()
	if err != nil {
		return UserImage{}, false
	}
	dir, err := store.AttachmentsDir(root, workspaceID)
	if err != nil {
		return UserImage{}, false
	}
	seenExt := map[string]bool{}
	for mime, ext := range chatImageMIME {
		if seenExt[ext] {
			continue
		}
		seenExt[ext] = true
		filename := id + ext
		if _, err := os.Stat(filepath.Join(dir, filename)); err != nil {
			continue
		}
		if mime == "image/jpg" {
			mime = "image/jpeg"
		}
		rel := filepath.ToSlash(filepath.Join("workspaces", workspaceID, "attachments", filename))
		return userImageFromStored(domain.ImagePayload{ID: id, Name: filename, Mime: mime, Path: rel})
	}
	return UserImage{}, false
}

// UserImage is one image attached to an AgentSpec turn (bytes already validated).
type UserImage struct {
	ID   string
	Name string
	Mime string
	Data string // raw base64
	Path string // relative to data root (workspaces/…/attachments/…)
}

// prepareChatAttachments validates pasted images, writes them under
// workspaces/<workspaceID>/attachments/, and returns the prompt text (unchanged
// markers) plus UserImage values for the agent turn / session mirror.
func prepareChatAttachments(workspaceID, text string, atts []domain.ChatAttachment) (string, []UserImage, error) {
	text = strings.TrimSpace(text)
	if len(atts) == 0 {
		return text, nil, nil
	}
	root, err := store.DataDir()
	if err != nil {
		return "", nil, err
	}
	dir, err := store.AttachmentsDir(root, workspaceID)
	if err != nil {
		return "", nil, err
	}

	seen := map[string]bool{}
	out := make([]UserImage, 0, len(atts))
	for _, a := range atts {
		id := sanitizeAttachmentID(a.ID)
		if id == "" || seen[id] {
			continue
		}
		mime := strings.ToLower(strings.TrimSpace(a.Mime))
		ext, ok := chatImageMIME[mime]
		if !ok {
			continue
		}
		if mime == "image/jpg" {
			mime = "image/jpeg"
		}
		raw, err := base64.StdEncoding.DecodeString(strings.TrimSpace(a.Data))
		if err != nil || len(raw) == 0 {
			continue
		}
		if len(raw) > maxChatAttachmentBytes {
			return "", nil, fmt.Errorf("attachment %s too large (%d bytes)", id, len(raw))
		}
		name := strings.TrimSpace(a.Name)
		if name == "" {
			name = id + ext
		}
		filename := id + ext
		full := filepath.Join(dir, filename)
		if err := os.WriteFile(full, raw, 0o644); err != nil {
			return "", nil, err
		}
		rel := filepath.ToSlash(filepath.Join("workspaces", workspaceID, "attachments", filename))
		seen[id] = true
		out = append(out, UserImage{
			ID: id, Name: name, Mime: mime,
			Data: base64.StdEncoding.EncodeToString(raw),
			Path: rel,
		})
		// Ensure the prompt mentions every attachment even if the UI omitted a marker.
		if !strings.Contains(text, AttachmentMarker(id)) {
			if text != "" {
				text += "\n"
			}
			text += AttachmentMarker(id)
		}
	}
	return text, out, nil
}

// userImageFromStored reloads a persisted chat attachment from its data-dir path
// so a Retry can send the same image bytes the original turn used.
func userImageFromStored(p domain.ImagePayload) (UserImage, bool) {
	if p.ID == "" || p.Path == "" {
		return UserImage{}, false
	}
	root, err := store.DataDir()
	if err != nil {
		return UserImage{}, false
	}
	full := filepath.Join(root, filepath.FromSlash(p.Path))
	raw, err := os.ReadFile(full)
	if err != nil || len(raw) == 0 {
		return UserImage{}, false
	}
	if len(raw) > maxChatAttachmentBytes {
		return UserImage{}, false
	}
	return UserImage{
		ID: p.ID, Name: p.Name, Mime: p.Mime,
		Data: base64.StdEncoding.EncodeToString(raw),
		Path: p.Path,
	}, true
}

func sanitizeAttachmentID(id string) string {
	id = strings.TrimSpace(id)
	if id == "" {
		return ""
	}
	var b strings.Builder
	for _, r := range id {
		if unicode.IsLetter(r) || unicode.IsDigit(r) || r == '-' || r == '_' {
			b.WriteRune(r)
		}
	}
	out := b.String()
	if len(out) > 64 {
		out = out[:64]
	}
	return out
}

// userContentBlocks splits prompt text on attachment markers into ordered
// domain content blocks (text + image), using imgs for image metadata.
// displayText is UI-only: with no images it lands on the single text block; with
// images, matching marker layout pairs per-segment display_text, otherwise the
// full displayText is set on the first text block only.
//
// Only markers whose ids are in imgs are split points. A fork inject's copied
// transcript often quotes Recur source that contains ⟦attachment:…⟧ as literal
// text; treating those as images broke pairing and leaked the wire into the bubble.
func userContentBlocks(text, displayText string, imgs []UserImage) []domain.ContentBlock {
	byID := map[string]UserImage{}
	for _, img := range imgs {
		byID[img.ID] = img
	}
	textLocs := imageAttachmentLocs(text, byID)
	if len(imgs) == 0 || len(textLocs) == 0 {
		if strings.TrimSpace(text) == "" {
			return nil
		}
		return []domain.ContentBlock{domain.TextBlockDisplay(text, displayText)}
	}
	textSegs := attachmentTextSegments(text, textLocs)
	var dispSegs []string
	if displayText != "" {
		dispLocs := imageAttachmentLocs(displayText, byID)
		if attachmentMarkersMatch(text, textLocs, displayText, dispLocs) {
			dispSegs = attachmentTextSegments(displayText, dispLocs)
		}
	}
	var blocks []domain.ContentBlock
	displayAssigned := false
	for i, seg := range textSegs {
		if seg != "" {
			dt := ""
			if dispSegs != nil {
				dt = dispSegs[i]
				// Image-first composer: the paired display segment before the
				// marker is empty, while the wire segment is the whole fork
				// inject. Keep the wire for Retry; hide it at the display boundary.
				if dt == "" {
					dt = displayHiddenSentinel
				}
			} else if !displayAssigned && displayText != "" {
				dt = displayText
				displayAssigned = true
			}
			blocks = append(blocks, domain.TextBlockDisplay(seg, dt))
		}
		if i >= len(textLocs) {
			continue
		}
		loc := textLocs[i]
		id := text[loc[2]:loc[3]]
		if img, ok := byID[id]; ok {
			blocks = append(blocks, domain.ImageBlock(img.ID, img.Name, img.Mime, img.Path))
		} else {
			blocks = append(blocks, domain.TextBlock(text[loc[0]:loc[1]]))
		}
	}
	// Drop empty leading/trailing text blocks created by adjacent markers.
	out := blocks[:0]
	for _, b := range blocks {
		if p, ok := b.AsText(); ok && p.Text == "" {
			continue
		}
		out = append(out, b)
	}
	return out
}

// attachmentTextSegments returns the text pieces around marker matches
// (len = len(locs)+1), including empty leading/trailing segments.
func attachmentTextSegments(s string, locs [][]int) []string {
	segs := make([]string, 0, len(locs)+1)
	last := 0
	for _, loc := range locs {
		segs = append(segs, s[last:loc[0]])
		last = loc[1]
	}
	segs = append(segs, s[last:])
	return segs
}

// attachmentMarkersMatch reports whether two strings share the same attachment
// marker ids in the same order (so display segments can pair with wire segments).
func attachmentMarkersMatch(a string, aLocs [][]int, b string, bLocs [][]int) bool {
	if len(aLocs) != len(bLocs) {
		return false
	}
	for i := range aLocs {
		if a[aLocs[i][2]:aLocs[i][3]] != b[bLocs[i][2]:bLocs[i][3]] {
			return false
		}
	}
	return true
}

// imageAttachmentLocs returns marker match indexes whose ids are in byID.
// Other ⟦attachment:…⟧ runs (quoted source in a fork transcript) stay in text.
func imageAttachmentLocs(s string, byID map[string]UserImage) [][]int {
	all := attachmentMarkerRE.FindAllStringSubmatchIndex(s, -1)
	if len(all) == 0 || len(byID) == 0 {
		return nil
	}
	out := make([][]int, 0, len(all))
	for _, loc := range all {
		if _, ok := byID[s[loc[2]:loc[3]]]; ok {
			out = append(out, loc)
		}
	}
	return out
}

// wrapAttachedImagesForCursor prepends a system_instruction that maps each
// ⟦attachment:id⟧ marker to its ordinal in SDKUserMessage.images (marker order).
// Markers stay in the body so the model knows where each image sits; the
// instruction tells it not to treat ids as worktree paths. No-op with no images.
func wrapAttachedImagesForCursor(text string, imgs []UserImage) string {
	if len(imgs) == 0 {
		return text
	}
	ordered := imagesInMarkerOrder(text, imgs)
	if len(ordered) == 0 {
		return text
	}
	var b strings.Builder
	b.WriteString("<system_instruction>\n")
	b.WriteString("The user attached image(s) as a multimodal message (SDKUserMessage.images).\n")
	b.WriteString("Markers like ⟦attachment:id⟧ in the text mark where each image sits; they are NOT files to open or search for.\n")
	b.WriteString("The images payload is ordered to match those markers:\n")
	for i, img := range ordered {
		n := i + 1
		fmt.Fprintf(&b, "- %s is image %d (%s attached image)\n",
			AttachmentMarker(img.ID), n, ordinalWord(n))
	}
	b.WriteString("</system_instruction>\n\n")
	b.WriteString(text)
	return b.String()
}

// claudeAttachmentIDInstruction surfaces ⟦attachment:id⟧ ids to Claude. The
// image blocks replace those markers in the body, so without this prefix the
// model never sees the ids submit_plan.attachments needs.
func claudeAttachmentIDInstruction(text string, imgs []UserImage) string {
	ordered := imagesInMarkerOrder(text, imgs)
	if len(ordered) == 0 {
		return ""
	}
	var b strings.Builder
	b.WriteString("<system_instruction>\n")
	b.WriteString("The user attached image(s). Markers like ⟦attachment:id⟧ identify each image for tools such as submit_plan.attachments; they are NOT files to open or search for.\n")
	for i, img := range ordered {
		n := i + 1
		fmt.Fprintf(&b, "- %s is image %d (%s attached image)\n",
			AttachmentMarker(img.ID), n, ordinalWord(n))
	}
	b.WriteString("</system_instruction>\n")
	return b.String()
}

func prependClaudeAttachmentIDs(text string, imgs []UserImage, blocks []any) []any {
	if instr := claudeAttachmentIDInstruction(text, imgs); instr != "" {
		return append([]any{map[string]any{"type": "text", "text": instr}}, blocks...)
	}
	return blocks
}

func ordinalWord(n int) string {
	switch n {
	case 1:
		return "first"
	case 2:
		return "second"
	case 3:
		return "third"
	case 4:
		return "fourth"
	case 5:
		return "fifth"
	default:
		return fmt.Sprintf("%dth", n)
	}
}

// imagesInMarkerOrder returns UserImages ordered as their markers appear in text.
func imagesInMarkerOrder(text string, imgs []UserImage) []UserImage {
	byID := map[string]UserImage{}
	for _, img := range imgs {
		byID[img.ID] = img
	}
	var ordered []UserImage
	seen := map[string]bool{}
	for _, m := range attachmentMarkerRE.FindAllStringSubmatch(text, -1) {
		id := m[1]
		if img, ok := byID[id]; ok && !seen[id] {
			ordered = append(ordered, img)
			seen[id] = true
		}
	}
	for _, img := range imgs {
		if !seen[img.ID] {
			ordered = append(ordered, img)
		}
	}
	return ordered
}
