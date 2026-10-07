package bundles

import (
	"crypto/sha256"
	_ "embed"
	"encoding/hex"
	"fmt"
	"os"
	"path/filepath"
	"strings"

	"github.com/evanw/esbuild/pkg/api"
)

// MacroSDKVersion bumps when the SDK or Worker contract changes so cached JS
// is invalidated (macro_sdk.ts, frontend/src/lib/macro-worker.ts).
const MacroSDKVersion = "8"

// MacroSDKModule is the import path authors use in typescript commands.
const MacroSDKModule = "@recur/sdk"

// MacroSDKSource is the `@recur/sdk` module body (esbuild virtual module).
// Author-facing copy: bundles/sdk/index.ts (keep in sync; see compile_test).
//
//go:embed macro_sdk.ts
var MacroSDKSource string

// IsScriptInterpreter reports whether cmd runs as compiled TypeScript in the
// frontend Worker rather than as a shebang subprocess.
func IsScriptInterpreter(interp string) bool {
	return interp == InterpreterTypeScript
}

// CompileResult is esbuild output ready for the macro Worker.
type CompileResult struct {
	JS       string // CommonJS source with default export
	CacheHit bool
	Hash     string
}

// CompileMacro transforms a typescript command body to CommonJS for the Worker.
// Results are cached under cacheDir as <uuid>.<hash>.js.
//
// Authors import from `@recur/sdk` (MacroContext, Output, replaceEdited, …);
// esbuild bundles that virtual module into the output.
func CompileMacro(cacheDir string, cmd Command) (CompileResult, error) {
	if !IsScriptInterpreter(cmd.Interpreter) {
		return CompileResult{}, fmt.Errorf("command %q: interpreter %q is not typescript", cmd.ID, cmd.Interpreter)
	}
	src := cmd.Body
	if strings.TrimSpace(src) == "" {
		return CompileResult{}, fmt.Errorf("command %q: empty typescript source", cmd.ID)
	}
	src = ensureDefaultExport(src)

	sum := sha256.Sum256([]byte(MacroSDKVersion + "\x00" + cmd.Interpreter + "\x00" + MacroSDKSource + "\x00" + src))
	hash := hex.EncodeToString(sum[:16])
	cachePath := filepath.Join(cacheDir, cmd.ID+"."+hash+".js")
	if data, err := os.ReadFile(cachePath); err == nil && len(data) > 0 {
		return CompileResult{JS: string(data), CacheHit: true, Hash: hash}, nil
	}

	// Bundle so `@recur/sdk` resolves; CommonJS for classic Worker + AsyncFunction.
	result := api.Build(api.BuildOptions{
		Stdin: &api.StdinOptions{
			Contents:   src,
			Loader:     api.LoaderTS,
			ResolveDir: "/",
			Sourcefile: cmd.ID + ".ts",
		},
		Bundle:   true,
		Format:   api.FormatCommonJS,
		Target:   api.ES2020,
		Platform: api.PlatformNeutral,
		LogLevel: api.LogLevelSilent,
		Charset:  api.CharsetUTF8,
		Write:    false,
		Plugins:  []api.Plugin{recurSDKPlugin()},
	})
	if len(result.Errors) > 0 {
		return CompileResult{}, fmt.Errorf("compile %q: %s", cmd.ID, formatESBuildMessages(result.Errors))
	}
	if len(result.OutputFiles) == 0 {
		return CompileResult{}, fmt.Errorf("compile %q: empty output", cmd.ID)
	}
	js := string(result.OutputFiles[0].Contents)
	if strings.TrimSpace(js) == "" {
		return CompileResult{}, fmt.Errorf("compile %q: empty output", cmd.ID)
	}
	if err := os.MkdirAll(cacheDir, 0o755); err != nil {
		return CompileResult{}, err
	}
	_ = os.WriteFile(cachePath, []byte(js), 0o644)
	pruneOldCache(cacheDir, cmd.ID, hash)
	return CompileResult{JS: js, CacheHit: false, Hash: hash}, nil
}

// recurSDKPlugin resolves `@recur/sdk` to the embedded module and rejects any
// other import so macros stay sandboxed to the SDK surface.
func recurSDKPlugin() api.Plugin {
	const ns = "recur-sdk"
	sdk := MacroSDKSource
	return api.Plugin{
		Name: "recur-sdk",
		Setup: func(build api.PluginBuild) {
			build.OnResolve(api.OnResolveOptions{Filter: `^@recur/sdk$`}, func(args api.OnResolveArgs) (api.OnResolveResult, error) {
				return api.OnResolveResult{Path: MacroSDKModule, Namespace: ns}, nil
			})
			// Deny every other resolve (node builtins, relative files, npm, …).
			build.OnResolve(api.OnResolveOptions{Filter: `.*`}, func(args api.OnResolveArgs) (api.OnResolveResult, error) {
				if args.Path == MacroSDKModule || args.Kind == api.ResolveEntryPoint {
					return api.OnResolveResult{}, nil
				}
				return api.OnResolveResult{}, fmt.Errorf("import %q is not allowed (only %q)", args.Path, MacroSDKModule)
			})
			build.OnLoad(api.OnLoadOptions{Filter: `.*`, Namespace: ns}, func(args api.OnLoadArgs) (api.OnLoadResult, error) {
				contents := sdk
				return api.OnLoadResult{
					Contents: &contents,
					Loader:   api.LoaderTS,
				}, nil
			})
		},
	}
}

// CompilePromptMacro transforms a typescript prompt-action body to CommonJS
// for the frontend Worker. Same cache layout as CompileMacro.
func CompilePromptMacro(cacheDir string, prompt PromptAction) (CompileResult, error) {
	if !IsScriptInterpreter(prompt.Interpreter) {
		return CompileResult{}, fmt.Errorf("prompt %q: interpreter %q is not typescript", prompt.ID, prompt.Interpreter)
	}
	src := prompt.Body
	if strings.TrimSpace(src) == "" {
		return CompileResult{}, fmt.Errorf("prompt %q: empty typescript source", prompt.ID)
	}
	src = ensurePromptDefaultExport(src)

	sum := sha256.Sum256([]byte(MacroSDKVersion + "\x00" + "prompt" + "\x00" + prompt.Interpreter + "\x00" + MacroSDKSource + "\x00" + src))
	hash := hex.EncodeToString(sum[:16])
	cachePath := filepath.Join(cacheDir, prompt.ID+"."+hash+".js")
	if data, err := os.ReadFile(cachePath); err == nil && len(data) > 0 {
		return CompileResult{JS: string(data), CacheHit: true, Hash: hash}, nil
	}

	result := api.Build(api.BuildOptions{
		Stdin: &api.StdinOptions{
			Contents:   src,
			Loader:     api.LoaderTS,
			ResolveDir: "/",
			Sourcefile: prompt.ID + ".ts",
		},
		Bundle:   true,
		Format:   api.FormatCommonJS,
		Target:   api.ES2020,
		Platform: api.PlatformNeutral,
		LogLevel: api.LogLevelSilent,
		Charset:  api.CharsetUTF8,
		Write:    false,
		Plugins:  []api.Plugin{recurSDKPlugin()},
	})
	if len(result.Errors) > 0 {
		return CompileResult{}, fmt.Errorf("compile prompt %q: %s", prompt.ID, formatESBuildMessages(result.Errors))
	}
	if len(result.OutputFiles) == 0 {
		return CompileResult{}, fmt.Errorf("compile prompt %q: empty output", prompt.ID)
	}
	js := string(result.OutputFiles[0].Contents)
	if strings.TrimSpace(js) == "" {
		return CompileResult{}, fmt.Errorf("compile prompt %q: empty output", prompt.ID)
	}
	if err := os.MkdirAll(cacheDir, 0o755); err != nil {
		return CompileResult{}, err
	}
	_ = os.WriteFile(cachePath, []byte(js), 0o644)
	pruneOldCache(cacheDir, prompt.ID, hash)
	return CompileResult{JS: js, CacheHit: false, Hash: hash}, nil
}

// CompileCompleteMacro compiles a typescript [[prompt]] complete script.
func CompileCompleteMacro(cacheDir string, prompt PromptAction) (CompileResult, error) {
	if prompt.CompleteInterpreter != InterpreterTypeScript {
		return CompileResult{}, fmt.Errorf("prompt %q: complete interpreter %q is not typescript", prompt.ID, prompt.CompleteInterpreter)
	}
	src := prompt.CompleteBody
	if strings.TrimSpace(src) == "" {
		return CompileResult{}, fmt.Errorf("prompt %q: empty complete source", prompt.ID)
	}
	src = ensureCompleteDefaultExport(src)
	id := prompt.ID + "-complete"
	sum := sha256.Sum256([]byte(MacroSDKVersion + "\x00" + "prompt-complete" + "\x00" + MacroSDKSource + "\x00" + src))
	hash := hex.EncodeToString(sum[:16])
	cachePath := filepath.Join(cacheDir, id+"."+hash+".js")
	if data, err := os.ReadFile(cachePath); err == nil && len(data) > 0 {
		return CompileResult{JS: string(data), CacheHit: true, Hash: hash}, nil
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
		Plugins:  []api.Plugin{recurSDKPlugin()},
	})
	if len(result.Errors) > 0 {
		return CompileResult{}, fmt.Errorf("compile complete %q: %s", prompt.ID, formatESBuildMessages(result.Errors))
	}
	if len(result.OutputFiles) == 0 {
		return CompileResult{}, fmt.Errorf("compile complete %q: empty output", prompt.ID)
	}
	js := string(result.OutputFiles[0].Contents)
	if strings.TrimSpace(js) == "" {
		return CompileResult{}, fmt.Errorf("compile complete %q: empty output", prompt.ID)
	}
	if err := os.MkdirAll(cacheDir, 0o755); err != nil {
		return CompileResult{}, err
	}
	_ = os.WriteFile(cachePath, []byte(js), 0o644)
	pruneOldCache(cacheDir, id, hash)
	return CompileResult{JS: js, CacheHit: false, Hash: hash}, nil
}

// ensureDefaultExport wraps bare bodies so authors can write either a full
// `export default function…` module or a concise statement body. Bare bodies
// get a default `@recur/sdk` import for replaceEdited / Output / types.
func ensureDefaultExport(src string) string {
	trim := strings.TrimSpace(src)
	if strings.Contains(trim, "export default") || strings.Contains(trim, "module.exports") {
		return src
	}
	return "import { Output, replaceEdited, type MacroContext, type MacroResult } from \"" + MacroSDKModule + "\"\n" +
		"export default async function (ctx: MacroContext): Promise<MacroResult> {\n" + src + "\n}\n"
}

// ensurePromptDefaultExport wraps bare prompt bodies with a PromptContext default export.
func ensurePromptDefaultExport(src string) string {
	trim := strings.TrimSpace(src)
	if strings.Contains(trim, "export default") || strings.Contains(trim, "module.exports") {
		return src
	}
	return "import { type PromptContext, type PromptResult } from \"" + MacroSDKModule + "\"\n" +
		"export default async function (ctx: PromptContext): Promise<PromptResult> {\n" + src + "\n}\n"
}

func ensureCompleteDefaultExport(src string) string {
	trim := strings.TrimSpace(src)
	if strings.Contains(trim, "export default") || strings.Contains(trim, "module.exports") {
		return src
	}
	return "import { type PromptCompleteContext, type PromptCompleteResult } from \"" + MacroSDKModule + "\"\n" +
		"export default async function (ctx: PromptCompleteContext): Promise<PromptCompleteResult> {\n" + src + "\n}\n"
}

func formatESBuildMessages(msgs []api.Message) string {
	parts := make([]string, 0, len(msgs))
	for _, m := range msgs {
		if m.Location != nil {
			parts = append(parts, fmt.Sprintf("%s:%d:%d: %s", m.Location.File, m.Location.Line, m.Location.Column, m.Text))
		} else {
			parts = append(parts, m.Text)
		}
	}
	return strings.Join(parts, "; ")
}

func pruneOldCache(cacheDir, uuid, keepHash string) {
	entries, err := os.ReadDir(cacheDir)
	if err != nil {
		return
	}
	prefix := uuid + "."
	keep := uuid + "." + keepHash + ".js"
	for _, e := range entries {
		name := e.Name()
		if strings.HasPrefix(name, prefix) && strings.HasSuffix(name, ".js") && name != keep {
			_ = os.Remove(filepath.Join(cacheDir, name))
		}
	}
}
