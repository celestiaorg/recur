package cursor

import "testing"

func TestUserDisplayNameAndSummary(t *testing.T) {
	u := User{
		APIKeyName:    "Recur laptop",
		UserEmail:     "dev@example.com",
		UserFirstName: "Josh",
		UserLastName:  "Dechant",
	}
	if got := u.DisplayName(); got != "Josh Dechant" {
		t.Fatalf("DisplayName = %q", got)
	}
	if got := u.Summary(); got != "dev@example.com" {
		t.Fatalf("Summary = %q", got)
	}

	keyOnly := User{APIKeyName: "ci-bot"}
	if got := keyOnly.Summary(); got != "ci-bot" {
		t.Fatalf("Summary without email = %q", got)
	}
	if got := (User{UserFirstName: "Ada"}).DisplayName(); got != "Ada" {
		t.Fatalf("first-only DisplayName = %q", got)
	}
}
