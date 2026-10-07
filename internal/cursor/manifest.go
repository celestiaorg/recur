package cursor

import (
	"encoding/json"
	"os"
	"path/filepath"
	"strings"
)

// SDKVersion returns the cursor-sdk version from an installed bridge's
// manifest.json. GetVersion's bridgeVersion is the bridge shell (currently
// always "1.0.0"); the pin Recur installs and users care about is sdkVersion.
func SDKVersion(binOrRoot string) string {
	root := BridgeRoot(binOrRoot)
	if root == "" {
		return ""
	}
	data, err := os.ReadFile(filepath.Join(root, "manifest.json"))
	if err != nil {
		return ""
	}
	var m struct {
		SDKVersion string `json:"sdkVersion"`
	}
	if json.Unmarshal(data, &m) != nil {
		return ""
	}
	return strings.TrimSpace(m.SDKVersion)
}
