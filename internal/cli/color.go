package cli

import "os"

// wantColor follows NO_COLOR / FORCE_COLOR and whether fd is a terminal.
// FORCE_COLOR=1 wins over NO_COLOR; FORCE_COLOR=0 / empty is ignored.
func wantColor(f *os.File) bool {
	if force, ok := colorForceEnv(); ok {
		return force
	}
	if os.Getenv("NO_COLOR") != "" {
		return false
	}
	fi, err := f.Stat()
	if err != nil {
		return false
	}
	return fi.Mode()&os.ModeCharDevice != 0
}

func colorForceEnv() (force bool, set bool) {
	for _, key := range []string{"FORCE_COLOR", "CLICOLOR_FORCE"} {
		v, ok := os.LookupEnv(key)
		if !ok || v == "" {
			continue
		}
		if v == "0" || v == "false" || v == "off" {
			return false, true
		}
		return true, true
	}
	return false, false
}

func paintRed(s string, color bool) string {
	if !color {
		return s
	}
	return "\033[31m" + s + "\033[0m"
}
