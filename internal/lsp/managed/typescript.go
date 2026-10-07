package managed

import "context"

// TypeScriptLanguageServerRelease is the pinned npm version of
// typescript-language-server (typescript is installed alongside).
const TypeScriptLanguageServerRelease = "5.3.0"

// TypeScriptRelease is the pinned typescript package version (tsserver host).
const TypeScriptRelease = "5.9.2"

func ensureTypeScriptLanguageServer(ctx context.Context, dataDir string) (string, error) {
	return ensureNpmBin(ctx, dataDir, npmBinPin{
		ID:       "typescript-language-server",
		Version:  TypeScriptLanguageServerRelease,
		EnvKey:   EnvTypeScript,
		PathName: "typescript-language-server",
		BinName:  "typescript-language-server",
		Pkgs: []string{
			"typescript-language-server@" + TypeScriptLanguageServerRelease,
			"typescript@" + TypeScriptRelease,
		},
	})
}
