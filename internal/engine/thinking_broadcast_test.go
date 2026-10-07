package engine

import (
	"testing"
)

// TestThinkingBroadcastReplacesAfterDeltas: each thinking publish is a
// cumulative replace:true snapshot. The complete KindThinking block must
// still publish as replace:true — the UI stamps that text onto the Thought
// row and closes it so a following assistant reply cannot splice into the body.
func TestThinkingBroadcastReplacesAfterDeltas(t *testing.T) {
	bus := NewBus()
	var got []map[string]any
	bus.Subscribe(func(ev Event) {
		if ev.Name != "session.thinking" {
			return
		}
		d, _ := ev.Data.(map[string]any)
		got = append(got, d)
	})
	b := &sessionBroadcast{e: &Engine{Bus: bus}, wsID: "ws-1", code: "PA"}

	b.emit(ParsedEvent{Kind: KindThinkingDelta, Text: "half "})
	b.emit(ParsedEvent{Kind: KindThinkingDelta, Text: "a thought"})
	b.emit(ParsedEvent{Kind: KindThinking, Text: "half a thought"})

	if len(got) != 3 {
		t.Fatalf("want 2 deltas + 1 replace, got %d: %+v", len(got), got)
	}
	// Deltas publish cumulative snapshots with replace:true.
	if got[0]["text"] != "half " || got[0]["replace"] != true {
		t.Errorf("first delta wrong: %+v", got[0])
	}
	if got[1]["text"] != "half a thought" || got[1]["replace"] != true {
		t.Errorf("second delta wrong: %+v", got[1])
	}
	if got[2]["text"] != "half a thought" || got[2]["replace"] != true {
		t.Errorf("complete block should publish replace:true, got %+v", got[2])
	}
	if b.gotThinking {
		t.Error("gotThinking should reset after replace so the next block can stream")
	}
}

func TestNoticeBroadcast(t *testing.T) {
	bus := NewBus()
	var notices, deltas int
	var text string
	bus.Subscribe(func(ev Event) {
		d, _ := ev.Data.(map[string]any)
		switch ev.Name {
		case "session.notice":
			notices++
			text, _ = d["text"].(string)
			if d["workspaceId"] != "ws-1" || d["code"] != "PA" {
				t.Errorf("session.notice tags = %+v, want workspaceId=ws-1 code=PA", d)
			}
		case "session.delta":
			deltas++
		}
	})
	b := &sessionBroadcast{e: &Engine{Bus: bus}, wsID: "ws-1", code: "PA"}
	b.emit(ParsedEvent{Kind: KindNotice, Text: "Waiting for previous run run-1 to finish."})
	b.emit(ParsedEvent{Kind: KindNotice, Text: "   "})
	b.emit(ParsedEvent{Kind: KindText, Text: "hello"})
	if notices != 1 {
		t.Fatalf("session.notice count = %d, want 1", notices)
	}
	if text != "Waiting for previous run run-1 to finish." {
		t.Errorf("session.notice text = %q, want waiting message", text)
	}
	if deltas != 1 {
		t.Fatalf("session.delta count = %d, want 1 (notice must not fold into delta)", deltas)
	}
}

// Without deltas, the complete thinking block still streams once (no replace).
func TestThinkingBroadcastWithoutDeltas(t *testing.T) {
	bus := NewBus()
	var got []map[string]any
	bus.Subscribe(func(ev Event) {
		if ev.Name != "session.thinking" {
			return
		}
		d, _ := ev.Data.(map[string]any)
		got = append(got, d)
	})
	b := &sessionBroadcast{e: &Engine{Bus: bus}, wsID: "ws-1", code: "PA"}

	b.emit(ParsedEvent{Kind: KindThinking, Text: "one shot"})
	if len(got) != 1 || got[0]["text"] != "one shot" || got[0]["replace"] == true {
		t.Errorf("want a single non-replace publish, got %+v", got)
	}
}

// Consecutive turns must keep a monotonic thinking seq. The UI drops events with
// seq <= last-seen; restarting at 1 each turn left live Thought rows stuck on
// token estimates while the persisted transcript (reload) had the full text.
func TestThinkingBroadcastSeqMonotonicAcrossTurns(t *testing.T) {
	eng := &Engine{Bus: NewBus()}
	var seqs []int
	eng.Bus.Subscribe(func(ev Event) {
		if ev.Name != "session.thinking" {
			return
		}
		d, _ := ev.Data.(map[string]any)
		if seq, ok := d["seq"].(int); ok {
			seqs = append(seqs, seq)
		}
	})

	turn1 := &sessionBroadcast{e: eng, wsID: "ws-1", code: "PA"}
	turn1.emit(ParsedEvent{Kind: KindThinkingDelta, Text: "first "})
	turn1.emit(ParsedEvent{Kind: KindThinkingDelta, Text: "turn"})
	turn1.emit(ParsedEvent{Kind: KindThinking, Text: "first turn"})

	// Same seed path newSessionBroadcast uses between turns.
	textSeq, thinkSeq := eng.streamSeqSeed("ws-1", "PA")
	turn2 := &sessionBroadcast{e: eng, wsID: "ws-1", code: "PA", textSeq: textSeq, thinkSeq: thinkSeq}
	turn2.emit(ParsedEvent{Kind: KindThinkingDelta, Text: "second"})
	turn2.emit(ParsedEvent{Kind: KindThinking, Text: "second"})

	if len(seqs) != 5 {
		t.Fatalf("want 5 seq'd events, got %d: %v", len(seqs), seqs)
	}
	for i := 1; i < len(seqs); i++ {
		if seqs[i] <= seqs[i-1] {
			t.Fatalf("thinking seq not monotonic across turns: %v", seqs)
		}
	}
	if thinkSeq != 3 {
		t.Fatalf("turn 2 should seed thinkSeq=3 after turn 1, got %d", thinkSeq)
	}
}

// Claude Code assigns a unique uuid to every stream-json line. Those are not
// assistant-frame ids — treating them as such reset the cumulative buffer and
// made the UI open a Thought row per token.
func TestThinkingBroadcastIgnoresPerEventUUIDs(t *testing.T) {
	bus := NewBus()
	var got []map[string]any
	bus.Subscribe(func(ev Event) {
		if ev.Name != "session.thinking" {
			return
		}
		d, _ := ev.Data.(map[string]any)
		got = append(got, d)
	})
	b := &sessionBroadcast{e: &Engine{Bus: bus}, wsID: "ws-1", code: "PA"}

	b.emit(ParsedEvent{Kind: KindThinkingDelta, Text: "Act", UUID: "evt-1"})
	b.emit(ParsedEvent{Kind: KindThinkingTokens, Tokens: 326, UUID: "evt-2"})
	b.emit(ParsedEvent{Kind: KindThinkingDelta, Text: "ually", UUID: "evt-3"})
	b.emit(ParsedEvent{Kind: KindThinking, Text: "Actually the bigger issue", UUID: "asst-frame"})

	if len(got) != 4 {
		t.Fatalf("want 3 live + 1 complete, got %d: %+v", len(got), got)
	}
	if got[0]["text"] != "Act" || got[0]["uuid"] != nil {
		t.Errorf("first delta must accumulate without event uuid: %+v", got[0])
	}
	if got[1]["tokens"] != int64(326) {
		t.Errorf("token tick wrong: %+v", got[1])
	}
	if _, ok := got[1]["uuid"]; ok {
		t.Errorf("token tick must not forward event uuid: %+v", got[1])
	}
	if got[2]["text"] != "Actually" || got[2]["replace"] != true {
		t.Errorf("second delta should append (not reset on uuid change): %+v", got[2])
	}
	if got[3]["text"] != "Actually the bigger issue" || got[3]["replace"] != true || got[3]["done"] != true {
		t.Errorf("complete block wrong: %+v", got[3])
	}
	if got[3]["uuid"] != "asst-frame" {
		t.Errorf("complete block should keep assistant frame uuid: %+v", got[3])
	}
}
