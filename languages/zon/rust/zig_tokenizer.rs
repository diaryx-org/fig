//! Port of lib/std/zig/tokenizer.zig (Zig 0.17.0).
//!
//! ZON is a subset of Zig source, and fig's ZON parser leans on the Zig
//! standard library's own tokenizer and parser (`std.zig.Ast.parse(.zon)`)
//! rather than hand-rolling one (see `parser.rs`). This is that tokenizer,
//! ported state for state, so that the bytes it accepts and the tokens it
//! cuts are exactly the pinned compiler's.
//!
//! The Zig tokenizer reads a sentinel-terminated buffer: the byte at
//! `buffer.len` is `0`, and a `0` anywhere else is an invalid byte. Here
//! the buffer is a plain slice, and `byte(i)` answers `0` at and past its
//! end.

/// A token's tag (Zig's `Token.Tag`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Tag {
    Invalid,
    Identifier,
    StringLiteral,
    MultilineStringLiteralLine,
    CharLiteral,
    Eof,
    Builtin,
    Bang,
    Pipe,
    PipePipe,
    PipeEqual,
    Equal,
    EqualEqual,
    EqualAngleBracketRight,
    BangEqual,
    LParen,
    RParen,
    Semicolon,
    Percent,
    PercentEqual,
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    Period,
    PeriodAsterisk,
    Ellipsis2,
    Ellipsis3,
    Caret,
    CaretEqual,
    Plus,
    PlusPlus,
    PlusEqual,
    PlusPercent,
    PlusPercentEqual,
    PlusPipe,
    PlusPipeEqual,
    Minus,
    MinusEqual,
    MinusPercent,
    MinusPercentEqual,
    MinusPipe,
    MinusPipeEqual,
    Asterisk,
    AsteriskEqual,
    AsteriskPercent,
    AsteriskPercentEqual,
    AsteriskPipe,
    AsteriskPipeEqual,
    Arrow,
    Colon,
    Slash,
    SlashEqual,
    Comma,
    Ampersand,
    AmpersandEqual,
    QuestionMark,
    AngleBracketLeft,
    AngleBracketLeftEqual,
    AngleBracketAngleBracketLeft,
    AngleBracketAngleBracketLeftEqual,
    AngleBracketAngleBracketLeftPipe,
    AngleBracketAngleBracketLeftPipeEqual,
    AngleBracketRight,
    AngleBracketRightEqual,
    AngleBracketAngleBracketRight,
    AngleBracketAngleBracketRightEqual,
    Tilde,
    NumberLiteral,
    DocComment,
    ContainerDocComment,
    KeywordAddrspace,
    KeywordAlign,
    KeywordAllowzero,
    KeywordAnd,
    KeywordAnyframe,
    KeywordAnytype,
    KeywordAsm,
    KeywordBreak,
    KeywordCallconv,
    KeywordCatch,
    KeywordComptime,
    KeywordConst,
    KeywordContinue,
    KeywordDefer,
    KeywordElse,
    KeywordEnum,
    KeywordErrdefer,
    KeywordError,
    KeywordExport,
    KeywordExtern,
    KeywordFn,
    KeywordFor,
    KeywordIf,
    KeywordInline,
    KeywordNoalias,
    KeywordNoinline,
    KeywordNosuspend,
    KeywordOpaque,
    KeywordOr,
    KeywordOrelse,
    KeywordPacked,
    KeywordPub,
    KeywordResume,
    KeywordReturn,
    KeywordLinksection,
    KeywordStruct,
    KeywordSuspend,
    KeywordSwitch,
    KeywordTest,
    KeywordThreadlocal,
    KeywordTry,
    KeywordUnion,
    KeywordUnreachable,
    KeywordVar,
    KeywordVolatile,
    KeywordWhile,
}

const KEYWORDS: &[(&[u8], Tag)] = &[
    (b"addrspace", Tag::KeywordAddrspace),
    (b"align", Tag::KeywordAlign),
    (b"allowzero", Tag::KeywordAllowzero),
    (b"and", Tag::KeywordAnd),
    (b"anyframe", Tag::KeywordAnyframe),
    (b"anytype", Tag::KeywordAnytype),
    (b"asm", Tag::KeywordAsm),
    (b"break", Tag::KeywordBreak),
    (b"callconv", Tag::KeywordCallconv),
    (b"catch", Tag::KeywordCatch),
    (b"comptime", Tag::KeywordComptime),
    (b"const", Tag::KeywordConst),
    (b"continue", Tag::KeywordContinue),
    (b"defer", Tag::KeywordDefer),
    (b"else", Tag::KeywordElse),
    (b"enum", Tag::KeywordEnum),
    (b"errdefer", Tag::KeywordErrdefer),
    (b"error", Tag::KeywordError),
    (b"export", Tag::KeywordExport),
    (b"extern", Tag::KeywordExtern),
    (b"fn", Tag::KeywordFn),
    (b"for", Tag::KeywordFor),
    (b"if", Tag::KeywordIf),
    (b"inline", Tag::KeywordInline),
    (b"noalias", Tag::KeywordNoalias),
    (b"noinline", Tag::KeywordNoinline),
    (b"nosuspend", Tag::KeywordNosuspend),
    (b"opaque", Tag::KeywordOpaque),
    (b"or", Tag::KeywordOr),
    (b"orelse", Tag::KeywordOrelse),
    (b"packed", Tag::KeywordPacked),
    (b"pub", Tag::KeywordPub),
    (b"resume", Tag::KeywordResume),
    (b"return", Tag::KeywordReturn),
    (b"linksection", Tag::KeywordLinksection),
    (b"struct", Tag::KeywordStruct),
    (b"suspend", Tag::KeywordSuspend),
    (b"switch", Tag::KeywordSwitch),
    (b"test", Tag::KeywordTest),
    (b"threadlocal", Tag::KeywordThreadlocal),
    (b"try", Tag::KeywordTry),
    (b"union", Tag::KeywordUnion),
    (b"unreachable", Tag::KeywordUnreachable),
    (b"var", Tag::KeywordVar),
    (b"volatile", Tag::KeywordVolatile),
    (b"while", Tag::KeywordWhile),
];

/// Zig's `Token.getKeyword`: the keyword `bytes` spells, if it spells one.
pub fn get_keyword(bytes: &[u8]) -> Option<Tag> {
    KEYWORDS.iter().find(|(k, _)| *k == bytes).map(|&(_, t)| t)
}

impl Tag {
    /// The fixed spelling of a tag, or `None` for one whose text varies
    /// (Zig's `Tag.lexeme`).
    pub fn lexeme(self) -> Option<&'static [u8]> {
        use Tag::*;
        Some(match self {
            Invalid | Identifier | StringLiteral | MultilineStringLiteralLine | CharLiteral | Eof | Builtin
            | NumberLiteral | DocComment | ContainerDocComment => return None,
            Bang => b"!",
            Pipe => b"|",
            PipePipe => b"||",
            PipeEqual => b"|=",
            Equal => b"=",
            EqualEqual => b"==",
            EqualAngleBracketRight => b"=>",
            BangEqual => b"!=",
            LParen => b"(",
            RParen => b")",
            Semicolon => b";",
            Percent => b"%",
            PercentEqual => b"%=",
            LBrace => b"{",
            RBrace => b"}",
            LBracket => b"[",
            RBracket => b"]",
            Period => b".",
            PeriodAsterisk => b".*",
            Ellipsis2 => b"..",
            Ellipsis3 => b"...",
            Caret => b"^",
            CaretEqual => b"^=",
            Plus => b"+",
            PlusPlus => b"++",
            PlusEqual => b"+=",
            PlusPercent => b"+%",
            PlusPercentEqual => b"+%=",
            PlusPipe => b"+|",
            PlusPipeEqual => b"+|=",
            Minus => b"-",
            MinusEqual => b"-=",
            MinusPercent => b"-%",
            MinusPercentEqual => b"-%=",
            MinusPipe => b"-|",
            MinusPipeEqual => b"-|=",
            Asterisk => b"*",
            AsteriskEqual => b"*=",
            AsteriskPercent => b"*%",
            AsteriskPercentEqual => b"*%=",
            AsteriskPipe => b"*|",
            AsteriskPipeEqual => b"*|=",
            Arrow => b"->",
            Colon => b":",
            Slash => b"/",
            SlashEqual => b"/=",
            Comma => b",",
            Ampersand => b"&",
            AmpersandEqual => b"&=",
            QuestionMark => b"?",
            AngleBracketLeft => b"<",
            AngleBracketLeftEqual => b"<=",
            AngleBracketAngleBracketLeft => b"<<",
            AngleBracketAngleBracketLeftEqual => b"<<=",
            AngleBracketAngleBracketLeftPipe => b"<<|",
            AngleBracketAngleBracketLeftPipeEqual => b"<<|=",
            AngleBracketRight => b">",
            AngleBracketRightEqual => b">=",
            AngleBracketAngleBracketRight => b">>",
            AngleBracketAngleBracketRightEqual => b">>=",
            Tilde => b"~",
            KeywordAddrspace => b"addrspace",
            KeywordAlign => b"align",
            KeywordAllowzero => b"allowzero",
            KeywordAnd => b"and",
            KeywordAnyframe => b"anyframe",
            KeywordAnytype => b"anytype",
            KeywordAsm => b"asm",
            KeywordBreak => b"break",
            KeywordCallconv => b"callconv",
            KeywordCatch => b"catch",
            KeywordComptime => b"comptime",
            KeywordConst => b"const",
            KeywordContinue => b"continue",
            KeywordDefer => b"defer",
            KeywordElse => b"else",
            KeywordEnum => b"enum",
            KeywordErrdefer => b"errdefer",
            KeywordError => b"error",
            KeywordExport => b"export",
            KeywordExtern => b"extern",
            KeywordFn => b"fn",
            KeywordFor => b"for",
            KeywordIf => b"if",
            KeywordInline => b"inline",
            KeywordNoalias => b"noalias",
            KeywordNoinline => b"noinline",
            KeywordNosuspend => b"nosuspend",
            KeywordOpaque => b"opaque",
            KeywordOr => b"or",
            KeywordOrelse => b"orelse",
            KeywordPacked => b"packed",
            KeywordPub => b"pub",
            KeywordResume => b"resume",
            KeywordReturn => b"return",
            KeywordLinksection => b"linksection",
            KeywordStruct => b"struct",
            KeywordSuspend => b"suspend",
            KeywordSwitch => b"switch",
            KeywordTest => b"test",
            KeywordThreadlocal => b"threadlocal",
            KeywordTry => b"try",
            KeywordUnion => b"union",
            KeywordUnreachable => b"unreachable",
            KeywordVar => b"var",
            KeywordVolatile => b"volatile",
            KeywordWhile => b"while",
        })
    }
}

/// A token: its tag and where it lies (`loc`), `[start, end)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Token {
    pub tag: Tag,
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Start,
    ExpectNewline,
    Identifier,
    Builtin,
    StringLiteral,
    StringLiteralBackslash,
    MultilineStringLiteralLine,
    CharLiteral,
    CharLiteralBackslash,
    Backslash,
    Equal,
    Bang,
    Pipe,
    Minus,
    MinusPercent,
    MinusPipe,
    Asterisk,
    AsteriskPercent,
    AsteriskPipe,
    Slash,
    LineCommentStart,
    LineComment,
    DocCommentStart,
    DocComment,
    Int,
    IntExponent,
    IntPeriod,
    Float,
    FloatExponent,
    Ampersand,
    Caret,
    Percent,
    Plus,
    PlusPercent,
    PlusPipe,
    AngleBracketLeft,
    AngleBracketAngleBracketLeft,
    AngleBracketAngleBracketLeftPipe,
    AngleBracketRight,
    AngleBracketAngleBracketRight,
    Period,
    Period2,
    SawAtSign,
    Invalid,
}

pub struct Tokenizer<'a> {
    pub buffer: &'a [u8],
    pub index: usize,
}

/// A control byte a string, a char literal or a comment may not hold
/// (`0x01...0x09, 0x0b...0x1f, 0x7f`).
fn is_string_control(c: u8) -> bool {
    matches!(c, 0x01..=0x09 | 0x0b..=0x1f | 0x7f)
}

/// A control byte a comment or a multiline string line may not hold
/// (`0x01...0x09, 0x0b...0x0c, 0x0e...0x1f, 0x7f`).
fn is_comment_control(c: u8) -> bool {
    matches!(c, 0x01..=0x09 | 0x0b..=0x0c | 0x0e..=0x1f | 0x7f)
}

/// A byte that continues a number literal's digits (`_`, the letters but
/// `e`/`E`/`p`/`P`, and the decimal digits).
fn is_number_body(c: u8) -> bool {
    matches!(c, b'_' | b'a'..=b'd' | b'f'..=b'o' | b'q'..=b'z' | b'A'..=b'D' | b'F'..=b'O' | b'Q'..=b'Z' | b'0'..=b'9')
}

impl<'a> Tokenizer<'a> {
    pub fn init(buffer: &'a [u8]) -> Tokenizer<'a> {
        // Skip the UTF-8 BOM if present.
        Tokenizer { buffer, index: if buffer.starts_with(b"\xEF\xBB\xBF") { 3 } else { 0 } }
    }

    /// The byte at `i`, or the sentinel `0` at and past the end.
    fn byte(&self, i: usize) -> u8 {
        self.buffer.get(i).copied().unwrap_or(0)
    }

    /// After this returns invalid, it will reset on the next newline,
    /// returning tokens starting from there. An eof token will always be
    /// returned at the end.
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> Token {
        let mut tag = Tag::Invalid;
        let mut start = self.index;
        let mut state = State::Start;
        let len = self.buffer.len();
        loop {
            match state {
                State::Start => match self.byte(self.index) {
                    0 => {
                        if self.index == len {
                            return Token { tag: Tag::Eof, start: self.index, end: self.index };
                        }
                        state = State::Invalid;
                    }
                    b' ' | b'\n' | b'\t' | b'\r' => {
                        self.index += 1;
                        start = self.index;
                    }
                    b'"' => {
                        tag = Tag::StringLiteral;
                        state = State::StringLiteral;
                    }
                    b'\'' => {
                        tag = Tag::CharLiteral;
                        state = State::CharLiteral;
                    }
                    b'a'..=b'z' | b'A'..=b'Z' | b'_' => {
                        tag = Tag::Identifier;
                        state = State::Identifier;
                    }
                    b'@' => state = State::SawAtSign,
                    b'=' => state = State::Equal,
                    b'!' => state = State::Bang,
                    b'|' => state = State::Pipe,
                    b'(' => {
                        tag = Tag::LParen;
                        self.index += 1;
                        break;
                    }
                    b')' => {
                        tag = Tag::RParen;
                        self.index += 1;
                        break;
                    }
                    b'[' => {
                        tag = Tag::LBracket;
                        self.index += 1;
                        break;
                    }
                    b']' => {
                        tag = Tag::RBracket;
                        self.index += 1;
                        break;
                    }
                    b';' => {
                        tag = Tag::Semicolon;
                        self.index += 1;
                        break;
                    }
                    b',' => {
                        tag = Tag::Comma;
                        self.index += 1;
                        break;
                    }
                    b'?' => {
                        tag = Tag::QuestionMark;
                        self.index += 1;
                        break;
                    }
                    b':' => {
                        tag = Tag::Colon;
                        self.index += 1;
                        break;
                    }
                    b'%' => state = State::Percent,
                    b'*' => state = State::Asterisk,
                    b'+' => state = State::Plus,
                    b'<' => state = State::AngleBracketLeft,
                    b'>' => state = State::AngleBracketRight,
                    b'^' => state = State::Caret,
                    b'\\' => {
                        tag = Tag::MultilineStringLiteralLine;
                        state = State::Backslash;
                    }
                    b'{' => {
                        tag = Tag::LBrace;
                        self.index += 1;
                        break;
                    }
                    b'}' => {
                        tag = Tag::RBrace;
                        self.index += 1;
                        break;
                    }
                    b'~' => {
                        tag = Tag::Tilde;
                        self.index += 1;
                        break;
                    }
                    b'.' => state = State::Period,
                    b'-' => state = State::Minus,
                    b'/' => state = State::Slash,
                    b'&' => state = State::Ampersand,
                    b'0'..=b'9' => {
                        tag = Tag::NumberLiteral;
                        self.index += 1;
                        state = State::Int;
                    }
                    _ => state = State::Invalid,
                },

                State::ExpectNewline => {
                    self.index += 1;
                    match self.byte(self.index) {
                        0 => {
                            if self.index == len {
                                tag = Tag::Invalid;
                                break;
                            }
                            state = State::Invalid;
                        }
                        b'\n' => {
                            self.index += 1;
                            start = self.index;
                            state = State::Start;
                        }
                        _ => state = State::Invalid,
                    }
                }

                State::Invalid => {
                    self.index += 1;
                    match self.byte(self.index) {
                        0 => {
                            if self.index == len {
                                tag = Tag::Invalid;
                                break;
                            }
                        }
                        b'\n' => {
                            tag = Tag::Invalid;
                            break;
                        }
                        _ => {}
                    }
                }

                State::SawAtSign => {
                    self.index += 1;
                    match self.byte(self.index) {
                        0 | b'\n' => {
                            tag = Tag::Invalid;
                            break;
                        }
                        b'"' => {
                            tag = Tag::Identifier;
                            state = State::StringLiteral;
                        }
                        b'a'..=b'z' | b'A'..=b'Z' | b'_' => {
                            tag = Tag::Builtin;
                            state = State::Builtin;
                        }
                        _ => state = State::Invalid,
                    }
                }

                State::Ampersand => {
                    self.index += 1;
                    if self.byte(self.index) == b'=' {
                        tag = Tag::AmpersandEqual;
                        self.index += 1;
                    } else {
                        tag = Tag::Ampersand;
                    }
                    break;
                }

                State::Asterisk => {
                    self.index += 1;
                    match self.byte(self.index) {
                        b'=' => {
                            tag = Tag::AsteriskEqual;
                            self.index += 1;
                            break;
                        }
                        b'%' => state = State::AsteriskPercent,
                        b'|' => state = State::AsteriskPipe,
                        _ => {
                            tag = Tag::Asterisk;
                            break;
                        }
                    }
                }

                State::AsteriskPercent => {
                    self.index += 1;
                    if self.byte(self.index) == b'=' {
                        tag = Tag::AsteriskPercentEqual;
                        self.index += 1;
                    } else {
                        tag = Tag::AsteriskPercent;
                    }
                    break;
                }

                State::AsteriskPipe => {
                    self.index += 1;
                    if self.byte(self.index) == b'=' {
                        tag = Tag::AsteriskPipeEqual;
                        self.index += 1;
                    } else {
                        tag = Tag::AsteriskPipe;
                    }
                    break;
                }

                State::Percent => {
                    self.index += 1;
                    if self.byte(self.index) == b'=' {
                        tag = Tag::PercentEqual;
                        self.index += 1;
                    } else {
                        tag = Tag::Percent;
                    }
                    break;
                }

                State::Plus => {
                    self.index += 1;
                    match self.byte(self.index) {
                        b'=' => {
                            tag = Tag::PlusEqual;
                            self.index += 1;
                            break;
                        }
                        b'+' => {
                            tag = Tag::PlusPlus;
                            self.index += 1;
                            break;
                        }
                        b'%' => state = State::PlusPercent,
                        b'|' => state = State::PlusPipe,
                        _ => {
                            tag = Tag::Plus;
                            break;
                        }
                    }
                }

                State::PlusPercent => {
                    self.index += 1;
                    if self.byte(self.index) == b'=' {
                        tag = Tag::PlusPercentEqual;
                        self.index += 1;
                    } else {
                        tag = Tag::PlusPercent;
                    }
                    break;
                }

                State::PlusPipe => {
                    self.index += 1;
                    if self.byte(self.index) == b'=' {
                        tag = Tag::PlusPipeEqual;
                        self.index += 1;
                    } else {
                        tag = Tag::PlusPipe;
                    }
                    break;
                }

                State::Caret => {
                    self.index += 1;
                    if self.byte(self.index) == b'=' {
                        tag = Tag::CaretEqual;
                        self.index += 1;
                    } else {
                        tag = Tag::Caret;
                    }
                    break;
                }

                State::Identifier => {
                    self.index += 1;
                    match self.byte(self.index) {
                        b'a'..=b'z' | b'A'..=b'Z' | b'_' | b'0'..=b'9' => {}
                        _ => {
                            if let Some(t) = get_keyword(&self.buffer[start..self.index]) {
                                tag = t;
                            }
                            break;
                        }
                    }
                }
                State::Builtin => {
                    self.index += 1;
                    match self.byte(self.index) {
                        b'a'..=b'z' | b'A'..=b'Z' | b'_' | b'0'..=b'9' => {}
                        _ => break,
                    }
                }
                State::Backslash => {
                    self.index += 1;
                    match self.byte(self.index) {
                        0 => {
                            tag = Tag::Invalid;
                            break;
                        }
                        b'\\' => state = State::MultilineStringLiteralLine,
                        b'\n' => {
                            tag = Tag::Invalid;
                            break;
                        }
                        _ => state = State::Invalid,
                    }
                }
                State::StringLiteral => {
                    self.index += 1;
                    match self.byte(self.index) {
                        0 => {
                            if self.index != len {
                                state = State::Invalid;
                            } else {
                                tag = Tag::Invalid;
                                break;
                            }
                        }
                        b'\n' => {
                            tag = Tag::Invalid;
                            break;
                        }
                        b'\\' => state = State::StringLiteralBackslash,
                        b'"' => {
                            self.index += 1;
                            break;
                        }
                        c if is_string_control(c) => state = State::Invalid,
                        _ => {}
                    }
                }

                State::StringLiteralBackslash => {
                    self.index += 1;
                    match self.byte(self.index) {
                        0 | b'\n' => {
                            tag = Tag::Invalid;
                            break;
                        }
                        c if is_string_control(c) => state = State::Invalid,
                        _ => state = State::StringLiteral,
                    }
                }

                State::CharLiteral => {
                    self.index += 1;
                    match self.byte(self.index) {
                        0 => {
                            if self.index != len {
                                state = State::Invalid;
                            } else {
                                tag = Tag::Invalid;
                                break;
                            }
                        }
                        b'\n' => {
                            tag = Tag::Invalid;
                            break;
                        }
                        b'\\' => state = State::CharLiteralBackslash,
                        b'\'' => {
                            self.index += 1;
                            break;
                        }
                        c if is_string_control(c) => state = State::Invalid,
                        _ => {}
                    }
                }

                State::CharLiteralBackslash => {
                    self.index += 1;
                    match self.byte(self.index) {
                        0 => {
                            if self.index != len {
                                state = State::Invalid;
                            } else {
                                tag = Tag::Invalid;
                                break;
                            }
                        }
                        b'\n' => {
                            tag = Tag::Invalid;
                            break;
                        }
                        c if is_string_control(c) => state = State::Invalid,
                        _ => state = State::CharLiteral,
                    }
                }

                State::MultilineStringLiteralLine => {
                    self.index += 1;
                    match self.byte(self.index) {
                        0 => {
                            if self.index != len {
                                state = State::Invalid;
                            } else {
                                break;
                            }
                        }
                        b'\n' => break,
                        b'\r' => {
                            if self.byte(self.index + 1) != b'\n' {
                                state = State::Invalid;
                            } else {
                                break;
                            }
                        }
                        c if is_comment_control(c) => state = State::Invalid,
                        _ => {}
                    }
                }

                State::Bang => {
                    self.index += 1;
                    if self.byte(self.index) == b'=' {
                        tag = Tag::BangEqual;
                        self.index += 1;
                    } else {
                        tag = Tag::Bang;
                    }
                    break;
                }

                State::Pipe => {
                    self.index += 1;
                    match self.byte(self.index) {
                        b'=' => {
                            tag = Tag::PipeEqual;
                            self.index += 1;
                        }
                        b'|' => {
                            tag = Tag::PipePipe;
                            self.index += 1;
                        }
                        _ => tag = Tag::Pipe,
                    }
                    break;
                }

                State::Equal => {
                    self.index += 1;
                    match self.byte(self.index) {
                        b'=' => {
                            tag = Tag::EqualEqual;
                            self.index += 1;
                        }
                        b'>' => {
                            tag = Tag::EqualAngleBracketRight;
                            self.index += 1;
                        }
                        _ => tag = Tag::Equal,
                    }
                    break;
                }

                State::Minus => {
                    self.index += 1;
                    match self.byte(self.index) {
                        b'>' => {
                            tag = Tag::Arrow;
                            self.index += 1;
                            break;
                        }
                        b'=' => {
                            tag = Tag::MinusEqual;
                            self.index += 1;
                            break;
                        }
                        b'%' => state = State::MinusPercent,
                        b'|' => state = State::MinusPipe,
                        _ => {
                            tag = Tag::Minus;
                            break;
                        }
                    }
                }

                State::MinusPercent => {
                    self.index += 1;
                    if self.byte(self.index) == b'=' {
                        tag = Tag::MinusPercentEqual;
                        self.index += 1;
                    } else {
                        tag = Tag::MinusPercent;
                    }
                    break;
                }
                State::MinusPipe => {
                    self.index += 1;
                    if self.byte(self.index) == b'=' {
                        tag = Tag::MinusPipeEqual;
                        self.index += 1;
                    } else {
                        tag = Tag::MinusPipe;
                    }
                    break;
                }

                State::AngleBracketLeft => {
                    self.index += 1;
                    match self.byte(self.index) {
                        b'<' => state = State::AngleBracketAngleBracketLeft,
                        b'=' => {
                            tag = Tag::AngleBracketLeftEqual;
                            self.index += 1;
                            break;
                        }
                        _ => {
                            tag = Tag::AngleBracketLeft;
                            break;
                        }
                    }
                }

                State::AngleBracketAngleBracketLeft => {
                    self.index += 1;
                    match self.byte(self.index) {
                        b'=' => {
                            tag = Tag::AngleBracketAngleBracketLeftEqual;
                            self.index += 1;
                            break;
                        }
                        b'|' => state = State::AngleBracketAngleBracketLeftPipe,
                        _ => {
                            tag = Tag::AngleBracketAngleBracketLeft;
                            break;
                        }
                    }
                }

                State::AngleBracketAngleBracketLeftPipe => {
                    self.index += 1;
                    if self.byte(self.index) == b'=' {
                        tag = Tag::AngleBracketAngleBracketLeftPipeEqual;
                        self.index += 1;
                    } else {
                        tag = Tag::AngleBracketAngleBracketLeftPipe;
                    }
                    break;
                }

                State::AngleBracketRight => {
                    self.index += 1;
                    match self.byte(self.index) {
                        b'>' => state = State::AngleBracketAngleBracketRight,
                        b'=' => {
                            tag = Tag::AngleBracketRightEqual;
                            self.index += 1;
                            break;
                        }
                        _ => {
                            tag = Tag::AngleBracketRight;
                            break;
                        }
                    }
                }

                State::AngleBracketAngleBracketRight => {
                    self.index += 1;
                    if self.byte(self.index) == b'=' {
                        tag = Tag::AngleBracketAngleBracketRightEqual;
                        self.index += 1;
                    } else {
                        tag = Tag::AngleBracketAngleBracketRight;
                    }
                    break;
                }

                State::Period => {
                    self.index += 1;
                    match self.byte(self.index) {
                        b'.' => state = State::Period2,
                        b'*' => {
                            tag = Tag::PeriodAsterisk;
                            self.index += 1;
                            break;
                        }
                        _ => {
                            tag = Tag::Period;
                            break;
                        }
                    }
                }

                State::Period2 => {
                    self.index += 1;
                    if self.byte(self.index) == b'.' {
                        tag = Tag::Ellipsis3;
                        self.index += 1;
                    } else {
                        tag = Tag::Ellipsis2;
                    }
                    break;
                }

                State::Slash => {
                    self.index += 1;
                    match self.byte(self.index) {
                        b'/' => state = State::LineCommentStart,
                        b'=' => {
                            tag = Tag::SlashEqual;
                            self.index += 1;
                            break;
                        }
                        _ => {
                            tag = Tag::Slash;
                            break;
                        }
                    }
                }
                State::LineCommentStart => {
                    self.index += 1;
                    match self.byte(self.index) {
                        0 => {
                            if self.index != len {
                                state = State::Invalid;
                            } else {
                                return Token { tag: Tag::Eof, start: self.index, end: self.index };
                            }
                        }
                        b'!' => {
                            tag = Tag::ContainerDocComment;
                            state = State::DocComment;
                        }
                        b'\n' => {
                            self.index += 1;
                            start = self.index;
                            state = State::Start;
                        }
                        b'/' => state = State::DocCommentStart,
                        b'\r' => state = State::ExpectNewline,
                        c if is_comment_control(c) => state = State::Invalid,
                        _ => state = State::LineComment,
                    }
                }
                State::DocCommentStart => {
                    self.index += 1;
                    match self.byte(self.index) {
                        0 | b'\n' => {
                            tag = Tag::DocComment;
                            break;
                        }
                        b'\r' => {
                            if self.byte(self.index + 1) == b'\n' {
                                tag = Tag::DocComment;
                                break;
                            }
                            state = State::Invalid;
                        }
                        b'/' => state = State::LineComment,
                        c if is_comment_control(c) => state = State::Invalid,
                        _ => {
                            tag = Tag::DocComment;
                            state = State::DocComment;
                        }
                    }
                }
                State::LineComment => {
                    self.index += 1;
                    match self.byte(self.index) {
                        0 => {
                            if self.index != len {
                                state = State::Invalid;
                            } else {
                                return Token { tag: Tag::Eof, start: self.index, end: self.index };
                            }
                        }
                        b'\n' => {
                            self.index += 1;
                            start = self.index;
                            state = State::Start;
                        }
                        b'\r' => state = State::ExpectNewline,
                        c if is_comment_control(c) => state = State::Invalid,
                        _ => {}
                    }
                }
                State::DocComment => {
                    self.index += 1;
                    match self.byte(self.index) {
                        0 | b'\n' => break,
                        b'\r' => {
                            if self.byte(self.index + 1) != b'\n' {
                                state = State::Invalid;
                            } else {
                                break;
                            }
                        }
                        c if is_comment_control(c) => state = State::Invalid,
                        _ => {}
                    }
                }
                State::Int => match self.byte(self.index) {
                    b'.' => state = State::IntPeriod,
                    c if is_number_body(c) => self.index += 1,
                    b'e' | b'E' | b'p' | b'P' => state = State::IntExponent,
                    _ => break,
                },
                State::IntExponent => {
                    self.index += 1;
                    match self.byte(self.index) {
                        b'-' | b'+' => {
                            self.index += 1;
                            state = State::Float;
                        }
                        _ => state = State::Int,
                    }
                }
                State::IntPeriod => {
                    self.index += 1;
                    match self.byte(self.index) {
                        c if is_number_body(c) => {
                            self.index += 1;
                            state = State::Float;
                        }
                        b'e' | b'E' | b'p' | b'P' => state = State::FloatExponent,
                        _ => {
                            self.index -= 1;
                            break;
                        }
                    }
                }
                State::Float => match self.byte(self.index) {
                    c if is_number_body(c) => self.index += 1,
                    b'e' | b'E' | b'p' | b'P' => state = State::FloatExponent,
                    _ => break,
                },
                State::FloatExponent => {
                    self.index += 1;
                    match self.byte(self.index) {
                        b'-' | b'+' => {
                            self.index += 1;
                            state = State::Float;
                        }
                        _ => state = State::Float,
                    }
                }
            }
        }
        Token { tag, start, end: self.index }
    }
}

/// Every token of `source`, ending with the `Eof` (what `Ast.parse` lists).
pub fn tokenize(source: &[u8]) -> Vec<Token> {
    let mut t = Tokenizer::init(source);
    let mut tokens = Vec::with_capacity(source.len() / 8);
    loop {
        let token = t.next();
        tokens.push(token);
        if token.tag == Tag::Eof {
            break;
        }
    }
    tokens
}

#[cfg(test)]
mod tests {
    use super::Tag::*;
    use super::*;

    fn test_tokenize(source: &str, expected: &[Tag]) {
        let mut t = Tokenizer::init(source.as_bytes());
        for &e in expected {
            let token = t.next();
            assert_eq!(e, token.tag, "in {source:?}");
        }
        // Last token should always be eof, even when the last token was
        // invalid, in which case the tokenizer is in an invalid state, which
        // can only be recovered by opinionated means outside the scope of
        // this implementation.
        let last = t.next();
        assert_eq!(Eof, last.tag);
        assert_eq!(source.len(), last.start);
        assert_eq!(source.len(), last.end);
    }

    #[test]
    fn keywords() {
        test_tokenize("test const else", &[KeywordTest, KeywordConst, KeywordElse]);
    }

    #[test]
    fn line_comment_followed_by_top_level_comptime() {
        test_tokenize("// line comment\ncomptime {}\n", &[KeywordComptime, LBrace, RBrace]);
    }

    #[test]
    fn unknown_length_pointer_and_then_c_pointer() {
        test_tokenize(
            "[*]u8\n[*c]u8",
            &[LBracket, Asterisk, RBracket, Identifier, LBracket, Asterisk, Identifier, RBracket, Identifier],
        );
    }

    #[test]
    fn code_point_literal_with_hex_escape() {
        test_tokenize("'\\x1b'", &[CharLiteral]);
        test_tokenize("'\\x1'", &[CharLiteral]);
    }

    #[test]
    fn newline_in_char_and_string_literal() {
        test_tokenize("'\n'", &[Invalid, Invalid]);
        test_tokenize("\"\n\"", &[Invalid, Invalid]);
    }

    #[test]
    fn code_point_literal_with_unicode_escapes() {
        test_tokenize("'\\u{3}'", &[CharLiteral]);
        test_tokenize("'\\u{6E09aBc1523}'", &[CharLiteral]);
        test_tokenize("\"\\u{440}\"", &[StringLiteral]);
        test_tokenize("'\\u'", &[CharLiteral]);
        test_tokenize("'\\U0333'", &[CharLiteral]);
        test_tokenize("'💩'", &[CharLiteral]);
    }

    #[test]
    fn multiline_string_literals_and_comments() {
        test_tokenize("\\\\a\n\\\\b\r\n", &[MultilineStringLiteralLine, MultilineStringLiteralLine]);
        test_tokenize("//! doc\n/// d\n//// not doc\n", &[ContainerDocComment, DocComment]);
        test_tokenize("// a\rb", &[Invalid]);
        test_tokenize("\\", &[Invalid]);
    }

    #[test]
    fn numbers() {
        test_tokenize("0x1.5p3 1e10 1.e3 1..2 0xFFe 1_000", &[
            NumberLiteral,
            NumberLiteral,
            NumberLiteral,
            NumberLiteral,
            Ellipsis2,
            NumberLiteral,
            NumberLiteral,
            NumberLiteral,
        ]);
        test_tokenize("1.+2", &[NumberLiteral, Period, Plus, NumberLiteral]);
    }

    #[test]
    fn operators() {
        test_tokenize("a<<|=b+%=c**d", &[
            Identifier,
            AngleBracketAngleBracketLeftPipeEqual,
            Identifier,
            PlusPercentEqual,
            Identifier,
            Asterisk,
            Asterisk,
            Identifier,
        ]);
        test_tokenize("@\"x y\" @foo .* .. ... =>", &[Identifier, Builtin, PeriodAsterisk, Ellipsis2, Ellipsis3, EqualAngleBracketRight]);
    }

    #[test]
    fn an_embedded_nul_is_invalid_and_the_bom_is_skipped() {
        test_tokenize("a\0b", &[Identifier, Invalid]);
        test_tokenize("\u{feff}a", &[Identifier]);
    }
}
