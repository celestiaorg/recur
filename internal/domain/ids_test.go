package domain

import (
	"strings"
	"testing"

	"github.com/google/uuid"

	"recur/internal/shortid"
)

func TestNewIDFinding(t *testing.T) {
	id, err := NewID(TypeFinding)
	if err != nil {
		t.Fatalf("NewID(TypeFinding) error = %v, want nil", err)
	}
	if !ValidID(id, TypeFinding) {
		t.Errorf("NewID(TypeFinding) = %q, want a valid find_ id", id)
	}
	if !strings.HasPrefix(id, string(TypeFinding)+"_") {
		t.Errorf("NewID(TypeFinding) = %q, want prefix %s_", id, TypeFinding)
	}
}

func TestValidID(t *testing.T) {
	if !ValidID("find_k7mq2xf9", TypeFinding) {
		t.Error("ValidID(find_k7mq2xf9, TypeFinding) = false, want true")
	}
	if ValidID("usr_k7mq2xf9", TypeFinding) {
		t.Error("ValidID(usr_k7mq2xf9, TypeFinding) = true, want false")
	}
	if ValidID("find_abcde", TypeFinding) {
		t.Error("ValidID(find_abcde, TypeFinding) = true, want false")
	}
}

func TestFindingPublicIDAndMatch(t *testing.T) {
	uid := uuid.NewString()
	f := Finding{ID: uid, ShortID: "find_k7mq2xf9"}
	if got := f.PublicID(); got != "find_k7mq2xf9" {
		t.Errorf("PublicID() = %q, want find_k7mq2xf9", got)
	}
	if !f.MatchesRef(uid) || !f.MatchesRef("find_k7mq2xf9") {
		t.Errorf("MatchesRef() missed UUID or short id on %+v", f)
	}
	if f.MatchesRef("find_otherxxx") || f.MatchesRef("") {
		t.Errorf("MatchesRef() accepted a non-match on %+v", f)
	}

	legacy := Finding{ID: "F-old-1"}
	if got := legacy.PublicID(); got != "F-old-1" {
		t.Errorf("PublicID() legacy = %q, want F-old-1", got)
	}
	if !shortid.IsValid("find_k7mq2xf9") {
		t.Error("shortid.IsValid(find_k7mq2xf9) = false, want true")
	}
}
