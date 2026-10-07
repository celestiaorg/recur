package domain

import (
	"encoding/json"
	"testing"
)

func TestEscalationAppliedLegacyJSON(t *testing.T) {
	var e Escalation
	if err := json.Unmarshal([]byte(`{"workspaceId":"W","status":"answered","answer":"ok"}`), &e); err != nil {
		t.Fatal(err)
	}
	if !e.Applied {
		t.Fatal("legacy answered row with no applied field must decode as applied")
	}

	if err := json.Unmarshal([]byte(`{"workspaceId":"W","status":"answered","answer":"ok","applied":false}`), &e); err != nil {
		t.Fatal(err)
	}
	if e.Applied {
		t.Fatal("explicit applied:false is still pending")
	}

	if err := json.Unmarshal([]byte(`{"workspaceId":"W","status":"open"}`), &e); err != nil {
		t.Fatal(err)
	}
	if e.Applied {
		t.Fatal("open row without applied must stay false")
	}
}
