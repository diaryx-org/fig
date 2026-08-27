//! `fig::Value` <-> `nu_protocol::Value`.
//!
//! This is the whole reason the plugin exists rather than a `def "from figl"`
//! shim that pipes through `fig get -o json`. A carrier format can only forward
//! what *it* can represent, and no format fig emits carries everything nushell
//! can hold:
//!
//!   * through JSON, a figl datetime arrives as a string (JSON has no date type);
//!   * through TOML, a figl `null` **disappears along with its key** (TOML has no
//!     null) — `figl/homebrew.figl`'s `on.workflow_dispatch` is a live example;
//!   * through YAML, both losses of the JSON path apply, since nushell's
//!     `from yaml` does not resolve timestamps either.
//!
//! Reading `fig::Value` directly sidesteps the carrier entirely: `Null` becomes
//! `nothing` and `Extended`-kind datetimes become real `datetime`s in the same
//! pass.

use chrono::{DateTime, FixedOffset, NaiveDate, NaiveDateTime, TimeZone, Utc};
use fig::{ExtKind, Value as FigValue};
use nu_protocol::{LabeledError, Record, Span, Value as NuValue};

/// Lower a parsed fig tree into nushell values.
///
/// Infallible by construction: every shape fig can produce has *some* nushell
/// spelling, and the cases with no exact counterpart (a bare clock time, an
/// unrecognized future `ExtKind`) fall back to the value's verbatim source text
/// rather than failing the document. `span` is the span of the input the tree
/// was parsed from — fig reports positions into its own source, which is not a
/// nushell source file, so every produced value carries the input's span.
pub fn fig_to_nu(value: &FigValue, span: Span) -> NuValue {
    match value {
        FigValue::Null => NuValue::nothing(span),
        FigValue::Bool(b) => NuValue::bool(*b, span),
        FigValue::Int(i) => NuValue::int(*i, span),
        // nushell's `int` is an i64, so past i64::MAX there is no lossless
        // numeric landing spot. A string keeps every digit; a float would keep
        // the type and quietly round the value, which is the worse trade for a
        // config file (an ID or a bitmask is exactly the thing that lives up
        // there, and exactly the thing rounding ruins).
        FigValue::Uint(u) => match i64::try_from(*u) {
            Ok(i) => NuValue::int(i, span),
            Err(_) => NuValue::string(u.to_string(), span),
        },
        FigValue::Float(f) => NuValue::float(*f, span),
        FigValue::Str(s) => NuValue::string(s.clone(), span),
        FigValue::Extended { kind, text } => extended_to_nu(*kind, text, span),
        FigValue::Seq(items) => {
            NuValue::list(items.iter().map(|v| fig_to_nu(v, span)).collect(), span)
        }
        FigValue::Map(entries) => {
            let mut record = Record::with_capacity(entries.len());
            for (key, val) in entries {
                // `insert`, not `push`: a duplicate column makes a record that
                // most nushell operations then disagree about. figl rejects
                // duplicate keys at parse time anyway, so this only bites when
                // two distinct non-string keys stringify alike.
                record.insert(key_to_column(key), fig_to_nu(val, span));
            }
            NuValue::record(record, span)
        }
    }
}

/// A format-specific scalar, carried by fig as `(kind, verbatim text)`.
///
/// The datetime arms are the plugin's headline: they are what a JSON hop cannot
/// give you. Where a shape has no nushell type at all, the verbatim text is
/// returned — losing the *type* but never the *value*.
fn extended_to_nu(kind: ExtKind, text: &str, span: Span) -> NuValue {
    let dated = |dt: Option<DateTime<FixedOffset>>| match dt {
        Some(dt) => NuValue::date(dt, span),
        // fig produced this kind, so the text parsed as a datetime once
        // already; if chrono still disagrees, hand back the source text rather
        // than inventing an instant.
        None => NuValue::string(text, span),
    };

    match kind {
        ExtKind::OffsetDateTime => dated(parse_offset_datetime(text)),
        // A local datetime names no zone, so an instant has to be assumed.
        // UTC is the assumption nushell's own `from toml` makes (verified:
        // `"c = 2026-05-08T07:32:00" | from toml` yields `+00:00`), and
        // agreeing with the shell's existing behaviour beats being novel.
        ExtKind::LocalDateTime => dated(parse_local_datetime(text).map(to_utc)),
        // Likewise midnight UTC, matching `from toml` on a bare TOML date.
        ExtKind::LocalDate => dated(parse_local_date(text).map(to_utc)),
        // nushell has no time-of-day type — `duration` is an elapsed span, not
        // a clock reading, so `10:30` would become "10 hours 30 minutes" and
        // mean something else. The text is the honest answer, and it is what
        // `from toml` returns too.
        ExtKind::LocalTime => NuValue::string(text, span),
        // `.foo` in ZON; `@enum "foo"` in figl. fig hands over the bare name.
        ExtKind::EnumLiteral => NuValue::string(text, span),
        ExtKind::CharLiteral => char_to_nu(text, span),
        // `Infinity`/`-Infinity`/`NaN` — all three are ordinary nushell floats.
        // fig's own lexer is reused so this agrees with how fig read it.
        ExtKind::NumberSpecial => match FigValue::parse_float(text) {
            Some(f) => NuValue::float(f, span),
            None => NuValue::string(text, span),
        },
        // `ExtKind` is `#[non_exhaustive]`: a newer core may carry kinds this
        // build has never heard of. Passing the verbatim text through keeps
        // such a document readable instead of erroring on it.
        _ => NuValue::string(text, span),
    }
}

/// fig documents `CharLiteral`'s text as the decimal codepoint, so render the
/// character it denotes; nushell has no char type, and a one-character string is
/// the closest thing users can actually work with.
fn char_to_nu(text: &str, span: Span) -> NuValue {
    match text.parse::<u32>().ok().and_then(char::from_u32) {
        Some(c) => NuValue::string(c.to_string(), span),
        None => NuValue::string(text, span),
    }
}

fn to_utc(naive: NaiveDateTime) -> DateTime<FixedOffset> {
    Utc.from_utc_datetime(&naive).fixed_offset()
}

fn parse_offset_datetime(text: &str) -> Option<DateTime<FixedOffset>> {
    // RFC 3339 permits a space in place of the `T` separator, and figl (like
    // TOML) accepts that spelling; chrono's rfc3339 parser does not.
    DateTime::parse_from_rfc3339(&text.replacen(' ', "T", 1)).ok()
}

fn parse_local_datetime(text: &str) -> Option<NaiveDateTime> {
    let text = text.replacen(' ', "T", 1);
    // `%.f` matches an *optional* fractional part, so this covers both
    // `…T07:32:00` and `…T07:32:00.123`. The secondless spelling is a separate
    // pattern because `%S` is not optional.
    NaiveDateTime::parse_from_str(&text, "%Y-%m-%dT%H:%M:%S%.f")
        .or_else(|_| NaiveDateTime::parse_from_str(&text, "%Y-%m-%dT%H:%M"))
        .ok()
}

fn parse_local_date(text: &str) -> Option<NaiveDateTime> {
    NaiveDate::parse_from_str(text, "%Y-%m-%d")
        .ok()?
        .and_hms_opt(0, 0, 0)
}

/// Flatten a mapping key to a record column.
///
/// nushell records are string-keyed, so a non-string key has to be spelled as
/// text somehow. figl keys are barekeys or quoted strings, so in practice only
/// the first arm ever runs; the rest exist because `fig::Value` is a
/// format-agnostic tree that other fig frontends can put a scalar key into.
fn key_to_column(key: &FigValue) -> String {
    match key {
        FigValue::Str(s) => s.clone(),
        FigValue::Bool(b) => b.to_string(),
        FigValue::Int(i) => i.to_string(),
        FigValue::Uint(u) => u.to_string(),
        FigValue::Float(f) => f.to_string(),
        FigValue::Extended { text, .. } => text.clone(),
        FigValue::Null => "null".to_string(),
        // A collection as a key: no sensible flat spelling, and no format fig
        // reads produces one. Render it as its fig source so the column is at
        // least identifiable rather than empty.
        other => other
            .serialize(fig::Format::Fig)
            .unwrap_or_default()
            .trim()
            .to_string(),
    }
}

/// Raise nushell values into a fig tree for serialization.
///
/// Fallible where `fig_to_nu` is not, because nushell's value space is the
/// larger one: it has closures, ranges, cell paths and errors, none of which are
/// configuration data and none of which fig can hold.
pub fn nu_to_fig(value: &NuValue) -> Result<FigValue, LabeledError> {
    Ok(match value {
        NuValue::Nothing { .. } => FigValue::Null,
        NuValue::Bool { val, .. } => FigValue::Bool(*val),
        NuValue::Int { val, .. } => FigValue::Int(*val),
        NuValue::Float { val, .. } => FigValue::Float(*val),
        NuValue::String { val, .. } => FigValue::Str(val.clone()),
        NuValue::Glob { val, .. } => FigValue::Str(val.clone()),
        // figl has a native datetime literal, so this round-trips as a datetime
        // rather than as a quoted string.
        NuValue::Date { val, .. } => FigValue::Extended {
            kind: ExtKind::OffsetDateTime,
            text: val.to_rfc3339(),
        },
        // Nanoseconds and bytes respectively — the same base units nushell's own
        // `to json` emits, so `to figl | from figl` agrees with the rest of the
        // shell about what a duration degrades to.
        NuValue::Duration { val, .. } => FigValue::Int(*val),
        NuValue::Filesize { val, .. } => FigValue::Int(val.get()),
        NuValue::List { vals, .. } => {
            FigValue::Seq(vals.iter().map(nu_to_fig).collect::<Result<_, _>>()?)
        }
        NuValue::Record { val, .. } => {
            let mut entries = Vec::with_capacity(val.len());
            for (col, item) in val.iter() {
                entries.push((FigValue::Str(col.clone()), nu_to_fig(item)?));
            }
            FigValue::Map(entries)
        }
        other => {
            return Err(
                LabeledError::new(format!("cannot write a {} to figl", other.get_type()))
                    .with_label("unsupported value", other.span())
                    .with_help(
                        "figl holds configuration data: null, booleans, numbers, strings, \
                 datetimes, lists and records.",
                    ),
            );
        }
    })
}
