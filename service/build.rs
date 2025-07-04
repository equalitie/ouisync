use std::{
    env,
    fs::File,
    io::{self, Write},
    path::{Path, PathBuf},
};

use anyhow::Result;
use heck::AsPascalCase;
use ouisync_api_parser::{Context, Field, Fields, Request, Response, Type};

fn main() {
    if let Err(error) = generate() {
        println!("cargo::error={:#}", error);
    }
}

fn generate() -> Result<()> {
    let ctx = Context::parse(&["src/lib.rs"])?;

    generate_request(&ctx.request)?;
    generate_response(&ctx.response)?;
    generate_dispatch(&ctx.request)?;

    Ok(())
}

fn output_path(name: &str) -> PathBuf {
    Path::new(&env::var("OUT_DIR").unwrap())
        .join(name)
        .with_extension("rs")
}

fn generate_request(request: &Request) -> Result<()> {
    let mut out = File::create(output_path("request"))?;

    write_file_header(&mut out)?;

    writeln!(
        out,
        "#[derive(Eq, PartialEq, Debug, Serialize, Deserialize)]"
    )?;
    writeln!(out, "pub enum Request {{")?;

    for (name, variant) in &request.variants {
        for line in &variant.docs.lines {
            writeln!(out, "{I}///{line}")?;
        }

        write!(out, "{I}{}", AsPascalCase(name))?;

        match &variant.fields {
            Fields::Named(fields) => {
                writeln!(out, " {{")?;

                for (name, field) in fields {
                    if let Some(s) = field.ty.serde_with() {
                        writeln!(out, "{I}{I}#[serde(with = \"{s}\")]")?;
                    }

                    writeln!(out, "{I}{I}{name}: {},", field.ty)?;
                }

                write!(out, "{I}}}")?;
            }
            Fields::Unnamed(Field { ty, .. }) => {
                write!(out, "(")?;

                if let Some(s) = ty.serde_with() {
                    write!(out, "#[serde(with = \"{s}\")]")?;
                }

                write!(out, "{ty})")?;
            }
            Fields::Unit => (),
        }

        writeln!(out, ",")?;
    }

    writeln!(out, "}}")?;

    Ok(())
}

fn generate_response(response: &Response) -> Result<()> {
    let mut out = File::create(output_path("response"))?;

    write_file_header(&mut out)?;

    writeln!(
        out,
        "#[derive(Eq, PartialEq, Debug, Serialize, Deserialize)]"
    )?;
    writeln!(out, "#[allow(clippy::large_enum_variant)]")?;
    writeln!(out, "pub enum Response {{")?;

    // generate the `Response` enum
    for (name, ty) in &response.variants {
        write!(out, "{I}{name}")?;

        match ty {
            Type::Unit => (),
            _ => {
                write!(out, "(")?;

                if let Some(s) = ty.serde_with() {
                    write!(out, "#[serde(with = \"{s}\")] ")?;
                }

                write!(out, "{ty})")?;
            }
        }
        writeln!(out, ",")?;
    }

    writeln!(out, "}}")?;

    writeln!(out)?;

    // generate `impl From<T> for Response`, `impl TryFrom<Response> for T` and `impl TryFrom<Response> for Option<T>`
    for (name, ty) in &response.variants {
        if matches!(ty, Type::Unit) {
            continue;
        }

        writeln!(out, "impl From<{ty}> for Response {{")?;
        writeln!(out, "{I}fn from(value: {ty}) -> Self {{")?;
        writeln!(out, "{I}{I}Self::{name}(value)")?;
        writeln!(out, "{I}}}")?;
        writeln!(out, "}}")?;
        writeln!(out)?;

        writeln!(out, "impl TryFrom<Response> for {ty} {{")?;
        writeln!(out, "{I}type Error = UnexpectedResponse;")?;
        writeln!(
            out,
            "{I}fn try_from(response: Response) -> Result<Self, Self::Error> {{"
        )?;
        writeln!(out, "{I}{I}match response {{")?;
        writeln!(out, "{I}{I}{I}Response::{name}(value) => Ok(value),")?;
        writeln!(out, "{I}{I}{I}_ => Err(UnexpectedResponse),")?;
        writeln!(out, "{I}{I}}}")?;
        writeln!(out, "{I}}}")?;
        writeln!(out, "}}")?;
        writeln!(out)?;

        writeln!(out, "impl TryFrom<Response> for Option<{ty}> {{")?;
        writeln!(out, "{I}type Error = UnexpectedResponse;")?;
        writeln!(
            out,
            "{I}fn try_from(value: Response) -> Result<Self, Self::Error> {{"
        )?;
        writeln!(out, "{I}{I}match value {{")?;
        writeln!(out, "{I}{I}{I}Response::{name}(value) => Ok(Some(value)),")?;
        writeln!(out, "{I}{I}{I}Response::None => Ok(None),")?;
        writeln!(out, "{I}{I}{I}_ => Err(UnexpectedResponse),")?;
        writeln!(out, "{I}{I}}}")?;
        writeln!(out, "}}")?;
        writeln!(out, "}}")?;
        writeln!(out)?;
    }

    Ok(())
}

fn generate_dispatch(request: &Request) -> Result<()> {
    let mut out = File::create(output_path("connection"))?;

    write_file_header(&mut out)?;

    writeln!(out, "#[allow(clippy::let_unit_value)]")?;
    writeln!(out, "async fn dispatch(")?;
    writeln!(out, "{I}state: &State,")?;
    writeln!(out, "{I}request: Request,")?;
    writeln!(out, ") -> Result<Action<Response>, ProtocolError> {{")?;

    writeln!(out, "{I}match request {{")?;

    for (name, variant) in &request.variants {
        write!(out, "{I}{I}Request::{}", AsPascalCase(name))?;

        match &variant.fields {
            Fields::Named(fields) => {
                writeln!(out, " {{")?;

                for (name, _) in fields {
                    writeln!(out, "{I}{I}{I}{name},")?;
                }

                write!(out, "{I}{I}}}")?;
            }
            Fields::Unnamed(_) => {
                writeln!(out, "(value)")?;
            }
            Fields::Unit => (),
        }

        writeln!(out, " => {{")?;
        writeln!(out, "{I}{I}{I}let ret = state.{}(", name)?;

        for (name, field) in &variant.fields {
            if matches!(field.ty, Type::Unit) {
                continue;
            }

            write!(out, "{I}{I}{I}{I}{}", name.unwrap_or("value"))?;

            if matches!(field.ty, Type::Bytes) {
                write!(out, ".into()")?;
            }

            writeln!(out, ",")?;
        }

        writeln!(
            out,
            "{I}{I}{I}){}{};",
            if variant.is_async { ".await" } else { "" },
            match variant.ret {
                Type::Result(..) => "?",
                _ => "",
            },
        )?;

        writeln!(out, "{I}{I}{I}")?;
        writeln!(out, "{I}{I}{I}Ok(ret.into())")?;

        writeln!(out, "{I}{I}}}")?;
    }

    writeln!(out, "{I}}}")?;
    writeln!(out, "}}")?;

    Ok(())
}

fn write_file_header(w: &mut impl Write) -> io::Result<()> {
    writeln!(w, "// This file is generated by the build script.")?;
    writeln!(w)?;

    Ok(())
}

const I: &str = "    ";
