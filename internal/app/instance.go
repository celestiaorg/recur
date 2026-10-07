package app

import (
	"os"
	"strings"
)

const instanceLabelEnv = "RECUR_INSTANCE_LABEL"

// InstanceLabel is the optional process env Recur paints in the titlebar so
// dogfood windows are distinguishable. Empty means production chrome.
func InstanceLabel() string {
	return strings.TrimSpace(os.Getenv(instanceLabelEnv))
}

// WindowTitle is the native window title. HiddenInset hides it in the custom
// bar; Mission Control still shows it. When the instance label is set, the
// title is that label only (no Recur prefix).
func WindowTitle() string {
	if label := InstanceLabel(); label != "" {
		return label
	}
	return "Recur"
}
