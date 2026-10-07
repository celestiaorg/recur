package domain

import "strings"

// BranchPrefixMode is the legacy (pre-string) branch-prefix schema.
// New settings store BranchNamePrefix only; these values are still read when
// that pointer is nil so existing blobs keep working.
type BranchPrefixMode string

const (
	BranchPrefixRecur  BranchPrefixMode = "recur"
	BranchPrefixGitHub BranchPrefixMode = "github"
	BranchPrefixCustom BranchPrefixMode = "custom"
	BranchPrefixNone   BranchPrefixMode = "none"

	// DefaultBranchNamePrefix is the historical workspace branch namespace.
	DefaultBranchNamePrefix = "recur"
)

// PrefixString returns a pointer to s for Settings.BranchNamePrefix.
func PrefixString(s string) *string { return &s }

// EffectiveBranchNamePrefix is the path segment prepended to new workspace
// branches (`prefix/slug`). Empty means no prefix. The stored string is the
// source of truth; GitHub is not queried here (the Settings GitHub chip fills
// the string once). Legacy mode fields apply only when the string is unset.
func (s Settings) EffectiveBranchNamePrefix() string {
	if s.BranchNamePrefix != nil {
		return strings.TrimSpace(*s.BranchNamePrefix)
	}
	switch s.BranchPrefixMode {
	case BranchPrefixNone:
		return ""
	case BranchPrefixCustom:
		if p := strings.TrimSpace(s.BranchPrefixCustom); p != "" {
			return p
		}
		return DefaultBranchNamePrefix
	default:
		// Unset, "recur", "github" (unmigrated — no live gh lookup), or unknown.
		return DefaultBranchNamePrefix
	}
}
