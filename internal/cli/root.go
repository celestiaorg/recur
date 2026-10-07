package cli

import (
	"fmt"
	"os"
	"path/filepath"
	"strings"

	"github.com/spf13/cobra"
	"github.com/spf13/viper"
)

// Execute runs the recur CLI (os.Args). Returns a process exit code.
func Execute() int {
	return execute(os.Args[1:])
}

// ExecuteBundlesForward runs as if invoked as `recur bundles ...` (legacy cmd/bundles).
func ExecuteBundlesForward(args []string) int {
	return execute(append([]string{"bundles"}, args...))
}

func execute(args []string) int {
	root := newRootCmd()
	root.SetArgs(args)
	if err := root.Execute(); err != nil {
		return exitCode(err)
	}
	return 0
}

func newRootCmd() *cobra.Command {
	cmd := &cobra.Command{
		Use:   "recur [path]",
		Short: "Open a path in Recur, or run authoring tools",
		Long: `Recur command-line tools.

  recur .                      open the current folder
  recur ./path/to/file         open a file or folder
  recur                        focus Recur (launch if needed)
  recur bundles test …         author Recur Bundles`,
		SilenceUsage:  true,
		SilenceErrors: true,
		Args:          cobra.ArbitraryArgs,
		PersistentPreRunE: func(cmd *cobra.Command, args []string) error {
			return initConfig()
		},
		RunE: func(cmd *cobra.Command, args []string) error {
			if len(args) == 0 {
				return runOpen("")
			}
			if !looksLikeOpenPath(args[0]) {
				return fmt.Errorf("unknown command %q for %q", args[0], cmd.CommandPath())
			}
			if len(args) > 1 {
				return fmt.Errorf("accepts at most 1 path argument, received %d", len(args))
			}
			return runOpen(args[0])
		},
	}
	cmd.PersistentFlags().String("bundles-dir", "", "user bundles root (env RECUR_BUNDLES_DIR)")
	_ = viper.BindPFlag("bundles-dir", cmd.PersistentFlags().Lookup("bundles-dir"))

	cmd.AddCommand(newBundlesCmd())
	cmd.AddCommand(newOpenCmd())
	return cmd
}

// looksLikeOpenPath reports an argument that should be opened rather than
// treated as a mistyped subcommand: `.` / `..`, anything with a separator,
// or a name that exists on disk.
func looksLikeOpenPath(s string) bool {
	s = strings.TrimSpace(s)
	if s == "" || s == "." || s == ".." {
		return true
	}
	if strings.ContainsRune(s, os.PathSeparator) || strings.Contains(s, "/") || strings.Contains(s, `\`) {
		return true
	}
	if _, err := os.Stat(s); err == nil {
		return true
	}
	if _, err := os.Stat(filepath.Clean(s)); err == nil {
		return true
	}
	return false
}

type exitError struct {
	code int
	err  error
}

func (e *exitError) Error() string {
	if e.err == nil {
		return fmt.Sprintf("exit %d", e.code)
	}
	return e.err.Error()
}

func exitCode(err error) int {
	if err == nil {
		return 0
	}
	if ee, ok := err.(*exitError); ok {
		if ee.err != nil {
			fmt.Fprintln(os.Stderr, paintRed(ee.err.Error(), wantColor(os.Stderr)))
		}
		return ee.code
	}
	fmt.Fprintln(os.Stderr, paintRed(err.Error(), wantColor(os.Stderr)))
	return 1
}
