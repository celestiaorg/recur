package code

import (
	"path/filepath"
	"strings"
)

// IsSourceFile reports whether name looks like a source file we should walk for
// outline, symbols, metrics, and drift. Config/data files are excluded.
func IsSourceFile(name string) bool {
	return IsSourceExt(strings.ToLower(filepath.Ext(name)))
}

// IsSourceExt reports whether ext (e.g. ".py") is a supported source extension.
func IsSourceExt(ext string) bool {
	if ext == "" {
		return false
	}
	if ext[0] != '.' {
		ext = "." + ext
	}
	_, ok := sourceExts[strings.ToLower(ext)]
	return ok
}

// sourceExts covers major languages used in application codebases.
// Highlighting still uses Chroma (broader); this list gates symbol/metrics walks.
var sourceExts = map[string]bool{
	// Web / JS
	".go": true, ".ts": true, ".tsx": true, ".js": true, ".jsx": true,
	".mjs": true, ".cjs": true, ".svelte": true, ".vue": true,
	// Scripting
	".py": true, ".rb": true, ".php": true, ".lua": true, ".pl": true, ".pm": true,
	".r": true, ".ex": true, ".exs": true, ".erl": true, ".jl": true,
	// Systems
	".rs": true, ".c": true, ".h": true, ".cc": true, ".cpp": true, ".cxx": true,
	".hpp": true, ".hxx": true, ".hh": true, ".m": true, ".mm": true, ".zig": true,
	// JVM / .NET / Apple
	".java": true, ".kt": true, ".kts": true, ".scala": true, ".groovy": true,
	".cs": true, ".fs": true, ".swift": true,
	// Other app langs
	".dart": true, ".hs": true, ".elm": true, ".clj": true, ".cljs": true,
	".nim": true, ".ml": true, ".mli": true,
	// Markup that carries logic / styles counted in LOC
	".css": true, ".scss": true, ".sass": true, ".less": true,
	".sql": true, ".graphql": true, ".gql": true,
}
