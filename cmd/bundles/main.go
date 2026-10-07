// Deprecated: use `recur bundles ...` (cmd/recur). This binary forwards for
// compatibility with earlier `go run ./cmd/bundles` docs.
package main

import (
	"fmt"
	"os"

	"recur/internal/cli"
)

func main() {
	fmt.Fprintln(os.Stderr, "note: use `recur bundles ...` (go run ./cmd/recur bundles ...)")
	os.Exit(cli.ExecuteBundlesForward(os.Args[1:]))
}
