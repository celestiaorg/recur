// Command recur is the Recur CLI (open a path in the desktop app, plus authoring tools).
//
// Packaged Recur.app embeds this binary at Contents/MacOS/recur-cli and
// symlinks it onto PATH at launch. Contributors: wails3 task build:cli
// (bin/recur-cli) or go install ./cmd/recur.
//
//	recur .
//	recur ./path/to/folder-or-file
//	recur bundles test Text
//	recur bundles lint ./bundles
//	recur bundles create MyPack
package main

import (
	"os"

	"recur/internal/cli"
)

func main() {
	os.Exit(cli.Execute())
}
