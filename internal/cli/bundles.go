package cli

import (
	"context"
	"fmt"
	"os"

	"github.com/spf13/cobra"

	"recur/internal/bundles"
)

func newBundlesCmd() *cobra.Command {
	cmd := &cobra.Command{
		Use:   "bundles",
		Short: "Author and test Recur Bundles",
		Long:  "Create, test, lint, and manage Recur Bundle packs (TOML macros for the editor).",
	}
	cmd.AddCommand(newBundlesTestCmd())
	cmd.AddCommand(newBundlesLintCmd())
	cmd.AddCommand(newBundlesCreateCmd())
	return cmd
}

func newBundlesTestCmd() *cobra.Command {
	return &cobra.Command{
		Use:   "test [name|path]...",
		Short: "Run tests/*.toml for one or more bundles",
		Long: `Run in-bundle tests through the real engine (TypeScript via esbuild+node, shebang via OS).

Args may be bundle names (resolved under --bundles-dir, ./bundles, shipped) or paths.
With no args, tests the current directory (or every child pack).`,
		Example: `  recur bundles test Text
  recur bundles test ./bundles/Source
  recur bundles test ~/.recur/bundles/MyPack
  recur bundles test ./bundles`,
		Args: cobra.ArbitraryArgs,
		RunE: func(cmd *cobra.Command, args []string) error {
			dirs, err := resolveBundleTargets(args)
			if err != nil {
				return &exitError{code: 2, err: err}
			}
			if len(dirs) == 0 {
				return &exitError{code: 2, err: fmt.Errorf("no bundle.toml found")}
			}
			cache, err := os.MkdirTemp("", "recur-bundle-test-cache-*")
			if err != nil {
				return &exitError{code: 2, err: err}
			}
			defer os.RemoveAll(cache)

			ctx := cmd.Context()
			if ctx == nil {
				ctx = context.Background()
			}
			var total bundles.Report
			for _, dir := range dirs {
				rep, err := bundles.RunBundleTests(ctx, dir, cache)
				if err != nil {
					return &exitError{code: 1, err: fmt.Errorf("%s: %w", dir, err)}
				}
				total.Results = append(total.Results, rep.Results...)
				total.Passed += rep.Passed
				total.Failed += rep.Failed
			}
			fmt.Fprint(cmd.OutOrStdout(), bundles.FormatReport(total, wantColor(os.Stdout)))
			if !total.OK() {
				return &exitError{code: 1}
			}
			return nil
		},
	}
}

func newBundlesLintCmd() *cobra.Command {
	return &cobra.Command{
		Use:   "lint [name|path]...",
		Short: "Lint bundle.toml, scripts, and keyEquivalent collisions",
		Long: `Validate Recur Bundles: manifest schema, TypeScript compile, shebang #!,
and keyEquivalent parse / reserved-app collisions.

Shared chords across commands and snippets are reported as info (allowed; runtime shows a menu).
Args resolve like bundles test.`,
		Example: `  recur bundles lint Text
  recur bundles lint ./bundles
  recur bundles lint ~/.recur/bundles/MyPack`,
		Args: cobra.ArbitraryArgs,
		RunE: func(cmd *cobra.Command, args []string) error {
			dirs, err := resolveBundleTargets(args)
			if err != nil {
				return &exitError{code: 2, err: err}
			}
			if len(dirs) == 0 {
				return &exitError{code: 2, err: fmt.Errorf("no bundle.toml found")}
			}
			cache, err := os.MkdirTemp("", "recur-bundle-lint-cache-*")
			if err != nil {
				return &exitError{code: 2, err: err}
			}
			defer os.RemoveAll(cache)

			rep, err := bundles.LintBundles(dirs, cache)
			if err != nil {
				return &exitError{code: 1, err: err}
			}
			fmt.Fprint(cmd.OutOrStdout(), bundles.FormatLintReport(rep, wantColor(os.Stdout)))
			if !rep.OK() {
				return &exitError{code: 1}
			}
			return nil
		},
	}
}

func newBundlesCreateCmd() *cobra.Command {
	var (
		dir   string
		force bool
	)
	cmd := &cobra.Command{
		Use:   "create <Name>",
		Short: "Scaffold a new bundle under the user bundles directory",
		Long: `Create a starter Recur Bundle: bundle.toml, a typescript sample, a shebang sample, and a test for each.

Default location: --bundles-dir / RECUR_BUNDLES_DIR / ~/.recur/bundles/<Name>.`,
		Example: `  recur bundles create MyPack
  recur bundles create MyPack --dir ./local-bundles`,
		Args: cobra.ExactArgs(1),
		RunE: func(cmd *cobra.Command, args []string) error {
			name := normalizeBundleName(args[0])
			destParent := dir
			if destParent == "" {
				var err error
				destParent, err = bundlesDir()
				if err != nil {
					return &exitError{code: 2, err: err}
				}
			}
			dest, err := createBundle(name, destParent, force)
			if err != nil {
				return &exitError{code: 1, err: err}
			}
			fmt.Fprintf(cmd.OutOrStdout(), "created %s\n", dest)
			fmt.Fprintf(cmd.OutOrStdout(), "see %s/README.md\n", dest)
			fmt.Fprintf(cmd.OutOrStdout(), "next: recur bundles test %s\n", name)
			return nil
		},
	}
	cmd.Flags().StringVar(&dir, "dir", "", "parent directory for the new bundle (default: bundles-dir)")
	cmd.Flags().BoolVar(&force, "force", false, "replace an existing bundle directory")
	return cmd
}
