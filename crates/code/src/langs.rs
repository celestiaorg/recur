use recur_domain::path;

/// Reports whether `name` looks like a source file the outline, symbol,
/// metrics and drift walks read. Config and data files are excluded.
pub fn is_source_file(name: &str) -> bool {
    is_source_ext(&path::ext(name).to_lowercase())
}

/// Reports whether `ext` (`.py` or `py`) is a supported source extension.
pub fn is_source_ext(ext: &str) -> bool {
    if ext.is_empty() {
        return false;
    }
    let ext = if ext.starts_with('.') {
        ext.to_lowercase()
    } else {
        format!(".{}", ext.to_lowercase())
    };
    SOURCE_EXTS.contains(&ext.as_str())
}

/// Major application languages. Highlighting covers more; this list gates
/// symbol and metrics walks.
const SOURCE_EXTS: &[&str] = &[
    // Web / JS
    ".go", ".ts", ".tsx", ".js", ".jsx", ".mjs", ".cjs", ".svelte", ".vue", // Scripting
    ".py", ".rb", ".php", ".lua", ".pl", ".pm", ".r", ".ex", ".exs", ".erl", ".jl",
    // Systems
    ".rs", ".c", ".h", ".cc", ".cpp", ".cxx", ".hpp", ".hxx", ".hh", ".m", ".mm", ".zig",
    // JVM / .NET / Apple
    ".java", ".kt", ".kts", ".scala", ".groovy", ".cs", ".fs", ".swift",
    // Other app langs
    ".dart", ".hs", ".elm", ".clj", ".cljs", ".nim", ".ml", ".mli",
    // Markup that carries logic / styles counted in LOC
    ".css", ".scss", ".sass", ".less", ".sql", ".graphql", ".gql",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_exts() {
        assert!(is_source_file("a/B.GO"));
        assert!(is_source_ext("py"));
        assert!(is_source_ext(".Rs"));
        assert!(!is_source_ext(""));
        assert!(!is_source_file("config.json"));
        assert!(!is_source_file("Makefile"));
    }
}
