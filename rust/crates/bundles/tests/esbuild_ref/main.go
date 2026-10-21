// Command esbuild_ref compiles one macro exactly like Go internal/bundles
// compile.go (same esbuild version, options, wrappers and @recur/sdk
// plugin). The recur-bundles compile parity test runs its output next to
// the swc output in node to prove identical behavior.
//
//	esbuild_ref <command|prompt|complete> <id> <macro_sdk.ts> < source.ts
package main

import (
	"fmt"
	"io"
	"os"
	"strings"

	"github.com/evanw/esbuild/pkg/api"
)

const sdkModule = "@recur/sdk"

func main() {
	if len(os.Args) != 4 {
		fmt.Fprintln(os.Stderr, "usage: esbuild_ref <command|prompt|complete> <id> <macro_sdk.ts>")
		os.Exit(2)
	}
	kind, id, sdkPath := os.Args[1], os.Args[2], os.Args[3]
	sdk, err := os.ReadFile(sdkPath)
	if err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(2)
	}
	body, err := io.ReadAll(os.Stdin)
	if err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(2)
	}
	src := string(body)
	switch kind {
	case "command":
		src = wrap(src, "Output, replaceEdited, type MacroContext, type MacroResult", "MacroContext", "MacroResult")
	case "prompt":
		src = wrap(src, "type PromptContext, type PromptResult", "PromptContext", "PromptResult")
	case "complete":
		src = wrap(src, "type PromptCompleteContext, type PromptCompleteResult", "PromptCompleteContext", "PromptCompleteResult")
		id += "-complete"
	default:
		fmt.Fprintln(os.Stderr, "unknown kind", kind)
		os.Exit(2)
	}
	result := api.Build(api.BuildOptions{
		Stdin: &api.StdinOptions{
			Contents:   src,
			Loader:     api.LoaderTS,
			ResolveDir: "/",
			Sourcefile: id + ".ts",
		},
		Bundle:   true,
		Format:   api.FormatCommonJS,
		Target:   api.ES2020,
		Platform: api.PlatformNeutral,
		LogLevel: api.LogLevelSilent,
		Charset:  api.CharsetUTF8,
		Write:    false,
		Plugins:  []api.Plugin{sdkPlugin(string(sdk))},
	})
	if len(result.Errors) > 0 {
		parts := make([]string, 0, len(result.Errors))
		for _, m := range result.Errors {
			if m.Location != nil {
				parts = append(parts, fmt.Sprintf("%s:%d:%d: %s", m.Location.File, m.Location.Line, m.Location.Column, m.Text))
			} else {
				parts = append(parts, m.Text)
			}
		}
		fmt.Fprint(os.Stderr, strings.Join(parts, "; "))
		os.Exit(1)
	}
	os.Stdout.Write(result.OutputFiles[0].Contents)
}

func wrap(src, imports, ctxType, resultType string) string {
	trim := strings.TrimSpace(src)
	if strings.Contains(trim, "export default") || strings.Contains(trim, "module.exports") {
		return src
	}
	return "import { " + imports + " } from \"" + sdkModule + "\"\n" +
		"export default async function (ctx: " + ctxType + "): Promise<" + resultType + "> {\n" + src + "\n}\n"
}

func sdkPlugin(sdk string) api.Plugin {
	const ns = "recur-sdk"
	return api.Plugin{
		Name: "recur-sdk",
		Setup: func(build api.PluginBuild) {
			build.OnResolve(api.OnResolveOptions{Filter: `^@recur/sdk$`}, func(args api.OnResolveArgs) (api.OnResolveResult, error) {
				return api.OnResolveResult{Path: sdkModule, Namespace: ns}, nil
			})
			build.OnResolve(api.OnResolveOptions{Filter: `.*`}, func(args api.OnResolveArgs) (api.OnResolveResult, error) {
				if args.Path == sdkModule || args.Kind == api.ResolveEntryPoint {
					return api.OnResolveResult{}, nil
				}
				return api.OnResolveResult{}, fmt.Errorf("import %q is not allowed (only %q)", args.Path, sdkModule)
			})
			build.OnLoad(api.OnLoadOptions{Filter: `.*`, Namespace: ns}, func(args api.OnLoadArgs) (api.OnLoadResult, error) {
				contents := sdk
				return api.OnLoadResult{Contents: &contents, Loader: api.LoaderTS}, nil
			})
		},
	}
}
