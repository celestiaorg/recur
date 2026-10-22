//! `lib/autosize-textarea.ts`: a textarea grows with its text, then scrolls.

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AutosizeOpts {
    pub min_px: f32,
    pub max_px: f32,
}

/// The main PR conversation composer: a bit taller than inline note replies.
pub const PR_COMPOSER_AUTOSIZE: AutosizeOpts = AutosizeOpts {
    min_px: 72.,
    max_px: 160.,
};

/// A line-note reply: compact, grows through ~6 lines.
pub const PR_REPLY_AUTOSIZE: AutosizeOpts = AutosizeOpts {
    min_px: 40.,
    max_px: 120.,
};

/// `fitTextareaHeight`: the height for a content (scroll) height, and whether it scrolls.
pub fn fit_textarea_height(content: f32, opts: AutosizeOpts) -> (f32, bool) {
    (
        content.max(opts.min_px).min(opts.max_px),
        content > opts.max_px,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grows_up_to_max_then_scrolls() {
        let o = AutosizeOpts {
            min_px: 64.,
            max_px: 136.,
        };
        assert_eq!(fit_textarea_height(200., o), (136., true));
        assert_eq!(fit_textarea_height(90., o), (90., false));
        assert_eq!(fit_textarea_height(20., o), (64., false));
    }

    #[test]
    fn grows_with_content_then_scrolls() {
        assert_eq!(fit_textarea_height(10., PR_REPLY_AUTOSIZE), (40., false));
        assert_eq!(fit_textarea_height(80., PR_REPLY_AUTOSIZE), (80., false));
        assert_eq!(fit_textarea_height(300., PR_REPLY_AUTOSIZE), (120., true));
        assert_eq!(
            fit_textarea_height(100., PR_COMPOSER_AUTOSIZE),
            (100., false)
        );
    }
}
