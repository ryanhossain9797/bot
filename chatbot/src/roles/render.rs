
use std::io;

use minijinja::{value::Value, Environment, Error, ErrorKind};
use serde::Serialize;

use super::{FormatFlags, RenderInputs};
use crate::chat_format::{ChatMessage, ToolDefinition};

struct Spaced;
impl serde_json::ser::Formatter for Spaced {
    fn begin_array_value<W: ?Sized + io::Write>(
        &mut self,
        w: &mut W,
        first: bool,
    ) -> io::Result<()> {
        if first {
            Ok(())
        } else {
            w.write_all(b", ")
        }
    }
    fn begin_object_key<W: ?Sized + io::Write>(
        &mut self,
        w: &mut W,
        first: bool,
    ) -> io::Result<()> {
        if first {
            Ok(())
        } else {
            w.write_all(b", ")
        }
    }
    fn begin_object_value<W: ?Sized + io::Write>(&mut self, w: &mut W) -> io::Result<()> {
        w.write_all(b": ")
    }
}

fn json_spaced<T: Serialize>(value: &T) -> String {
    let mut buf = Vec::new();
    value
        .serialize(&mut serde_json::Serializer::with_formatter(
            &mut buf, Spaced,
        ))
        .expect("serializing to a Vec is infallible");
    String::from_utf8(buf).expect("serde_json emits valid utf-8")
}

fn prepare_tools(tools: &[ToolDefinition]) -> Vec<String> {
    tools.iter().map(json_spaced).collect()
}

pub(super) fn render(
    template: &str,
    system_prompt: &str,
    inputs: &RenderInputs,
    flags: FormatFlags,
) -> anyhow::Result<String> {
    let messages: Vec<ChatMessage> = std::iter::once(ChatMessage::system(system_prompt))
        .chain(inputs.messages.iter().cloned())
        .collect();

    let tools = inputs.tools.map(prepare_tools);

    let mut env = Environment::new();
    env.set_unknown_method_callback(minijinja_contrib::pycompat::unknown_method_callback);
    env.add_function("raise_exception", |msg: String| -> Result<Value, Error> {
        Err(Error::new(ErrorKind::InvalidOperation, msg))
    });
    env.add_template("chat", template)?;
    let tmpl = env.get_template("chat")?;
    let rendered = tmpl.render(minijinja::context! {
        messages => Value::from_serialize(&messages),
        tools => tools.map_or(Value::UNDEFINED, |t| Value::from_serialize(&t)),
        footer => inputs.footer,
        add_generation_prompt => flags.add_generation_prompt,
        enable_thinking => flags.enable_thinking,
    })?;
    Ok(rendered)
}
