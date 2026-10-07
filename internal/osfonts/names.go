package osfonts

import (
	"regexp"
	"strings"
)

// Families that are monospace even when the fixed-pitch bit is missing/wrong.
// Keep this specific — bare "code"/"fira" match proportional faces.
var monoNameRe = regexp.MustCompile(`(?i)^(` + strings.Join([]string{
	`.*\bmono\b.*`,
	`.*\bconsolas\b.*`,
	`courier(\s+new)?$`,
	`.*\binconsolata\b.*`,
	`menlo.*`,
	`monaco$`,
	`.*\biosevka\b.*`,
	`.*\bmeslo\b.*`,
	`.*\bjetbrains\b.*`,
	`hack(\s+nerd\s+font|\s+nf)?$`,
	`fira\s*(code|mono).*`,
	`cascadia\s*(code|mono).*`,
	`source\s*code\s*pro.*`,
	`ibm\s*plex\s*mono.*`,
	`ubuntu\s*mono.*`,
	`dejavu\s*sans\s*mono.*`,
	`liberation\s*mono.*`,
	`noto\s*sans\s*mono.*`,
	`droid\s*sans\s*mono.*`,
	`anonymous\s*pro.*`,
	`pt\s*mono.*`,
	`andale\s*mono.*`,
	`commit\s*mono.*`,
	`geist\s*mono.*`,
	`input\s*mono.*`,
	`operator\s*mono.*`,
	`berkeley\s*mono.*`,
	`sauce\s*code.*`,
	`caskaydia\s*cove.*`,
	`press\s*start\s*2p$`,
}, "|") + `)$`)

// Fontconfig on macOS tags some non-mono faces as spacing=mono (emoji, bitmaps…).
func isJunkMonoName(name string) bool {
	lower := strings.ToLower(strings.TrimSpace(name))
	for _, bad := range []string{
		"emoji", "lastresort", "last resort", "bitmap", "braille",
		"ornament", "dingbat", "wingding", "symbol", "notodef",
		"apple color", "gb18030", "times lt mm", "multiple master",
	} {
		if strings.Contains(lower, bad) {
			return true
		}
	}
	return false
}
