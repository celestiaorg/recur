//! Find over the scrollback with alacritty's regex search.

use alacritty_terminal::term::search::RegexSearch;

use super::theme::Rgba;

/// The terminal dock's `highlightLimit` (`PREVIEW_FIND_CAP`).
pub const HIGHLIGHT_LIMIT: usize = 999;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Decorations {
    pub match_background: Rgba,
    pub active_match_background: Rgba,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SearchOptions {
    pub case_sensitive: bool,
    pub regex: bool,
    pub whole_word: bool,
    /// Typing into the find field: keep the current match while it still matches.
    pub incremental: bool,
    pub decorations: Decorations,
}

/// `index` is 1-based, 0 when no match is selected. `count` stops at [`HIGHLIGHT_LIMIT`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SearchResults {
    pub index: usize,
    pub count: usize,
}

#[derive(Debug, thiserror::Error)]
#[error("invalid regular expression: {0}")]
pub struct InvalidRegex(String);

pub(crate) struct Search {
    pub(crate) query: String,
    pub(crate) opts: SearchOptions,
    pub(crate) regex: RegexSearch,
}

impl Search {
    pub(crate) fn new(query: &str, opts: &SearchOptions) -> Result<Self, InvalidRegex> {
        let mut pattern = if opts.regex {
            query.to_string()
        } else {
            regex::escape(query)
        };
        if opts.whole_word {
            pattern = format!(r"(?-u:\b)(?:{pattern})(?-u:\b)");
        }
        let case = if opts.case_sensitive { "(?-i)" } else { "(?i)" };
        let regex = RegexSearch::new(&format!("{case}{pattern}"))
            .map_err(|e| InvalidRegex(e.to_string()))?;
        Ok(Self {
            query: query.to_string(),
            opts: opts.clone(),
            regex,
        })
    }
}
