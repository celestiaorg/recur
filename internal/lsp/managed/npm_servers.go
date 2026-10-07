package managed

import "context"

const (
	BasedPyrightRelease               = "1.39.9"
	YAMLLanguageServerRelease         = "1.24.0"
	BashLanguageServerRelease         = "5.6.0"
	SvelteLanguageServerRelease       = "0.18.3"
	VueLanguageServerRelease          = "3.3.9"
	VSCodeLangserversExtractedRelease = "4.10.0"
)

func ensureBasedPyright(ctx context.Context, dataDir string) (string, error) {
	return ensureNpmBin(ctx, dataDir, npmBinPin{
		ID:       "basedpyright",
		Version:  BasedPyrightRelease,
		EnvKey:   EnvBasedPyright,
		PathName: "basedpyright-langserver",
		BinName:  "basedpyright-langserver",
		Pkgs:     []string{"basedpyright@" + BasedPyrightRelease},
	})
}

func ensureYAMLLanguageServer(ctx context.Context, dataDir string) (string, error) {
	return ensureNpmBin(ctx, dataDir, npmBinPin{
		ID:       "yaml-language-server",
		Version:  YAMLLanguageServerRelease,
		EnvKey:   EnvYAML,
		PathName: "yaml-language-server",
		BinName:  "yaml-language-server",
		Pkgs:     []string{"yaml-language-server@" + YAMLLanguageServerRelease},
	})
}

func ensureBashLanguageServer(ctx context.Context, dataDir string) (string, error) {
	return ensureNpmBin(ctx, dataDir, npmBinPin{
		ID:       "bash-language-server",
		Version:  BashLanguageServerRelease,
		EnvKey:   EnvBash,
		PathName: "bash-language-server",
		BinName:  "bash-language-server",
		Pkgs:     []string{"bash-language-server@" + BashLanguageServerRelease},
	})
}

func ensureSvelteLanguageServer(ctx context.Context, dataDir string) (string, error) {
	return ensureNpmBin(ctx, dataDir, npmBinPin{
		ID:       "svelte-language-server",
		Version:  SvelteLanguageServerRelease,
		EnvKey:   EnvSvelte,
		PathName: "svelteserver",
		BinName:  "svelteserver",
		Pkgs:     []string{"svelte-language-server@" + SvelteLanguageServerRelease},
	})
}

func ensureVueLanguageServer(ctx context.Context, dataDir string) (string, error) {
	return ensureNpmBin(ctx, dataDir, npmBinPin{
		ID:       "vue-language-server",
		Version:  VueLanguageServerRelease,
		EnvKey:   EnvVue,
		PathName: "vue-language-server",
		BinName:  "vue-language-server",
		Pkgs:     []string{"@vue/language-server@" + VueLanguageServerRelease},
	})
}

func ensureVSCodeJSONLanguageServer(ctx context.Context, dataDir string) (string, error) {
	return ensureVSCodeLangserver(ctx, dataDir, "vscode-json-language-server", EnvJSON)
}

func ensureVSCodeHTMLLanguageServer(ctx context.Context, dataDir string) (string, error) {
	return ensureVSCodeLangserver(ctx, dataDir, "vscode-html-language-server", EnvHTML)
}

func ensureVSCodeCSSLanguageServer(ctx context.Context, dataDir string) (string, error) {
	return ensureVSCodeLangserver(ctx, dataDir, "vscode-css-language-server", EnvCSS)
}

func ensureVSCodeLangserver(ctx context.Context, dataDir, binName, envKey string) (string, error) {
	return ensureNpmBin(ctx, dataDir, npmBinPin{
		ID:       binName,
		Version:  VSCodeLangserversExtractedRelease,
		EnvKey:   envKey,
		PathName: binName,
		BinName:  binName,
		PrefixID: "vscode-langservers-extracted",
		Pkgs:     []string{"vscode-langservers-extracted@" + VSCodeLangserversExtractedRelease},
	})
}
