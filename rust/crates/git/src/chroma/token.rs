//! Chroma token types: categories in thousands, sub-categories in hundreds.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct TokenType(pub i32);

impl TokenType {
    pub(crate) const IGNORE: TokenType = TokenType(-14);
    pub(crate) const NONE: TokenType = TokenType(-13);
    pub(crate) const OTHER: TokenType = TokenType(-12);
    pub(crate) const ERROR: TokenType = TokenType(-11);
    pub(crate) const CODE_LINE: TokenType = TokenType(-10);
    pub(crate) const LINE_LINK: TokenType = TokenType(-9);
    pub(crate) const LINE_TABLE_T_D: TokenType = TokenType(-8);
    pub(crate) const LINE_TABLE: TokenType = TokenType(-7);
    pub(crate) const LINE_HIGHLIGHT: TokenType = TokenType(-6);
    pub(crate) const LINE_NUMBERS_TABLE: TokenType = TokenType(-5);
    pub(crate) const LINE_NUMBERS: TokenType = TokenType(-4);
    pub(crate) const LINE: TokenType = TokenType(-3);
    pub(crate) const PRE_WRAPPER: TokenType = TokenType(-2);
    pub(crate) const BACKGROUND: TokenType = TokenType(-1);
    pub(crate) const KEYWORD: TokenType = TokenType(1000);
    pub(crate) const KEYWORD_CONSTANT: TokenType = TokenType(1001);
    pub(crate) const KEYWORD_DECLARATION: TokenType = TokenType(1002);
    pub(crate) const KEYWORD_NAMESPACE: TokenType = TokenType(1003);
    pub(crate) const KEYWORD_PSEUDO: TokenType = TokenType(1004);
    pub(crate) const KEYWORD_RESERVED: TokenType = TokenType(1005);
    pub(crate) const KEYWORD_TYPE: TokenType = TokenType(1006);
    pub(crate) const NAME: TokenType = TokenType(2000);
    pub(crate) const NAME_ATTRIBUTE: TokenType = TokenType(2001);
    pub(crate) const NAME_CLASS: TokenType = TokenType(2002);
    pub(crate) const NAME_CONSTANT: TokenType = TokenType(2003);
    pub(crate) const NAME_DECORATOR: TokenType = TokenType(2004);
    pub(crate) const NAME_ENTITY: TokenType = TokenType(2005);
    pub(crate) const NAME_EXCEPTION: TokenType = TokenType(2006);
    pub(crate) const NAME_KEYWORD: TokenType = TokenType(2007);
    pub(crate) const NAME_LABEL: TokenType = TokenType(2008);
    pub(crate) const NAME_NAMESPACE: TokenType = TokenType(2009);
    pub(crate) const NAME_OPERATOR: TokenType = TokenType(2010);
    pub(crate) const NAME_OTHER: TokenType = TokenType(2011);
    pub(crate) const NAME_PSEUDO: TokenType = TokenType(2012);
    pub(crate) const NAME_PROPERTY: TokenType = TokenType(2013);
    pub(crate) const NAME_TAG: TokenType = TokenType(2014);
    pub(crate) const NAME_BUILTIN: TokenType = TokenType(2100);
    pub(crate) const NAME_BUILTIN_PSEUDO: TokenType = TokenType(2101);
    pub(crate) const NAME_VARIABLE: TokenType = TokenType(2200);
    pub(crate) const NAME_VARIABLE_ANONYMOUS: TokenType = TokenType(2201);
    pub(crate) const NAME_VARIABLE_CLASS: TokenType = TokenType(2202);
    pub(crate) const NAME_VARIABLE_GLOBAL: TokenType = TokenType(2203);
    pub(crate) const NAME_VARIABLE_INSTANCE: TokenType = TokenType(2204);
    pub(crate) const NAME_VARIABLE_MAGIC: TokenType = TokenType(2205);
    pub(crate) const NAME_FUNCTION: TokenType = TokenType(2300);
    pub(crate) const NAME_FUNCTION_MAGIC: TokenType = TokenType(2301);
    pub(crate) const LITERAL: TokenType = TokenType(3000);
    pub(crate) const LITERAL_DATE: TokenType = TokenType(3001);
    pub(crate) const LITERAL_OTHER: TokenType = TokenType(3002);
    pub(crate) const LITERAL_STRING: TokenType = TokenType(3100);
    pub(crate) const LITERAL_STRING_AFFIX: TokenType = TokenType(3101);
    pub(crate) const LITERAL_STRING_ATOM: TokenType = TokenType(3102);
    pub(crate) const LITERAL_STRING_BACKTICK: TokenType = TokenType(3103);
    pub(crate) const LITERAL_STRING_BOOLEAN: TokenType = TokenType(3104);
    pub(crate) const LITERAL_STRING_CHAR: TokenType = TokenType(3105);
    pub(crate) const LITERAL_STRING_DELIMITER: TokenType = TokenType(3106);
    pub(crate) const LITERAL_STRING_DOC: TokenType = TokenType(3107);
    pub(crate) const LITERAL_STRING_DOUBLE: TokenType = TokenType(3108);
    pub(crate) const LITERAL_STRING_ESCAPE: TokenType = TokenType(3109);
    pub(crate) const LITERAL_STRING_HEREDOC: TokenType = TokenType(3110);
    pub(crate) const LITERAL_STRING_INTERPOL: TokenType = TokenType(3111);
    pub(crate) const LITERAL_STRING_NAME: TokenType = TokenType(3112);
    pub(crate) const LITERAL_STRING_OTHER: TokenType = TokenType(3113);
    pub(crate) const LITERAL_STRING_REGEX: TokenType = TokenType(3114);
    pub(crate) const LITERAL_STRING_SINGLE: TokenType = TokenType(3115);
    pub(crate) const LITERAL_STRING_SYMBOL: TokenType = TokenType(3116);
    pub(crate) const LITERAL_NUMBER: TokenType = TokenType(3200);
    pub(crate) const LITERAL_NUMBER_BIN: TokenType = TokenType(3201);
    pub(crate) const LITERAL_NUMBER_FLOAT: TokenType = TokenType(3202);
    pub(crate) const LITERAL_NUMBER_HEX: TokenType = TokenType(3203);
    pub(crate) const LITERAL_NUMBER_INTEGER: TokenType = TokenType(3204);
    pub(crate) const LITERAL_NUMBER_INTEGER_LONG: TokenType = TokenType(3205);
    pub(crate) const LITERAL_NUMBER_OCT: TokenType = TokenType(3206);
    pub(crate) const LITERAL_NUMBER_BYTE: TokenType = TokenType(3207);
    pub(crate) const OPERATOR: TokenType = TokenType(4000);
    pub(crate) const OPERATOR_WORD: TokenType = TokenType(4001);
    pub(crate) const OPERATOR_RESERVED: TokenType = TokenType(4002);
    pub(crate) const PUNCTUATION: TokenType = TokenType(5000);
    pub(crate) const COMMENT: TokenType = TokenType(6000);
    pub(crate) const COMMENT_HASHBANG: TokenType = TokenType(6001);
    pub(crate) const COMMENT_MULTILINE: TokenType = TokenType(6002);
    pub(crate) const COMMENT_SINGLE: TokenType = TokenType(6003);
    pub(crate) const COMMENT_SPECIAL: TokenType = TokenType(6004);
    pub(crate) const COMMENT_PREPROC: TokenType = TokenType(6100);
    pub(crate) const COMMENT_PREPROC_FILE: TokenType = TokenType(6101);
    pub(crate) const GENERIC: TokenType = TokenType(7000);
    pub(crate) const GENERIC_DELETED: TokenType = TokenType(7001);
    pub(crate) const GENERIC_EMPH: TokenType = TokenType(7002);
    pub(crate) const GENERIC_ERROR: TokenType = TokenType(7003);
    pub(crate) const GENERIC_HEADING: TokenType = TokenType(7004);
    pub(crate) const GENERIC_INSERTED: TokenType = TokenType(7005);
    pub(crate) const GENERIC_OUTPUT: TokenType = TokenType(7006);
    pub(crate) const GENERIC_PROMPT: TokenType = TokenType(7007);
    pub(crate) const GENERIC_STRONG: TokenType = TokenType(7008);
    pub(crate) const GENERIC_SUBHEADING: TokenType = TokenType(7009);
    pub(crate) const GENERIC_TRACEBACK: TokenType = TokenType(7010);
    pub(crate) const GENERIC_UNDERLINE: TokenType = TokenType(7011);
    pub(crate) const TEXT: TokenType = TokenType(8000);
    pub(crate) const TEXT_WHITESPACE: TokenType = TokenType(8001);
    pub(crate) const TEXT_SYMBOL: TokenType = TokenType(8002);
    pub(crate) const TEXT_PUNCTUATION: TokenType = TokenType(8003);

    /// Same category (Go `InCategory`): equal thousands.
    pub(crate) fn in_category(self, other: TokenType) -> bool {
        self.0 / 1000 == other.0 / 1000
    }

    /// The chroma name (`LiteralStringDouble`).
    pub(crate) fn name(self) -> &'static str {
        NAMES
            .iter()
            .find(|(v, _)| *v == self.0)
            .map_or("", |(_, n)| n)
    }

    /// Go `TokenTypeString`: exact name, then case-insensitive.
    pub(crate) fn from_name(s: &str) -> Option<TokenType> {
        NAMES
            .iter()
            .find(|(_, n)| *n == s)
            .or_else(|| NAMES.iter().find(|(_, n)| n.eq_ignore_ascii_case(s)))
            .map(|(v, _)| TokenType(*v))
    }
}

/// Every named token type (Go `TokenTypeValues`).
static NAMES: &[(i32, &str)] = &[
    (-14, "Ignore"),
    (-13, "None"),
    (-12, "Other"),
    (-11, "Error"),
    (-10, "CodeLine"),
    (-9, "LineLink"),
    (-8, "LineTableTD"),
    (-7, "LineTable"),
    (-6, "LineHighlight"),
    (-5, "LineNumbersTable"),
    (-4, "LineNumbers"),
    (-3, "Line"),
    (-2, "PreWrapper"),
    (-1, "Background"),
    (0, "EOFType"),
    (1000, "Keyword"),
    (1001, "KeywordConstant"),
    (1002, "KeywordDeclaration"),
    (1003, "KeywordNamespace"),
    (1004, "KeywordPseudo"),
    (1005, "KeywordReserved"),
    (1006, "KeywordType"),
    (2000, "Name"),
    (2001, "NameAttribute"),
    (2002, "NameClass"),
    (2003, "NameConstant"),
    (2004, "NameDecorator"),
    (2005, "NameEntity"),
    (2006, "NameException"),
    (2007, "NameKeyword"),
    (2008, "NameLabel"),
    (2009, "NameNamespace"),
    (2010, "NameOperator"),
    (2011, "NameOther"),
    (2012, "NamePseudo"),
    (2013, "NameProperty"),
    (2014, "NameTag"),
    (2100, "NameBuiltin"),
    (2101, "NameBuiltinPseudo"),
    (2200, "NameVariable"),
    (2201, "NameVariableAnonymous"),
    (2202, "NameVariableClass"),
    (2203, "NameVariableGlobal"),
    (2204, "NameVariableInstance"),
    (2205, "NameVariableMagic"),
    (2300, "NameFunction"),
    (2301, "NameFunctionMagic"),
    (3000, "Literal"),
    (3001, "LiteralDate"),
    (3002, "LiteralOther"),
    (3100, "LiteralString"),
    (3101, "LiteralStringAffix"),
    (3102, "LiteralStringAtom"),
    (3103, "LiteralStringBacktick"),
    (3104, "LiteralStringBoolean"),
    (3105, "LiteralStringChar"),
    (3106, "LiteralStringDelimiter"),
    (3107, "LiteralStringDoc"),
    (3108, "LiteralStringDouble"),
    (3109, "LiteralStringEscape"),
    (3110, "LiteralStringHeredoc"),
    (3111, "LiteralStringInterpol"),
    (3112, "LiteralStringName"),
    (3113, "LiteralStringOther"),
    (3114, "LiteralStringRegex"),
    (3115, "LiteralStringSingle"),
    (3116, "LiteralStringSymbol"),
    (3200, "LiteralNumber"),
    (3201, "LiteralNumberBin"),
    (3202, "LiteralNumberFloat"),
    (3203, "LiteralNumberHex"),
    (3204, "LiteralNumberInteger"),
    (3205, "LiteralNumberIntegerLong"),
    (3206, "LiteralNumberOct"),
    (3207, "LiteralNumberByte"),
    (4000, "Operator"),
    (4001, "OperatorWord"),
    (4002, "OperatorReserved"),
    (5000, "Punctuation"),
    (6000, "Comment"),
    (6001, "CommentHashbang"),
    (6002, "CommentMultiline"),
    (6003, "CommentSingle"),
    (6004, "CommentSpecial"),
    (6100, "CommentPreproc"),
    (6101, "CommentPreprocFile"),
    (7000, "Generic"),
    (7001, "GenericDeleted"),
    (7002, "GenericEmph"),
    (7003, "GenericError"),
    (7004, "GenericHeading"),
    (7005, "GenericInserted"),
    (7006, "GenericOutput"),
    (7007, "GenericPrompt"),
    (7008, "GenericStrong"),
    (7009, "GenericSubheading"),
    (7010, "GenericTraceback"),
    (7011, "GenericUnderline"),
    (8000, "Text"),
    (8001, "TextWhitespace"),
    (8002, "TextSymbol"),
    (8003, "TextPunctuation"),
];
