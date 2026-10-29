//! marked's `rules.ts`: block and inline grammars for the options Recur sets (`gfm: true`,
//! `breaks: true`), built with the same `edit()` source splicing so every regex is
//! byte-for-byte the one marked compiles.

use std::sync::OnceLock;

use crate::js::{Re, replace_first};

/// `edit(regex, opt)`: splice named sub-patterns into a source, then compile.
struct Edit {
    source: String,
    flags: &'static str,
}

fn caret() -> &'static Re {
    static CARET: OnceLock<Re> = OnceLock::new();
    CARET.get_or_init(|| Re::new(r"(^|[^\[])\^", "g"))
}

fn edit(source: &str, flags: &'static str) -> Edit {
    Edit {
        source: source.to_string(),
        flags,
    }
}

impl Edit {
    /// `.replace('name', val)`: the first occurrence.
    fn first(mut self, name: &str, val: &str) -> Edit {
        let val = caret().replace(val, "$1");
        self.source = replace_first(&self.source, name, &val);
        self
    }
    /// `.replace(/name/g, val)`: every occurrence.
    fn all(mut self, name: &str, val: &str) -> Edit {
        let val = caret().replace(val, "$1");
        self.source = self.source.replace(name, &val);
        self
    }
    fn source(&self) -> &str {
        &self.source
    }
    fn get(&self) -> Re {
        Re::new(&self.source, self.flags)
    }
}

/// `other`: helper regexes the tokenizer and renderer use.
pub struct Other {
    pub code_remove_indent: Re,
    pub output_link_replace: Re,
    pub indent_code_compensation: Re,
    pub beginning_space: Re,
    pub ending_hash: Re,
    pub starting_space_char: Re,
    pub ending_space_char: Re,
    pub non_space_char: Re,
    pub new_line_char_global: Re,
    pub tab_char_global: Re,
    pub multiple_space_global: Re,
    pub blank_line: Re,
    pub double_blank_line: Re,
    pub blockquote_start: Re,
    pub blockquote_setext_replace: Re,
    pub blockquote_setext_replace2: Re,
    pub list_is_task: Re,
    pub list_replace_task: Re,
    pub list_task_checkbox: Re,
    pub any_line: Re,
    pub href_brackets: Re,
    pub table_delimiter: Re,
    pub table_align_chars: Re,
    pub table_row_blank_line: Re,
    pub table_align_right: Re,
    pub table_align_center: Re,
    pub table_align_left: Re,
    pub start_a_tag: Re,
    pub end_a_tag: Re,
    pub start_pre_script_tag: Re,
    pub end_pre_script_tag: Re,
    pub start_angle_bracket: Re,
    pub end_angle_bracket: Re,
    pub unicode_alpha_numeric: Re,
    pub escape_test: Re,
    pub escape_replace: Re,
    pub escape_test_no_encode: Re,
    pub escape_replace_no_encode: Re,
    pub percent_decode: Re,
    pub find_pipe: Re,
    pub split_pipe: Re,
    pub slash_pipe: Re,
    pub carriage_return: Re,
    pub not_space_start: Re,
    pub ending_newline: Re,
    indent: OnceLock<[IndentRules; 4]>,
}

/// The `cachedIndentRegex` family for one cache slot.
pub struct IndentRules {
    pub next_bullet: Re,
    pub hr: Re,
    pub fences_begin: Re,
    pub heading_begin: Re,
    pub html_begin: Re,
    pub blockquote_begin: Re,
}

impl Other {
    /// `cachedIndentRegex`: the slot is `max(0, min(3, indent - 1))`, and that slot (not the
    /// indent) is what the regex is built from.
    pub fn indent(&self, indent: usize) -> &IndentRules {
        let slot = (indent as i64 - 1).clamp(0, 3) as usize;
        &self.indent.get_or_init(|| {
            std::array::from_fn(|i| IndentRules {
                next_bullet: Re::new(
                    &format!("^ {{0,{i}}}(?:[*+-]|\\d{{1,9}}[.)])((?:[ \t][^\\n]*)?(?:\\n|$))"),
                    "",
                ),
                hr: Re::new(
                    &format!("^ {{0,{i}}}((?:- *){{3,}}|(?:_ *){{3,}}|(?:\\* *){{3,}})(?:\\n+|$)"),
                    "",
                ),
                fences_begin: Re::new(&format!("^ {{0,{i}}}(?:```|~~~)"), ""),
                heading_begin: Re::new(&format!("^ {{0,{i}}}#"), ""),
                html_begin: Re::new(&format!("^ {{0,{i}}}<(?:[a-z].*>|!--)"), "i"),
                blockquote_begin: Re::new(&format!("^ {{0,{i}}}>"), ""),
            })
        })[slot]
    }

    /// `listItemRegex(bull)`.
    pub fn list_item(&self, bull: &str) -> Re {
        Re::new(
            &format!("^( {{0,3}}{bull})((?:[\t ][^\\n]*)?(?:\\n|$))"),
            "",
        )
    }
}

pub struct Block {
    pub newline: Re,
    pub code: Re,
    pub fences: Re,
    pub hr: Re,
    pub heading: Re,
    pub blockquote: Re,
    pub list: Re,
    pub html: Re,
    pub def: Re,
    pub table: Re,
    pub lheading: Re,
    pub paragraph: Re,
    pub text: Re,
}

pub struct Inline {
    pub escape: Re,
    pub tag: Re,
    pub link: Re,
    pub reflink: Re,
    pub nolink: Re,
    pub reflink_search: Re,
    pub em_strong_l_delim: Re,
    pub em_strong_r_delim_ast: Re,
    pub em_strong_r_delim_und: Re,
    pub code: Re,
    pub br: Re,
    pub del_l_delim: Re,
    pub del_r_delim: Re,
    pub autolink: Re,
    pub url: Re,
    pub backpedal: Re,
    pub text: Re,
    pub punctuation: Re,
    pub any_punctuation: Re,
    pub block_skip: Re,
}

pub struct Rules {
    pub other: Other,
    pub block: Block,
    pub inline: Inline,
}

/// The gfm + breaks grammar, compiled once.
pub fn rules() -> &'static Rules {
    static RULES: OnceLock<Rules> = OnceLock::new();
    RULES.get_or_init(build)
}

const TAG: &str = "address|article|aside|base|basefont|blockquote|body|caption\
|center|col|colgroup|dd|details|dialog|dir|div|dl|dt|fieldset|figcaption\
|figure|footer|form|frame|frameset|h[1-6]|head|header|hr|html|iframe\
|legend|li|link|main|menu|menuitem|meta|nav|noframes|ol|optgroup|option\
|p|param|search|section|summary|table|tbody|td|tfoot|th|thead|title\
|tr|track|ul";

fn build() -> Rules {
    let other = Other {
        code_remove_indent: Re::new(r"^(?: {1,4}| {0,3}\t)", "gm"),
        output_link_replace: Re::new(r"\\([\[\]])", "g"),
        indent_code_compensation: Re::new(r"^(\s+)(?:```)", ""),
        beginning_space: Re::new(r"^\s+", ""),
        ending_hash: Re::new(r"#$", ""),
        starting_space_char: Re::new(r"^ ", ""),
        ending_space_char: Re::new(r" $", ""),
        non_space_char: Re::new(r"[^ ]", ""),
        new_line_char_global: Re::new(r"\n", "g"),
        tab_char_global: Re::new(r"\t", "g"),
        multiple_space_global: Re::new(r"\s+", "g"),
        blank_line: Re::new(r"^[ \t]*$", ""),
        double_blank_line: Re::new(r"\n[ \t]*\n[ \t]*$", ""),
        blockquote_start: Re::new(r"^ {0,3}>", ""),
        blockquote_setext_replace: Re::new(r"\n {0,3}((?:=+|-+) *)(?=\n|$)", "g"),
        blockquote_setext_replace2: Re::new(r"^ {0,3}>[ \t]?", "gm"),
        list_is_task: Re::new(r"^\[[ xX]\] +\S", ""),
        list_replace_task: Re::new(r"^\[[ xX]\] +", ""),
        list_task_checkbox: Re::new(r"\[[ xX]\]", ""),
        any_line: Re::new(r"\n.*\n", ""),
        href_brackets: Re::new(r"^<(.*)>$", ""),
        table_delimiter: Re::new(r"[:|]", ""),
        table_align_chars: Re::new(r"^\||\| *$", "g"),
        table_row_blank_line: Re::new(r"\n[ \t]*$", ""),
        table_align_right: Re::new(r"^ *-+: *$", ""),
        table_align_center: Re::new(r"^ *:-+: *$", ""),
        table_align_left: Re::new(r"^ *:-+ *$", ""),
        start_a_tag: Re::new(r"^<a ", "i"),
        end_a_tag: Re::new(r"^</a>", "i"),
        start_pre_script_tag: Re::new(r"^<(pre|code|kbd|script)(\s|>)", "i"),
        end_pre_script_tag: Re::new(r"^</(pre|code|kbd|script)(\s|>)", "i"),
        start_angle_bracket: Re::new(r"^<", ""),
        end_angle_bracket: Re::new(r">$", ""),
        unicode_alpha_numeric: Re::new(r"[\p{L}\p{N}]", "u"),
        escape_test: Re::new(r#"[&<>"']"#, ""),
        escape_replace: Re::new(r#"[&<>"']"#, "g"),
        escape_test_no_encode: Re::new(r#"[<>"']|&(?!(#\d{1,7}|#[Xx][a-fA-F0-9]{1,6}|\w+);)"#, ""),
        escape_replace_no_encode: Re::new(
            r#"[<>"']|&(?!(#\d{1,7}|#[Xx][a-fA-F0-9]{1,6}|\w+);)"#,
            "g",
        ),
        percent_decode: Re::new(r"%25", "g"),
        find_pipe: Re::new(r"\|", "g"),
        split_pipe: Re::new(r" \|", ""),
        slash_pipe: Re::new(r"\\\|", "g"),
        carriage_return: Re::new(r"\r\n|\r", "g"),
        not_space_start: Re::new(r"^\S*", ""),
        ending_newline: Re::new(r"\n$", ""),
        indent: OnceLock::new(),
    };

    // Block grammar.
    let newline = r"^(?:[ \t]*(?:\n|$))+";
    let block_code = r"^((?: {4}| {0,3}\t)[^\n]+(?:\n(?:[ \t]*(?:\n|$))*)?)+";
    let fences = r"^ {0,3}(`{3,}(?=[^`\n]*(?:\n|$))|~{3,})([^\n]*)(?:\n|$)(?:|([\s\S]*?)(?:\n|$))(?: {0,3}\1[~`]* *(?=\n|$)|$)";
    let hr = r"^ {0,3}((?:-[\t ]*){3,}|(?:_[ \t]*){3,}|(?:\*[ \t]*){3,})(?:\n+|$)";
    let heading = r"^ {0,3}(#{1,6})(?=\s|$)(.*)(?:\n+|$)";
    let bullet = r" {0,3}(?:[*+-]|\d{1,9}[.)])";
    let lheading_core = r"^(?!bull |blockCode|fences|blockquote|heading|html|table)((?:.|\n(?!\s*?\n|bull |blockCode|fences|blockquote|heading|html|table))+?)\n {0,3}(=+|-+) *(?:\n+|$)";
    let lheading_gfm = edit(lheading_core, "")
        .all("bull", bullet)
        .all("blockCode", r"(?: {4}| {0,3}\t)")
        .all("fences", r" {0,3}(?:`{3,}|~{3,})")
        .all("blockquote", r" {0,3}>")
        .all("heading", r" {0,3}#{1,6}(?:\s|$)")
        .all("html", r" {0,3}<[^\n>]+>\n")
        .all("table", r" {0,3}\|?(?:[:\- ]*\|)+[\:\- ]*\n")
        .get();
    let paragraph_src =
        r"^([^\n]+(?:\n(?!hr|heading|lheading|blockquote|fences|list|html|table|[ \t]+\n)[^\n]+)*)";
    let block_text = r"^[^\n]+";
    let block_label = r"(?!\s*\])(?:\\[\s\S]|[^\[\]\\])+";
    let def = edit(
        r"^ {0,3}\[(label)\]: *(?:\n[ \t]*)?([^<\s][^\s]*|<.*?>)(?:(?: +(?:\n[ \t]*)?| *\n[ \t]*)(title))? *(?:\n+|$)",
        "",
    )
    .first("label", block_label)
    .first(
        "title",
        r#"(?:"(?:\\"?|[^"\\])*"|'[^'\n]*(?:\n[^'\n]+)*\n?'|\([^()]*\))"#,
    )
    .get();
    let list = edit(r"^(bull)([ \t][^\n]*?)?(?:\n|$)", "")
        .all("bull", bullet)
        .get();
    let comment = r"<!--(?:-?>|[\s\S]*?(?:-->|$))";
    let html = edit(
        &[
            r"^ {0,3}(?:",
            r"<(script|pre|style|textarea)[\s>][\s\S]*?(?:</\1>[^\n]*\n*|$)",
            r"|comment[^\n]*(\n+|$)",
            r"|<\?[\s\S]*?(?:\?>[^\n]*\n*|$)",
            r"|<![A-Z][\s\S]*?(?:>[^\n]*\n*|$)",
            r"|<!\[CDATA\[[\s\S]*?(?:\]\]>[^\n]*\n*|$)",
            "|</?(tag)(?: +|\\n|/?>)[\\s\\S]*?(?:(?:\\n[ \t]*)+\\n|$)",
            "|<(?!script|pre|style|textarea)([a-z][\\w-]*)(?:attribute)*? */?>(?=[ \\t]*(?:\\n|$))[\\s\\S]*?(?:(?:\\n[ \t]*)+\\n|$)",
            "|</(?!script|pre|style|textarea)[a-z][\\w-]*\\s*>(?=[ \\t]*(?:\\n|$))[\\s\\S]*?(?:(?:\\n[ \t]*)+\\n|$)",
            ")",
        ]
        .concat(),
        "i",
    )
    .first("comment", comment)
    .first("tag", TAG)
    .first(
        "attribute",
        r#" +[a-zA-Z:_][\w.:-]*(?: *= *"[^"\n]*"| *= *'[^'\n]*'| *= *[^\s"'=<>`]+)?"#,
    )
    .get();
    let hr_src = hr;
    let create_paragraph = |list_interrupt: &str| {
        edit(paragraph_src, "")
            .first("hr", hr_src)
            .first("heading", r" {0,3}#{1,6}(?:\s|$)")
            .first("|lheading", "")
            .first("|table", "")
            .first("blockquote", r" {0,3}>")
            .first("fences", " {0,3}(?:`{3,}(?=[^`\\n]*\\n)|~~~)[^\\n]*\\n")
            .first("list", list_interrupt)
            .first(
                "html",
                r"</?(?:tag)(?: +|\n|/?>)|<(?:script|pre|style|textarea|!--)",
            )
            .first("tag", TAG)
    };
    let blockquote_paragraph = create_paragraph(r" {0,3}(?:[*+-]|\d{1,9}[.)])(?:[ \t]|\n|$)");
    let blockquote = edit(r"^( {0,3}> ?(paragraph|[^\n]*)(?:\n|$))+", "")
        .first("paragraph", blockquote_paragraph.source())
        .get();
    let gfm_table = edit(
        &[
            r"^ *([^\n ].*)\n",
            r" {0,3}((?:\| *)?:?-+:? *(?:\| *:?-+:? *)*(?:\| *)?)",
            r"(?:\n((?:(?! *\n|hr|heading|blockquote|code|fences|list|html).*(?:\n|$))*)\n*|$)",
        ]
        .concat(),
        "",
    )
    .first("hr", hr)
    .first("heading", r" {0,3}#{1,6}(?:\s|$)")
    .first("blockquote", r" {0,3}>")
    .first("code", "(?: {4}| {0,3}\t)[^\\n]")
    .first("fences", " {0,3}(?:`{3,}(?=[^`\\n]*\\n)|~~~)[^\\n]*\\n")
    .first("list", r" {0,3}(?:[*+-]|1[.)])[ \t]")
    .first(
        "html",
        r"</?(?:tag)(?: +|\n|/?>)|<(?:script|pre|style|textarea|!--)",
    )
    .first("tag", TAG);
    let paragraph = edit(paragraph_src, "")
        .first("hr", hr)
        .first("heading", r" {0,3}#{1,6}(?:\s|$)")
        .first("|lheading", "")
        .first("table", gfm_table.source())
        .first("blockquote", r" {0,3}>")
        .first("fences", " {0,3}(?:`{3,}(?=[^`\\n]*\\n)|~~~)[^\\n]*\\n")
        .first("list", r" {0,3}(?:[*+-]|1[.)])[ \t]+[^ \t\n]")
        .first(
            "html",
            r"</?(?:tag)(?: +|\n|/?>)|<(?:script|pre|style|textarea|!--)",
        )
        .first("tag", TAG)
        .get();
    let block = Block {
        newline: Re::new(newline, ""),
        code: Re::new(block_code, ""),
        fences: Re::new(fences, ""),
        hr: Re::new(hr, ""),
        heading: Re::new(heading, ""),
        blockquote,
        list,
        html,
        def,
        table: gfm_table.get(),
        lheading: lheading_gfm,
        paragraph,
        text: Re::new(block_text, ""),
    };

    // Inline grammar.
    let punctuation_cls = r"[\p{P}\p{S}]";
    let punctuation_or_space = r"[\s\p{P}\p{S}]";
    let not_punctuation_or_space = r"[^\s\p{P}\p{S}]";
    let punctuation = edit(r"^((?![*_])punctSpace)", "u")
        .all("punctSpace", punctuation_or_space)
        .get();
    let punct_gfm = r"(?!~)[\p{P}\p{S}]";
    let punct_or_space_gfm = r"(?!~)[\s\p{P}\p{S}]";
    let not_punct_or_space_gfm = r"(?:[^\s\p{P}\p{S}]|~)";
    let block_skip = edit(r"link|precode-code|html", "g")
        .first(
            "link",
            r"\[(?:[^\[\]`]|(?<a>`+)[^`]+\k<a>(?!`))*?\]\((?:\\[\s\S]|[^\\\(\)]|\((?:\\[\s\S]|[^\\\(\)])*\))*\)",
        )
        .first("precode-", "(?<!`)()")
        .first("code", r"(?<b>`+)[^`]+\k<b>(?!`)")
        .first("html", r"<(?! )[^<>]*?>")
        .get();
    let em_strong_l_delim = edit(
        r"^(?:\*+(?:((?!\*)punct)|([^\s*]))?)|^_+(?:((?!_)punct)|([^\s_]))?",
        "u",
    )
    .all("punct", punct_gfm)
    .get();
    let em_strong_r_delim_ast_core = [
        r"^[^_*]*?__[^_*]*?\*[^_*]*?(?=__)",
        r"|[^*]+(?=[^*])",
        r"|(?!\*)punct(\*+)(?=[\s]|$)",
        r"|notPunctSpace(\*+)(?!\*)(?=punctSpace|$)",
        r"|(?!\*)punctSpace(\*+)(?=notPunctSpace)",
        r"|[\s](\*+)(?!\*)(?=punct)",
        r"|(?!\*)punct(\*+)(?!\*)(?=punct)",
        r"|notPunctSpace(\*+)(?=notPunctSpace)",
    ]
    .concat();
    let em_strong_r_delim_ast = edit(&em_strong_r_delim_ast_core, "gu")
        .all("notPunctSpace", not_punct_or_space_gfm)
        .all("punctSpace", punct_or_space_gfm)
        .all("punct", punct_gfm)
        .get();
    let em_strong_r_delim_und = edit(
        &[
            r"^[^_*]*?\*\*[^_*]*?_[^_*]*?(?=\*\*)",
            r"|[^_]+(?=[^_])",
            r"|(?!_)punct(_+)(?=[\s]|$)",
            r"|notPunctSpace(_+)(?!_)(?=punctSpace|$)",
            r"|(?!_)punctSpace(_+)(?=notPunctSpace)",
            r"|[\s](_+)(?!_)(?=punct)",
            r"|(?!_)punct(_+)(?!_)(?=punct)",
        ]
        .concat(),
        "gu",
    )
    .all("notPunctSpace", not_punctuation_or_space)
    .all("punctSpace", punctuation_or_space)
    .all("punct", punctuation_cls)
    .get();
    let del_l_delim = edit(r"^~~?(?:((?!~)punct)|[^\s~])", "u")
        .all("punct", punctuation_cls)
        .get();
    let del_r_delim = edit(
        &[
            r"^[^~]+(?=[^~])",
            r"|(?!~)punct(~~?)(?=[\s]|$)",
            r"|notPunctSpace(~~?)(?!~)(?=punctSpace|$)",
            r"|(?!~)punctSpace(~~?)(?=notPunctSpace)",
            r"|[\s](~~?)(?!~)(?=punct)",
            r"|(?!~)punct(~~?)(?!~)(?=punct)",
            r"|notPunctSpace(~~?)(?=notPunctSpace)",
        ]
        .concat(),
        "gu",
    )
    .all("notPunctSpace", not_punctuation_or_space)
    .all("punctSpace", punctuation_or_space)
    .all("punct", punctuation_cls)
    .get();
    let any_punctuation = edit(r"\\(punct)", "gu").all("punct", punctuation_cls).get();
    let autolink = edit(r"^<(scheme:[^\s\x00-\x1f<>]*|email)>", "")
        .first("scheme", r"[a-zA-Z][a-zA-Z0-9+.-]{1,31}")
        .first(
            "email",
            r"[a-zA-Z0-9.!#$%&'*+/=?^_`{|}~-]+(@)[a-zA-Z0-9](?:[a-zA-Z0-9-]{0,61}[a-zA-Z0-9])?(?:\.[a-zA-Z0-9](?:[a-zA-Z0-9-]{0,61}[a-zA-Z0-9])?)+(?![-_])",
        )
        .get();
    let inline_comment = edit(comment, "").first("(?:-->|$)", "-->");
    let tag = edit(
        &[
            r"^comment",
            r"|^</[a-zA-Z][\w:-]*\s*>",
            r"|^<[a-zA-Z][\w-]*(?:attribute)*?\s*/?>",
            r"|^<\?[\s\S]*?\?>",
            r"|^<![a-zA-Z]+\s[\s\S]*?>",
            r"|^<!\[CDATA\[[\s\S]*?\]\]>",
        ]
        .concat(),
        "",
    )
    .first("comment", inline_comment.source())
    .first(
        "attribute",
        r#"\s+[a-zA-Z:_][\w.:-]*(?:\s*=\s*"[^"]*"|\s*=\s*'[^']*'|\s*=\s*[^\s"'=<>`]+)?"#,
    )
    .get();
    let inline_label =
        r"(?:\[(?:\\[\s\S]|[^\[\]\\])*\]|\\[\s\S]|`+(?!`)[^`]*?`+(?!`)|``+(?=\])|[^\[\]\\`])*?";
    let link = edit(
        r"^!?\[(label)\]\(\s*(href)(?:(?:[ \t]+(?:\n[ \t]*)?|\n[ \t]*)(title))?\s*\)",
        "",
    )
    .first("label", inline_label)
    .first("href", r"<(?:\\.|[^\n<>\\])+>|[^ \t\n\x00-\x1f]+|(?=\))")
    .first(
        "title",
        r#""(?:\\"?|[^"\\])*"|'(?:\\'?|[^'\\])*'|\((?:\\\)?|[^)\\])*\)"#,
    )
    .get();
    let reflink = edit(r"^!?\[(label)\]\[(ref)\]", "")
        .first("label", inline_label)
        .first("ref", block_label);
    let nolink = edit(r"^!?\[(ref)\](?:\[\])?", "").first("ref", block_label);
    let reflink_search = edit(r"reflink|nolink(?!\()", "g")
        .first("reflink", reflink.source())
        .first("nolink", nolink.source())
        .get();
    let protocol = r"[hH][tT][tT][pP][sS]?|[fF][tT][pP]";
    let url = edit(
        r"^((?:protocol):\/\/|www\.)(?:[a-zA-Z0-9\-]+\.?)+[^\s<]*|^email",
        "",
    )
    .first("protocol", protocol)
    .first(
        "email",
        r"[A-Za-z0-9._+-]+(@)[a-zA-Z0-9-_]+(?:\.[a-zA-Z0-9-_]*[a-zA-Z0-9])+(?![-_])",
    )
    .get();
    let text_gfm = edit(
        r"^(`+|~+|[^`~])(?:(?=[`~])|(?= {2,}\n)|(?=[a-zA-Z0-9.!#$%&'*+\/=?_`{\|}~-]+@)|[\s\S]*?(?:(?=[\\<!\[`*~_]|\b_|protocol:\/\/|www\.|$)|[^ ](?= {2,}\n)|[^a-zA-Z0-9.!#$%&'*+\/=?_`{\|}~-](?=[a-zA-Z0-9.!#$%&'*+\/=?_`{\|}~-]+@)))",
        "",
    )
    .first("protocol", protocol);
    let text_breaks = edit(text_gfm.source(), "")
        .first(r"\b_", r"\b_| {2,}\n")
        .all("{2,}", "*")
        .get();
    let br_breaks = edit(r"^( {2,}|\\)\n(?!\s*$)", "").first("{2,}", "*").get();
    let inline = Inline {
        escape: Re::new(r##"^\\([!"#$%&'()*+,\-./:;<=>?@\[\]\\^_`{|}~])"##, ""),
        tag,
        link,
        reflink: reflink.get(),
        nolink: nolink.get(),
        reflink_search,
        em_strong_l_delim,
        em_strong_r_delim_ast,
        em_strong_r_delim_und,
        code: Re::new(r"^(`+)([^`]|[^`][\s\S]*?[^`])\1(?!`)", ""),
        br: br_breaks,
        del_l_delim,
        del_r_delim,
        autolink,
        url,
        backpedal: Re::new(
            r#"(?:[^?!.,:;*_'"~()&]+|\([^)]*\)|&(?![a-zA-Z0-9]+;$)|[?!.,:;*_'"~)]+(?!$))+"#,
            "",
        ),
        text: text_breaks,
        punctuation,
        any_punctuation,
        block_skip,
    };
    Rules {
        other,
        block,
        inline,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_rule_compiles() {
        let r = rules();
        assert!(r.block.paragraph.test("hello"));
        assert!(r.inline.text.test("abc"));
        let _ = r.other.indent(4);
    }

    #[test]
    fn caret_strip_matches_marked() {
        // marked's caret strip also removes the `^` inside the autolink email class.
        let r = rules();
        assert!(r.inline.autolink.test("<a@b.co>"));
        assert!(!r.inline.autolink.test("<a^b@c.co>"));
    }
}
