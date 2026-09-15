use anyhow::Result;
use heck::{AsPascalCase, AsShoutySnakeCase, AsSnakeCase};
use ouisync_api_parser::{
    ComplexEnum, Context, Fields, Item, RequestVariant, SimpleEnum, Struct, Type,
    ToResponseVariantName,
};
use std::{fmt, io::Write};

pub(crate) fn generate(ctx: &Context, out: &mut dyn Write) -> Result<()> {
    writeln!(out, "from __future__ import annotations")?;
    writeln!(out)?;
    writeln!(out, "import typing")?;
    writeln!(out, "from dataclasses import dataclass")?;
    writeln!(out, "from enum import IntEnum")?;
    writeln!(out, "from typing import ClassVar")?;
    writeln!(out)?;

    for (name, item) in &ctx.items {
        match item {
            Item::SimpleEnum(item) => {
                write_simple_enum(out, name, item)?;

                if name == "ErrorCode" {
                    write_exception(out, item)?;
                }
            }
            Item::ComplexEnum(item) => write_complex_enum(out, name, item)?,
            Item::Struct(item) => write_struct(out, name, item)?,
        }
    }

    write_complex_enum(out, "Request", &ctx.request.to_enum())?;
    write_complex_enum(out, "Response", &ctx.response.to_enum())?;

    write_api_class(out, "Session", false, &ctx.request.variants)?;
    write_api_class(out, "Repository", true, &ctx.request.variants)?;
    write_api_class(out, "File", true, &ctx.request.variants)?;
    write_api_class(out, "NetworkSocket", true, &ctx.request.variants)?;
    write_api_class(out, "NetworkStream", true, &ctx.request.variants)?;

    writeln!(out, "class UnexpectedResponse(OuisyncError):")?;
    writeln!(out, "    def __init__(self):")?;
    writeln!(
        out,
        "        super().__init__(ErrorCode.INVALID_DATA, \"unexpected response\")"
    )?;
    writeln!(out)?;

    Ok(())
}

// Wire shape for one struct/variant: named fields -> array, one unnamed field -> transparent,
// no fields -> no payload.
#[derive(Clone, Copy)]
enum Shape {
    Unit,
    Named,
    Unnamed,
}

impl Shape {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Unit => "unit",
            Self::Named => "named",
            Self::Unnamed => "unnamed",
        }
    }
}

fn shape_of(fields: &Fields) -> Shape {
    match fields {
        Fields::Unit => Shape::Unit,
        Fields::Named(_) => Shape::Named,
        Fields::Unnamed(_) => Shape::Unnamed,
    }
}

fn write_simple_enum(out: &mut dyn Write, name: &str, item: &SimpleEnum) -> Result<()> {
    writeln!(out, "class {name}(IntEnum):")?;

    for (variant_name, variant) in &item.variants {
        writeln!(
            out,
            "    {} = {}",
            AsShoutySnakeCase(variant_name),
            variant.value
        )?;
    }

    writeln!(out)?;

    Ok(())
}

fn write_complex_enum(out: &mut dyn Write, name: &str, item: &ComplexEnum) -> Result<()> {
    writeln!(out, "class {name}:")?;
    writeln!(out, "    _variants: ClassVar[dict[str, type]] = {{}}")?;
    writeln!(out)?;

    for (variant_name, variant) in &item.variants {
        let class_name = format!("{name}_{variant_name}");
        let shape = shape_of(&variant.fields);

        writeln!(out, "@dataclass")?;
        writeln!(out, "class {class_name}({name}):")?;
        writeln!(out, "    _tag: ClassVar[str] = {variant_name:?}")?;
        writeln!(out, "    _shape: ClassVar[str] = {:?}", shape.as_str())?;

        match &variant.fields {
            Fields::Unit => (),
            Fields::Unnamed(field) => {
                writeln!(out, "    value: {}", PythonType(&field.ty))?;
            }
            Fields::Named(fields) => {
                for (field_name, field) in fields {
                    writeln!(out, "    {field_name}: {}", PythonType(&field.ty))?;
                }
            }
        }

        writeln!(out)?;
    }

    writeln!(out, "{name}._variants = {{")?;

    for (variant_name, _) in &item.variants {
        writeln!(out, "    {variant_name:?}: {name}_{variant_name},")?;
    }

    writeln!(out, "}}")?;
    writeln!(out)?;

    Ok(())
}

fn write_struct(out: &mut dyn Write, name: &str, item: &Struct) -> Result<()> {
    let shape = shape_of(&item.fields);

    writeln!(out, "@dataclass")?;
    writeln!(out, "class {name}:")?;
    writeln!(out, "    _shape: ClassVar[str] = {:?}", shape.as_str())?;

    match &item.fields {
        Fields::Unit => (),
        Fields::Unnamed(field) => {
            writeln!(out, "    value: {}", PythonType(&field.ty))?;
        }
        Fields::Named(fields) => {
            for (field_name, field) in fields {
                writeln!(out, "    {field_name}: {}", PythonType(&field.ty))?;
            }
        }
    }

    if item.secret {
        writeln!(out, "    def __repr__(self) -> str:")?;
        writeln!(out, "        return f\"{{type(self).__name__}}(******)\"")?;
    }

    writeln!(out)?;

    Ok(())
}

fn write_exception(out: &mut dyn Write, item: &SimpleEnum) -> Result<()> {
    writeln!(out, "class OuisyncError(Exception):")?;
    writeln!(
        out,
        "    def __init__(self, code: \"ErrorCode\", message: str | None = None, sources: list[str] | None = None):"
    )?;
    writeln!(out, "        self.code = code")?;
    writeln!(out, "        self.message = message")?;
    writeln!(out, "        self.sources = sources or []")?;
    writeln!(out, "        super().__init__(message or str(code))")?;
    writeln!(out)?;

    for (variant_name, _) in &item.variants {
        if variant_name == "Ok" || variant_name == "Other" {
            continue;
        }

        writeln!(out, "class OuisyncError_{variant_name}(OuisyncError):")?;
        writeln!(
            out,
            "    def __init__(self, message: str | None = None, sources: list[str] | None = None):"
        )?;
        writeln!(
            out,
            "        super().__init__(ErrorCode.{}, message, sources)",
            AsShoutySnakeCase(variant_name)
        )?;
        writeln!(out)?;
    }

    writeln!(
        out,
        "def dispatch_error(code: \"ErrorCode\", message: str | None = None, sources: list[str] | None = None) -> OuisyncError:"
    )?;
    writeln!(out, "    variant = _ERROR_VARIANTS.get(code)")?;
    writeln!(out, "    if variant is not None:")?;
    writeln!(out, "        return variant(message, sources)")?;
    writeln!(out, "    return OuisyncError(code, message, sources)")?;
    writeln!(out)?;

    writeln!(out, "_ERROR_VARIANTS: dict[ErrorCode, type] = {{")?;

    for (variant_name, _) in &item.variants {
        if variant_name == "Ok" || variant_name == "Other" {
            continue;
        }

        writeln!(
            out,
            "    ErrorCode.{}: OuisyncError_{variant_name},",
            AsShoutySnakeCase(variant_name)
        )?;
    }

    writeln!(out, "}}")?;
    writeln!(out)?;

    Ok(())
}

fn write_api_class(
    out: &mut dyn Write,
    name: &str,
    handle: bool,
    request_variants: &[(String, RequestVariant)],
) -> Result<()> {
    writeln!(out, "class {name}:")?;

    if handle {
        writeln!(
            out,
            "    def __init__(self, client: \"Client\", handle: \"{name}Handle\"):"
        )?;
        writeln!(out, "        self._client = client")?;
        writeln!(out, "        self._handle = handle")?;
    } else {
        writeln!(out, "    def __init__(self, client: \"Client\"):")?;
        writeln!(out, "        self._client = client")?;
    }

    writeln!(out)?;

    let prefix = format!("{}_", AsSnakeCase(name));
    let mut any_method = false;

    for (variant_name, variant) in request_variants {
        if variant.skip {
            continue;
        }

        // Streams are handled by hand-written wrapper functions instead.
        if variant.ret_stream_item.is_some() {
            continue;
        }

        let Some(op_name) = variant_name.strip_prefix(&prefix) else {
            continue;
        };

        any_method = true;

        let ret = match &variant.ret {
            Type::Result(ty, _) => ty.as_ref(),
            ty => ty,
        };
        let ret_stripped = ret.strip_suffix("Handle");
        let response_variant_name = ret.to_response_variant_name();
        let request_class = format!("Request_{}", AsPascalCase(variant_name));

        writeln!(out, "    async def {op_name}(")?;
        writeln!(out, "        self,")?;

        let remaining_fields = variant.fields.len().saturating_sub(if handle { 1 } else { 0 });

        // Keyword-only: defaulted fields aren't guaranteed to trail non-defaulted ones.
        if remaining_fields > 0 {
            writeln!(out, "        *,")?;
        }

        for (index, (arg_name, field)) in variant.fields.iter().enumerate() {
            if index == 0 && handle {
                continue;
            }

            let arg_name = arg_name.unwrap_or("value");

            write!(out, "        {arg_name}: {}", PythonType(&field.ty))?;

            match &field.ty {
                Type::Option(_) => write!(out, " = None")?,
                Type::Scalar(s) if s == "bool" => write!(out, " = False")?,
                _ => (),
            }

            writeln!(out, ",")?;
        }

        write!(out, "    )")?;

        match ret {
            Type::Unit => (),
            _ => {
                let ty = ret_stripped.as_ref().unwrap_or(ret);
                write!(out, " -> \"{}\"", PythonType(ty))?;
            }
        }

        writeln!(out, ":")?;

        write!(out, "        request = {request_class}(")?;

        if !variant.fields.is_empty() {
            writeln!(out)?;

            for (index, (arg_name, _)) in variant.fields.iter().enumerate() {
                let arg_name = arg_name.unwrap_or("value");

                if index == 0 && handle {
                    writeln!(out, "            self._handle,")?;
                    continue;
                }

                writeln!(out, "            {arg_name},")?;
            }

            writeln!(out, "        )")?;
        } else {
            writeln!(out, ")")?;
        }

        writeln!(out, "        response = await self._client.invoke(request)")?;

        match ret {
            Type::Unit => {
                writeln!(out, "        if isinstance(response, Response_Unit):")?;
                writeln!(out, "            return")?;
            }
            Type::Option(_) => {
                writeln!(
                    out,
                    "        if isinstance(response, Response_{response_variant_name}):"
                )?;
                write!(out, "            return ")?;

                match ret_stripped {
                    Some(Type::Option(w)) => {
                        writeln!(out, "{}(self._client, response.value)", w)?;
                    }
                    _ => writeln!(out, "response.value")?,
                }

                writeln!(out, "        if isinstance(response, Response_None):")?;
                writeln!(out, "            return None")?;
            }
            _ => {
                writeln!(
                    out,
                    "        if isinstance(response, Response_{response_variant_name}):"
                )?;
                write!(out, "            return ")?;

                match ret_stripped {
                    Some(Type::Scalar(w)) => writeln!(out, "{w}(self._client, response.value)")?,
                    Some(Type::Vec(w)) => {
                        writeln!(out, "[{w}(self._client, item) for item in response.value]")?
                    }
                    Some(Type::Map(_, w)) => writeln!(
                        out,
                        "{{k: {w}(self._client, v) for k, v in response.value.items()}}"
                    )?,
                    _ => writeln!(out, "response.value")?,
                }
            }
        }

        writeln!(out, "        raise UnexpectedResponse()")?;
        writeln!(out)?;
    }

    if !any_method {
        writeln!(out, "    pass")?;
        writeln!(out)?;
    }

    writeln!(out)?;

    Ok(())
}

struct PythonType<'a>(&'a Type);

impl fmt::Display for PythonType<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            Type::Unit => write!(f, "None"),
            Type::Scalar(s) => write!(f, "{}", PythonScalar(s)),
            Type::Option(s) => write!(f, "{} | None", PythonScalar(s)),
            Type::Vec(s) => write!(f, "list[{}]", PythonScalar(s)),
            Type::Map(k, v) => write!(f, "dict[{}, {}]", PythonScalar(k), PythonScalar(v)),
            Type::Bytes => write!(f, "bytes"),
            Type::Result(..) => Err(fmt::Error),
        }
    }
}

struct PythonScalar<'a>(&'a str);

impl fmt::Display for PythonScalar<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            "u8" | "u16" | "u32" | "u64" | "i8" | "i16" | "i32" | "i64" | "usize" | "isize" => {
                write!(f, "int")
            }
            "bool" => write!(f, "bool"),
            "PathBuf" | "PeerAddr" | "SocketAddr" | "String" | "ShareToken" => write!(f, "str"),
            // Wire-encoded as raw milliseconds (helpers::millis).
            "Duration" => write!(f, "int"),
            // No confirmed wire representation.
            "SystemTime" | "StateMonitor" => write!(f, "typing.Any"),
            _ => write!(f, "{}", self.0),
        }
    }
}
