//! `to figl` — render nushell data as the fig authoring dialect.
//!
//! Lossy in the one direction that matters, and unavoidably so: a nushell record
//! has no comment slot, so `open x.figl | update port 8080 | to figl` writes back
//! a file stripped of every comment the original carried. That is a property of
//! the pipeline, not of this command — `to json`/`to yaml` do the same. When the
//! goal is to change one value in a real file, the `fig` CLI's `set`/`comment`
//! subcommands edit in place and preserve every other byte.

use fig::{Format, SerializeOptions};
use nu_plugin::{EngineInterface, EvaluatedCall, SimplePluginCommand};
use nu_protocol::{Category, Example, LabeledError, Signature, SyntaxShape, Type, Value};

use crate::FigPlugin;
use crate::convert::nu_to_fig;

pub struct ToFigl;

impl SimplePluginCommand for ToFigl {
    type Plugin = FigPlugin;

    fn name(&self) -> &str {
        "to figl"
    }

    fn description(&self) -> &str {
        "Convert structured data into .figl (the fig authoring dialect)."
    }

    fn extra_description(&self) -> &str {
        "Comments are not written: nushell values do not carry any. Round-tripping \
         a commented file through `from figl | to figl` therefore drops its \
         comments — use the `fig` CLI to edit such a file in place instead."
    }

    fn signature(&self) -> Signature {
        Signature::build(self.name())
            .input_output_types(vec![(Type::Any, Type::String)])
            // Of fig's serializer options only `width` reaches the fig printer
            // (`pretty` and `indent` are honoured by the JSON/TOML/ZON printers),
            // so it is the only one worth a flag here.
            .named(
                "width",
                SyntaxShape::Int,
                "column budget before a value is written expanded rather than inline",
                Some('w'),
            )
            .category(Category::Formats)
    }

    fn search_terms(&self) -> Vec<&str> {
        vec!["fig", "figl", "config"]
    }

    fn examples(&self) -> Vec<Example<'_>> {
        vec![Example {
            example: r#"{replicas: 2} | to figl"#,
            description: "Render a record as figl",
            result: Some(Value::test_string("replicas = 2\n")),
        }]
    }

    fn run(
        &self,
        _plugin: &FigPlugin,
        _engine: &EngineInterface,
        call: &EvaluatedCall,
        input: &Value,
    ) -> Result<Value, LabeledError> {
        let span = call.head;
        let value = nu_to_fig(input)?;

        let mut options = SerializeOptions::default();
        if let Some(width) = call.get_flag::<i64>("width")? {
            let width = u16::try_from(width).map_err(|_| {
                LabeledError::new("width out of range")
                    .with_label(format!("{width} does not fit in a u16"), span)
            })?;
            options = options.width(width);
        }

        let rendered = value.serialize_with(Format::Fig, options).map_err(|e| {
            LabeledError::new("could not write figl").with_label(e.to_string(), span)
        })?;
        Ok(Value::string(rendered, span))
    }
}
