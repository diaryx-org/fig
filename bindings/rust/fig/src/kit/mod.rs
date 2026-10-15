//! The kit: what a [`Language`](crate::language::Language) written in Rust
//! is built from. It is the same kit fig's own languages are written with,
//! and that `@diaryx/fig/kit` is in JavaScript, layer for layer, and the
//! kit's shared fixtures (`kit/testdata/` in the fig repository) hold every
//! binding's to the same answers.
//!
//! The **rules** have one right answer, which fig's own languages give, so
//! they are the core kit's own code (`fig-core`'s `kit`, the Rust port of
//! the Zig kit), called directly:
//!
//! * [`number`]: how a number's lexeme is spelled for a format;
//! * [`datetime`]: which datetime a bare token is;
//! * [`literal`]: what a bare token means when nothing else says;
//! * [`json_string`]: a string quoted as JSON or JSON5 quotes it;
//! * [`xml`]: the tokens of an XML-shaped format.
//!
//! The **structure** is Rust of its own, since it deals in the values a
//! language builds:
//!
//! * [`rows`]: the [`NodeTable`](crate::language::NodeTable) written in any
//!   order, linked, and finished into the pre-order a parse returns;
//! * [`tree`]: nodes that hold their children and comments, for a parser
//!   that builds owned values, laid out as rows at the end;
//! * [`scan`]: a byte cursor whose positions are already the table's
//!   offsets, and a cursor over a token list;
//! * [`grammar`]: combinators over the scanner that write the table as they
//!   match, so a format is a description;
//! * [`print`]: the table a `print` receives, indexed by node, and a writer
//!   that knows the print options.

pub mod grammar;
pub mod json_string;
pub mod literal;
pub mod print;
pub mod rows;
pub mod scan;
pub mod tree;
pub mod xml;

/// How a number's lexeme is spelled for a format.
///
/// A number's text is its source lexeme, verbatim: `0xff`, `1_000`, `.5`,
/// `+7`. A printer's instinct is to write it back as it stands, and that is
/// right only when the format being written reads the lexeme back as the
/// same number. It often does not: YAML 1.2 reads `0b1010` as a string,
/// strict JSON refuses `.5` outright. So a printer asks
/// [`spellable`](number::spellable) first and falls back to
/// [`canonical`](number::canonical), decimal, the one spelling every format
/// shares. [`text`](number::text) does both.
pub mod number {
    use std::borrow::Cow;

    use fig_core::kit::number as core_number;

    /// The spellings a format reads back as a number. A field left `false`
    /// means that spelling is canonicalized to decimal before it is written.
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
    #[non_exhaustive]
    pub struct Spelling {
        /// `0x1F`
        pub hex: bool,
        /// `0o17`
        pub octal: bool,
        /// `0b1010`
        pub binary: bool,
        /// `1_000`
        pub underscores: bool,
        /// `0755`, a multi-digit integer part led by a zero
        pub leading_zero: bool,
        /// `.5` and `5.`
        pub bare_dot: bool,
        /// a leading `+`
        pub plus: bool,
    }

    impl Spelling {
        /// Plain decimal alone.
        pub const NONE: Spelling = Spelling {
            hex: false,
            octal: false,
            binary: false,
            underscores: false,
            leading_zero: false,
            bare_dot: false,
            plus: false,
        };
        /// Strict JSON: none of it.
        pub const JSON: Spelling = Spelling::NONE;
        /// JSON5's numbers are ES5.1 numeric literals: hex, a leading `+`,
        /// and bare dots, but not `0o`/`0b`, `_` or a leading zero.
        pub const JSON5: Spelling = Spelling {
            hex: true,
            bare_dot: true,
            plus: true,
            ..Spelling::NONE
        };
        /// YAML 1.2 core: hex and octal, a bare dot, a leading `+`, and a
        /// leading zero (read as decimal); no `0b` and no `_`, which 1.2
        /// reads as strings.
        pub const YAML_1_2: Spelling = Spelling {
            hex: true,
            octal: true,
            leading_zero: true,
            bare_dot: true,
            plus: true,
            ..Spelling::NONE
        };

        /// The core's spelling: the same seven fields, and no non-finite
        /// names (no `FIG_SPELL_*` bit spells them).
        fn to_core(self) -> core_number::Spelling {
            core_number::Spelling {
                hex: self.hex,
                octal: self.octal,
                binary: self.binary,
                underscores: self.underscores,
                leading_zero: self.leading_zero,
                bare_dot: self.bare_dot,
                plus: self.plus,
                non_finite: None,
            }
        }
    }

    /// Whether a format that spells `s` reads `raw` back as the same number.
    pub fn spellable(raw: &str, s: Spelling) -> bool {
        core_number::spellable(raw.as_bytes(), s.to_core())
    }

    /// `raw` as the decimal lexeme every format reads: a radix converted,
    /// `_` and a leading `+` dropped, a bare dot padded, leading zeros
    /// stripped. A lexeme that is not a number at all comes back as it was.
    pub fn canonical(raw: &str) -> String {
        let mut out = Vec::with_capacity(raw.len() + core_number::ANSWER_ROOM);
        core_number::write_canonical(&mut out, raw.as_bytes());
        assert!(
            out.len() <= raw.len() + core_number::ANSWER_ROOM,
            "the canonical number outgrew its answer room"
        );
        // The answer is `raw`'s own characters less some ASCII, or ASCII
        // digits, so it is UTF-8 whenever `raw` is.
        String::from_utf8(out)
            .unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned())
    }

    /// `raw` as a format that spells `s` writes it: itself when
    /// [`spellable`], else [`canonical`].
    pub fn text(raw: &str, s: Spelling) -> Cow<'_, str> {
        if spellable(raw, s) {
            Cow::Borrowed(raw)
        } else {
            Cow::Owned(canonical(raw))
        }
    }

    /// A non-finite float, whatever its spelling.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    pub enum NonFinite {
        /// Infinity.
        Inf,
        /// Negative infinity.
        NegInf,
        /// Not a number.
        NaN,
    }

    /// Which non-finite float `raw` spells, in any format's words (`inf`,
    /// `.inf`, `Infinity`, `nan`, …, with an optional sign), or `None` when
    /// it spells a finite number or none.
    pub fn non_finite(raw: &str) -> Option<NonFinite> {
        match core_number::non_finite(raw.as_bytes())? {
            core_number::NonFinite::Inf => Some(NonFinite::Inf),
            core_number::NonFinite::NegInf => Some(NonFinite::NegInf),
            core_number::NonFinite::Nan => Some(NonFinite::NaN),
        }
    }
}

/// Which datetime a bare token is.
///
/// TOML, YAML and figl all read `2024-01-02`, `12:30:00`,
/// `2024-01-02T12:30:00` and `2024-01-02T12:30:00+01:00` as typed scalars,
/// and validate them the same way: a real calendar date, a time of day with
/// an optional fraction, an offset within a day. What differs per format is
/// only what is allowed, which is [`Options`].
pub mod datetime {
    use crate::ExtKind;
    use fig_core::kit::datetime as core_datetime;

    /// What a datetime may leave out. Both are allowed by default.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    #[non_exhaustive]
    pub struct Options {
        /// `HH:MM` with no seconds, as TOML 1.1 and YAML read and TOML 1.0
        /// does not.
        pub minute_precision: bool,
        /// A time with no date, as TOML reads and YAML 1.1 does not (there a
        /// `:`-run with no date is a sexagesimal number).
        pub time_only: bool,
    }

    impl Default for Options {
        fn default() -> Self {
            Options {
                minute_precision: true,
                time_only: true,
            }
        }
    }

    impl Options {
        fn to_core(self) -> core_datetime::Options {
            core_datetime::Options {
                allow_minute_precision: self.minute_precision,
                allow_time_only: self.time_only,
            }
        }
    }

    /// The datetime `raw` is ([`ExtKind::OffsetDateTime`],
    /// [`LocalDateTime`](ExtKind::LocalDateTime),
    /// [`LocalDate`](ExtKind::LocalDate) or
    /// [`LocalTime`](ExtKind::LocalTime)), or `None` when it is none. The
    /// date and the time are joined by `T`, `t` or a space; an offset is
    /// `Z`, `z` or `±HH:MM`.
    pub fn classify(raw: &str, opts: Options) -> Option<ExtKind> {
        let kind = core_datetime::classify(raw.as_bytes(), opts.to_core()).ok()?;
        // The kind's index is its `ExtKind`'s.
        ExtKind::from_c(kind as i32)
    }

    /// Whether `s` is `YYYY-MM-DD` and a day that exists.
    pub fn valid_date(s: &str) -> bool {
        core_datetime::validate_date(s.as_bytes()).is_ok()
    }

    /// Whether `s` is `HH:MM:SS[.frac]`, or `HH:MM` when `opts` allows it.
    /// A second of 60 is a leap second.
    pub fn valid_time(s: &str, opts: Options) -> bool {
        core_datetime::validate_time(s.as_bytes(), opts.to_core()).is_ok()
    }

    /// Whether `s` is `±HH:MM`, an offset within a day.
    pub fn valid_offset(s: &str) -> bool {
        core_datetime::validate_offset(s.as_bytes()).is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::datetime::{self, Options};
    use super::number::{self, NonFinite, Spelling};
    use crate::ExtKind;

    #[test]
    fn number_spelling() {
        assert!(!number::spellable("0xff", Spelling::JSON));
        assert!(number::spellable("0xff", Spelling::JSON5));
        assert!(number::spellable("0o755", Spelling::YAML_1_2));
        assert!(!number::spellable("1_000", Spelling::YAML_1_2));
        let mut s = Spelling::default();
        s.underscores = true;
        assert!(number::spellable("1_000", s));
        assert_eq!(number::text("0xff", Spelling::JSON), "255");
        assert_eq!(number::text("0xff", Spelling::JSON5), "0xff");
    }

    #[test]
    fn number_canonical() {
        assert_eq!(number::canonical("0xdead_beef"), "3735928559");
        assert_eq!(number::canonical(".5"), "0.5");
        assert_eq!(number::canonical("+5."), "5.0");
        assert_eq!(number::canonical("0xg"), "0xg");
        let long = "9".repeat(100_000);
        assert_eq!(number::canonical(&format!("+{long}")), long);
        assert_eq!(number::canonical("1é"), "1é");
    }

    #[test]
    fn number_non_finite() {
        assert_eq!(number::non_finite("-.inf"), Some(NonFinite::NegInf));
        assert_eq!(number::non_finite("Infinity"), Some(NonFinite::Inf));
        assert_eq!(number::non_finite(".NaN"), Some(NonFinite::NaN));
        assert_eq!(number::non_finite("1e999"), None);
    }

    #[test]
    fn datetime_classify() {
        let d = Options::default();
        assert_eq!(
            datetime::classify("1979-05-27T07:32:00Z", d),
            Some(ExtKind::OffsetDateTime)
        );
        assert_eq!(
            datetime::classify("1979-05-27 07:32:00", d),
            Some(ExtKind::LocalDateTime)
        );
        assert_eq!(
            datetime::classify("1979-05-27", d),
            Some(ExtKind::LocalDate)
        );
        assert_eq!(datetime::classify("07:32:00", d), Some(ExtKind::LocalTime));
        assert_eq!(datetime::classify("2002-02-30", d), None);
        let mut yaml = d;
        yaml.time_only = false;
        assert_eq!(datetime::classify("07:32:00", yaml), None);
        let mut toml_1_0 = d;
        toml_1_0.minute_precision = false;
        assert!(!datetime::valid_time("07:32", toml_1_0));
        assert!(datetime::valid_date("2024-02-29"));
        assert!(!datetime::valid_offset("+24:00"));
    }
}
