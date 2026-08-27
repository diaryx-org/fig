//! `from figl` — parse the fig authoring dialect into nushell data.
//!
//! Registering this command is also what teaches `open` about the format:
//! nushell dispatches `open foo.figl` to whatever `from figl` is in scope, so
//! installing the plugin makes `open` on a `.figl` file return a record with no
//! further setup.

use chrono::{TimeZone, Utc};
use fig::{Document, Format};
use nu_plugin::{EngineInterface, EvaluatedCall, SimplePluginCommand};
use nu_protocol::{Category, Example, LabeledError, Signature, Span, Type, Value};

use crate::FigPlugin;
use crate::convert::fig_to_nu;

pub struct FromFigl;

impl SimplePluginCommand for FromFigl {
    type Plugin = FigPlugin;

    fn name(&self) -> &str {
        "from figl"
    }

    fn description(&self) -> &str {
        "Parse .figl (the fig authoring dialect) into structured data."
    }

    fn extra_description(&self) -> &str {
        "Comments are not carried into the result: a nushell record has nowhere to \
         put them. To edit a .figl file without losing its comments, use the `fig` \
         CLI (`fig set` / `fig comment`), which rewrites the file in place and \
         leaves every other byte untouched."
    }

    fn signature(&self) -> Signature {
        Signature::build(self.name())
            // Binary as well as String: `open` hands over raw bytes whenever it
            // cannot establish that a file is text, and a .figl file that opens
            // as binary should still parse rather than reporting a type error.
            .input_output_types(vec![(Type::String, Type::Any), (Type::Binary, Type::Any)])
            .category(Category::Formats)
    }

    fn search_terms(&self) -> Vec<&str> {
        vec!["fig", "figl", "config", "frontmatter"]
    }

    fn examples(&self) -> Vec<Example<'_>> {
        vec![
            Example {
                example: r#"'replicas = 2' | from figl"#,
                description: "Parse figl into a record",
                result: Some(Value::test_record(nu_protocol::record! {
                    "replicas" => Value::test_int(2),
                })),
            },
            // The point of the plugin in one line: a JSON hop would make this
            // a string, because JSON has no date type to carry it in.
            Example {
                example: r#"'released = 2026-05-08' | from figl"#,
                description: "figl datetimes arrive as nushell datetimes, not strings",
                result: Some(Value::test_record(nu_protocol::record! {
                    "released" => Value::test_date(
                        Utc.with_ymd_and_hms(2026, 5, 8, 0, 0, 0).unwrap().fixed_offset(),
                    ),
                })),
            },
        ]
    }

    fn run(
        &self,
        _plugin: &FigPlugin,
        _engine: &EngineInterface,
        _call: &EvaluatedCall,
        input: &Value,
    ) -> Result<Value, LabeledError> {
        let span = input.span();
        let bytes: &[u8] = match input {
            Value::String { val, .. } => val.as_bytes(),
            Value::Binary { val, .. } => val,
            other => {
                return Err(LabeledError::new("expected string or binary input")
                    .with_label(format!("got {}", other.get_type()), span));
            }
        };

        let document = Document::parse(bytes, Format::Fig).map_err(|e| parse_error(e, span))?;
        let value = document.to_value().map_err(|e| parse_error(e, span))?;
        Ok(fig_to_nu(&value, span))
    }
}

/// Turn a fig error into a nushell one.
///
/// The label covers the whole input rather than the offending byte: fig's
/// `ParseError` carries `line`/`column`/`byte_offset` fields, but the core does
/// not yet fill them through the C ABI (they are wired for when it does), and a
/// nushell span is an offset into engine source — for `open foo.figl` that is
/// the `open` call itself, not the file's bytes, so a byte offset from inside
/// the file could not be added to it meaningfully anyway.
fn parse_error(error: fig::Error, span: Span) -> LabeledError {
    let mut label = LabeledError::new("could not parse figl").with_label(error.to_string(), span);
    if let fig::Error::Parse(parse) = &error
        && let (Some(line), Some(column)) = (parse.line, parse.column)
    {
        label = label.with_help(format!("at line {line}, column {column}"));
    }
    label
}
