use anyhow::{bail, Error, Result};
use heck::{AsLowerCamelCase, AsPascalCase, ToLowerCamelCase, ToSnakeCase};
use ouisync_api_parser::{
    ComplexEnum, Context, Docs, Field, Item, Newtype, RequestVariant, SimpleEnum, Struct,
    ToResponseVariantName, Type,
};
use std::{borrow::Cow, fmt, io::Write};

pub(crate) fn generate(ctx: &Context, out: &mut dyn Write) -> Result<()> {
    writeln!(out, "{}", include_str!("support.dart"))?;

    for (name, item) in &ctx.items {
        match item {
            Item::SimpleEnum(item) => {
                write_simple_enum(out, name, item)?;

                if name == "ErrorCode" {
                    write_exception(out, item)?;
                }
            }
            Item::ComplexEnum(item) => write_complex_enum(out, name, item, true, true)?,
            Item::Struct(item) => write_struct(out, name, item)?,
            Item::Newtype(item) => write_newtype(out, name, item)?,
        }
    }

    write_complex_enum(out, "Request", &ctx.request.to_enum(), true, false)?;
    write_complex_enum(out, "Response", &ctx.response.to_enum(), false, true)?;

    write_api_class(out, "Session", None, &ctx.request.variants)?;
    write_api_class(
        out,
        "Repository",
        Some(("handle", "RepositoryHandle")),
        &ctx.request.variants,
    )?;
    write_api_class(
        out,
        "ShareToken",
        Some(("value", "String")),
        &ctx.request.variants,
    )?;
    write_api_class(
        out,
        "File",
        Some(("handle", "FileHandle")),
        &ctx.request.variants,
    )?;

    Ok(())
}

fn write_simple_enum(out: &mut dyn Write, name: &str, item: &SimpleEnum) -> Result<()> {
    write_docs(out, "", &item.docs)?;
    writeln!(out, "enum {name} {{")?;

    for (name, variant) in &item.variants {
        write_docs(out, I, &variant.docs)?;
        writeln!(out, "{I}{},", AsLowerCamelCase(name))?;
    }

    writeln!(out, "{I};")?;
    writeln!(out)?;

    // fromInt
    writeln!(out, "{I}static {name}? fromInt(int n) {{")?;
    writeln!(out, "{I}{I}switch (n) {{")?;

    for (variant_name, variant) in &item.variants {
        writeln!(
            out,
            "{I}{I}{I}case {}: return {}.{};",
            variant.value,
            name,
            AsLowerCamelCase(variant_name)
        )?;
    }

    writeln!(out, "{I}{I}{I}default: return null;")?;
    writeln!(out, "{I}{I}}}")?;

    writeln!(out, "{I}}}")?;
    writeln!(out)?;

    // decode
    writeln!(out, "{I}static {name}? decode(Unpacker u) {{")?;
    writeln!(out, "{I}{I}final n = u.unpackInt();")?;
    writeln!(out, "{I}{I}return n != null ? fromInt(n) : null;")?;
    writeln!(out, "{I}}}")?;
    writeln!(out)?;

    // encode
    writeln!(out, "{I}void encode(Packer p) {{")?;
    writeln!(out, "{I}{I}switch (this) {{")?;

    for (variant_name, variant) in &item.variants {
        writeln!(
            out,
            "{I}{I}{I}case {}.{}: p.packInt({});",
            name,
            AsLowerCamelCase(variant_name),
            variant.value
        )?;
    }

    writeln!(out, "{I}{I}}}")?;
    writeln!(out, "{I}}}")?;
    writeln!(out)?;

    writeln!(out, "}}")?;
    writeln!(out)?;

    Ok(())
}

fn write_complex_enum(
    out: &mut dyn Write,
    name: &str,
    item: &ComplexEnum,
    encode: bool,
    decode: bool,
) -> Result<()> {
    write_docs(out, "", &item.docs)?;
    writeln!(out, "sealed class {name} {{")?;

    if encode {
        writeln!(out, "{I}void encode(Packer p) {{")?;
        writeln!(out, "{I}{I}switch (this) {{")?;

        for (variant_name, variant) in &item.variants {
            writeln!(out, "{I}{I}{I}case {name}{variant_name}(")?;

            for (field_name, _) in &variant.fields {
                writeln!(
                    out,
                    "{I}{I}{I}{I}{0}: final {0},",
                    AsLowerCamelCase(field_name)
                )?;
            }

            writeln!(out, "{I}{I}{I}):")?;
            writeln!(
                out,
                "{I}{I}{I}{I}p.packListLength({});",
                variant.fields.len()
            )?;

            for (field_name, field) in &variant.fields {
                write!(out, "{I}{I}{I}{I}")?;
                DartType::try_from(&field.ty)?
                    .write_encode(out, &field_name.to_lower_camel_case())?;
                writeln!(out, ";")?;
            }
        }

        writeln!(out, "{I}{I}}}")?;
        writeln!(out, "{I}}}")?;
        writeln!(out, "{I}")?;
    }

    if decode {
        writeln!(out, "{I}static {name}? decode(Unpacker u) {{")?;
        writeln!(out, "{I}{I}try {{")?;
        writeln!(out, "{I}{I}{I}switch (u.unpackString()) {{")?;

        for (variant_name, variant) in &item.variants {
            if !variant.fields.is_empty() {
                continue;
            }

            writeln!(
                out,
                "{I}{I}{I}{I}case \"{variant_name}\": return {name}{variant_name}();"
            )?;
        }

        writeln!(out, "{I}{I}{I}{I}case null: return null;")?;
        writeln!(out, "{I}{I}{I}{I}default: {THROW_DECODE_ERROR};")?;

        writeln!(out, "{I}{I}{I}}}")?;
        writeln!(out, "{I}{I}}} on FormatException {{")?;
        writeln!(
            out,
            "{I}{I}{I}if (u.unpackMapLength() != 1) {THROW_DECODE_ERROR};"
        )?;
        writeln!(out, "{I}{I}{I}switch (u.unpackString()) {{")?;

        for (variant_name, variant) in &item.variants {
            if variant.fields.is_empty() {
                continue;
            }

            writeln!(out, "{I}{I}{I}{I}case \"{variant_name}\":")?;
            writeln!(
                out,
                "{I}{I}{I}{I}{I}if (u.unpackListLength() != {}) {THROW_DECODE_ERROR};",
                variant.fields.len()
            )?;
            writeln!(out, "{I}{I}{I}{I}{I}return {name}{variant_name}(")?;

            for (field_name, field) in &variant.fields {
                write!(out, "{I}{I}{I}{I}{I}{I}{}: ", AsLowerCamelCase(field_name))?;
                DartType::try_from(&field.ty)?.write_decode(out)?;
                writeln!(out, ",")?;
            }

            writeln!(out, "{I}{I}{I}{I}{I});")?;
        }

        writeln!(out, "{I}{I}{I}{I}case null: return null;")?;
        writeln!(out, "{I}{I}{I}{I}default: {THROW_DECODE_ERROR};")?;

        writeln!(out, "{I}{I}{I}}}")?;

        writeln!(out, "{I}{I}}}")?;
        writeln!(out, "{I}}}")?;
        writeln!(out, "{I}")?;
    }

    writeln!(out, "}}")?;
    writeln!(out)?;

    for (variant_name, variant) in &item.variants {
        let full_name = format!("{name}{variant_name}");

        write_docs(out, "", &variant.docs)?;
        writeln!(out, "class {full_name} extends {name} {{")?;
        write_class_body(out, &full_name, &variant.fields)?;
        writeln!(out, "}}")?;
        writeln!(out)?;
    }

    Ok(())
}

fn write_struct(out: &mut dyn Write, name: &str, item: &Struct) -> Result<()> {
    write_docs(out, "", &item.docs)?;
    writeln!(out, "class {name} {{")?;

    write_class_body(out, name, &item.fields)?;
    writeln!(out, "{I}")?;

    // encode
    writeln!(out, "{I}void encode(Packer p) {{")?;

    writeln!(out, "{I}{I}p.packListLength({});", item.fields.len())?;

    for (field_name, field) in &item.fields {
        write!(out, "{I}{I}")?;
        DartType::try_from(&field.ty)?.write_encode(out, &field_name.to_lower_camel_case())?;
        writeln!(out, ";")?;
    }

    writeln!(out, "{I}}}")?;
    writeln!(out, "{I}")?;

    // decode
    writeln!(out, "{I}static {name}? decode(Unpacker u) {{")?;
    writeln!(out, "{I}{I}switch (u.unpackListLength()) {{")?;
    writeln!(out, "{I}{I}{I}case 0: return null;")?;
    writeln!(out, "{I}{I}{I}case {}: break;", item.fields.len())?;
    writeln!(out, "{I}{I}{I}default: {THROW_DECODE_ERROR};")?;
    writeln!(out, "{I}{I}}}")?;

    writeln!(out, "{I}{I}return {name}(")?;

    for (field_name, field) in &item.fields {
        write!(out, "{I}{I}{I}{}: ", AsLowerCamelCase(field_name))?;
        DartType::try_from(&field.ty)?.write_decode(out)?;
        writeln!(out, ",")?
    }

    writeln!(out, "{I}{I});")?;

    writeln!(out, "{I}}}")?;

    writeln!(out, "}}")?;
    writeln!(out)?;

    Ok(())
}

fn write_newtype(out: &mut dyn Write, name: &str, item: &Newtype) -> Result<()> {
    let ty = DartType::try_from(&item.ty)?;

    write_docs(out, "", &item.docs)?;
    writeln!(out, "extension type {name}({} value) {{", ty)?;

    writeln!(out, "{I}void encode(Packer p) {{")?;
    write!(out, "{I}{I}")?;
    ty.write_encode(out, "value")?;
    writeln!(out, ";")?;
    writeln!(out, "{I}}}")?;
    writeln!(out, "{I}")?;

    writeln!(out, "{I}static {name}? decode(Unpacker u) {{")?;

    match ty.try_into_nullable() {
        Ok(ty) => {
            write!(out, "{I}{I}final value = ")?;
            ty.write_decode(out)?;
            writeln!(out, ";")?;

            writeln!(out, "{I}{I}return value != null ? {name}(value) : null;")?;
        }
        Err(ty) => {
            write!(out, "{I}{I}return {name}(")?;
            ty.write_decode(out)?;
            writeln!(out, ");")?;
        }
    }

    writeln!(out, "{I}}}")?;

    writeln!(out, "}}")?;
    writeln!(out)?;

    Ok(())
}

fn write_exception(out: &mut dyn Write, item: &SimpleEnum) -> Result<()> {
    writeln!(out, "class OuisyncException implements Exception {{")?;

    writeln!(out, "{I}final ErrorCode code;")?;
    writeln!(out, "{I}final String message;")?;
    writeln!(out, "{I}final List<String> sources;")?;
    writeln!(out, "{I}")?;

    writeln!(
        out,
        "{I}OuisyncException._(this.code, String? message, this.sources)"
    )?;
    writeln!(out, "{I}{I}{I}: message = message ?? code.toString() {{")?;
    writeln!(out, "{I}{I}assert(code != ErrorCode.ok);")?;
    writeln!(out, "{I}}}")?;
    writeln!(out, "{I}")?;

    writeln!(out, "{I}factory OuisyncException(")?;
    writeln!(out, "{I}{I}ErrorCode code, [")?;
    writeln!(out, "{I}{I}String? message,")?;
    writeln!(out, "{I}{I}List<String> sources = const [],")?;
    writeln!(out, "{I}]) =>")?;
    writeln!(out, "{I}{I}switch (code) {{")?;

    for (variant_name, _) in &item.variants {
        if variant_name == "Ok" || variant_name == "Other" {
            writeln!(
                out,
                "{I}{I}{I}ErrorCode.{} => OuisyncException._(code, message, sources),",
                AsLowerCamelCase(variant_name)
            )?;
            continue;
        }

        writeln!(
            out,
            "{I}{I}{I}ErrorCode.{} => {}(message, sources),",
            AsLowerCamelCase(variant_name),
            variant_name
        )?;
    }

    writeln!(out, "{I}{I}}};")?;
    writeln!(out, "{I}")?;

    writeln!(out, "{I}@override")?;
    writeln!(
        out,
        "{I}String toString() => [message].followedBy(sources).join(' → ');"
    )?;

    writeln!(out, "}}")?;
    writeln!(out)?;

    for (variant_name, variant) in &item.variants {
        if variant_name == "Ok" || variant_name == "Other" {
            continue;
        }

        write_docs(out, "", &variant.docs)?;
        writeln!(out, "class {} extends OuisyncException {{", variant_name)?;
        writeln!(
            out,
            "{I}{}([String? message, List<String> sources = const[]])",
            variant_name,
        )?;
        writeln!(
            out,
            "{I}{I}{I}: super._(ErrorCode.{}, message, sources);",
            AsLowerCamelCase(variant_name)
        )?;
        writeln!(out, "}}")?;
        writeln!(out)?;
    }

    Ok(())
}

fn write_api_class(
    out: &mut dyn Write,
    name: &str,
    inner: Option<(&str, &str)>,
    request_variants: &[(String, RequestVariant)],
) -> Result<()> {
    writeln!(out, "class {name} {{")?;

    // fields
    writeln!(out, "{I}final Client _client;")?;

    if let Some((name, ty)) = inner {
        writeln!(out, "{I}final {ty} _{name};")?;
    }

    writeln!(out, "{I}")?;

    // constructor
    write!(out, "{I}{name}(this._client")?;

    if let Some((name, _)) = inner {
        write!(out, ", this._{name}")?;
    }

    writeln!(out, ");")?;
    writeln!(out, "{I}")?;

    let prefix = format!("{}_", name.to_snake_case());

    for (variant_name, variant) in request_variants {
        let op_name = if let Some(op_name) = variant_name.strip_prefix(&prefix) {
            op_name.to_lower_camel_case()
        } else {
            continue;
        };

        let ok = match &variant.ret {
            Type::Result(ty, _) => ty,
            ty => ty,
        };
        let ret = DartType::try_from(ok)?;
        let ret_handle_target = ret.handle_target();

        write_docs(out, I, &variant.docs)?;
        write!(out, "{I}Future<")?;

        if let Some(target) = ret_handle_target {
            write!(out, "{target}")?;
        } else {
            write!(out, "{ret}")?;
        }

        writeln!(out, "> {op_name}(",)?;

        for (index, (arg_name, ty)) in variant.fields.iter().enumerate() {
            if index == 0 && inner.is_some() {
                continue;
            }

            writeln!(
                out,
                "{I}{I}{} {},",
                DartType::try_from(ty)?,
                AsLowerCamelCase(arg_name)
            )?;
        }

        writeln!(out, "{I}) async {{")?;

        writeln!(
            out,
            "{I}{I}final request = Request{}(",
            AsPascalCase(variant_name)
        )?;

        let inner_name = inner.map(|(name, _)| AsLowerCamelCase(name));

        for (index, (arg_name, _)) in variant.fields.iter().enumerate() {
            let arg_name = AsLowerCamelCase(arg_name);

            if index == 0 {
                if let Some(inner_name) = &inner_name {
                    writeln!(out, "{I}{I}{I}{arg_name}: _{inner_name},",)?;
                    continue;
                }
            }

            writeln!(out, "{I}{I}{I}{arg_name}: {arg_name},")?;
        }

        writeln!(out, "{I}{I});")?;

        writeln!(out, "{I}{I}final response = await _client.invoke(request);")?;
        writeln!(out, "{I}{I}switch (response) {{")?;

        let value = if let Some(target) = ret_handle_target {
            Cow::Owned(format!("{target}(_client, value)"))
        } else {
            Cow::Borrowed("value")
        };

        match ok {
            Type::Unit => writeln!(out, "{I}{I}{I}case ResponseNone(): return;")?,
            Type::Option(ty) => {
                writeln!(
                    out,
                    "{I}{I}{I}case Response{}(value: final value): return {value};",
                    ty.to_response_variant_name(),
                )?;
                writeln!(out, "{I}{I}{I}case ResponseNone(): return null;")?;
            }
            ty => {
                writeln!(
                    out,
                    "{I}{I}{I}case Response{}(value: final value): return {value};",
                    ty.to_response_variant_name(),
                )?;
            }
        }

        writeln!(
            out,
            "{I}{I}{I}default: throw InvalidData('unexpected response');"
        )?;
        writeln!(out, "{I}{I}}}")?;

        writeln!(out, "{I}}}")?;
        writeln!(out, "{I}")?;
    }

    // special cases
    if name == "Session" {
        writeln!(out, "{I}Future<void> close() async {{")?;
        writeln!(out, "{I}{I}await _client.close();")?;
        writeln!(out, "{I}}}")?;
        writeln!(out, "{I}")?;
    }

    writeln!(out, "}}")?;
    writeln!(out)?;

    Ok(())
}

fn write_class_body(out: &mut dyn Write, name: &str, fields: &[(String, Field)]) -> Result<()> {
    for (field_name, field) in fields {
        write_docs(out, I, &field.docs)?;
        write!(out, "{I}final ")?;
        write!(out, "{}", DartType::try_from(&field.ty)?)?;
        writeln!(out, " {};", AsLowerCamelCase(field_name))?;
    }

    if !fields.is_empty() {
        writeln!(out)?;
    }

    // constructor
    write!(out, "{I}{name}(")?;

    if !fields.is_empty() {
        writeln!(out, "{{")?
    } else {
        writeln!(out)?;
    }

    for (field_name, _) in fields {
        writeln!(out, "{I}{I}required this.{},", AsLowerCamelCase(field_name))?;
    }

    write!(out, "{I}")?;

    if !fields.is_empty() {
        write!(out, "}}")?;
    }

    writeln!(out, ");")?;

    Ok(())
}

fn write_docs(out: &mut dyn Write, prefix: &str, docs: &Docs) -> Result<()> {
    for line in &docs.lines {
        writeln!(out, "{prefix}///{}", line)?;
    }

    Ok(())
}

enum DartType {
    Void,
    Bytes,
    Scalar(DartScalar),
    Nullable(DartScalar),
    List(DartScalar),
    Map(DartScalar, DartScalar),
}

impl DartType {
    fn write_decode(&self, out: &mut dyn Write) -> Result<()> {
        match self {
            Self::Void => bail!("can't generate decode for void"),
            Self::Bytes => write!(out, "u.unpackBinary()")?,
            Self::Scalar(s) => s.write_decode_bang(out)?,
            Self::Nullable(s) => s.write_decode(out)?,
            Self::List(s) => {
                write!(out, "_decodeList(u, (u) => ")?;
                s.write_decode_bang(out)?;
                write!(out, ")")?;
            }
            Self::Map(k, v) => {
                write!(out, "_decodeMap(u, (u) => ")?;
                k.write_decode_bang(out)?;
                write!(out, ", (u) => ")?;
                v.write_decode_bang(out)?;
                write!(out, ")")?;
            }
        }

        Ok(())
    }

    fn write_encode(&self, out: &mut dyn Write, name: &str) -> Result<()> {
        match self {
            Self::Void => bail!("can't generate encode for void"),
            Self::Bytes => write!(out, "p.packBinary({name})")?,
            Self::Scalar(s) => s.write_encode(out, name)?,
            Self::Nullable(s) => {
                write!(out, "_encodeNullable(p, {name}, (p, e) => ")?;
                s.write_encode(out, "e")?;
                write!(out, ")")?;
            }
            Self::List(s) => {
                write!(out, "_encodeList(p, {name}, (p, e) => ")?;
                s.write_encode(out, "e")?;
                write!(out, ")")?;
            }
            Self::Map(k, v) => {
                write!(out, "_encodeMap(p, {name}, (p, k) => ")?;
                k.write_encode(out, "k")?;
                write!(out, ", (p, v) => ")?;
                v.write_encode(out, "v")?;
                write!(out, ")")?;
            }
        }

        Ok(())
    }

    fn try_into_nullable(self) -> Result<Self, Self> {
        match self {
            Self::Scalar(scalar) => Ok(Self::Nullable(scalar)),
            _ => Err(self),
        }
    }

    // If the return type is a "handle" (e.g, `RepositoryHandle`, `FileHandle`, ...), this is
    // the name of the target type of the handle (i.e., `Repository`, `File`). Otherwise it's
    // `None`.
    fn handle_target(&self) -> Option<&str> {
        match self {
            Self::Scalar(DartScalar::Other(name)) | Self::Nullable(DartScalar::Other(name)) => {
                name.strip_suffix("Handle")
            }
            _ => None,
        }
    }
}

impl<'a> TryFrom<&'a Type> for DartType {
    type Error = Error;

    fn try_from(ty: &'a Type) -> Result<Self, Self::Error> {
        match ty {
            Type::Scalar(name) => Ok(Self::Scalar(DartScalar::from(name.as_str()))),
            Type::Bytes => Ok(Self::Bytes),
            Type::Option(name) => Ok(Self::Nullable(DartScalar::from(name.as_str()))),
            Type::Vec(name) => Ok(Self::List(DartScalar::from(name.as_str()))),
            Type::Map(k, v) => Ok(Self::Map(
                DartScalar::from(k.as_str()),
                DartScalar::from(v.as_str()),
            )),
            Type::Unit => Ok(Self::Void),
            Type::Result(..) => bail!("unsupported type: {:?}", ty),
        }
    }
}

impl fmt::Display for DartType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Void => write!(f, "void"),
            Self::Bytes => write!(f, "List<int>"),
            Self::Scalar(s) => s.fmt(f),
            Self::Nullable(s) => write!(f, "{s}?"),
            Self::List(s) => write!(f, "List<{s}>"),
            Self::Map(k, v) => write!(f, "Map<{k}, {v}>"),
        }
    }
}

enum DartScalar {
    Int,
    Bool,
    String,
    DateTime,
    Duration,
    Other(String),
}

impl DartScalar {
    fn write_decode(&self, out: &mut dyn Write) -> Result<()> {
        match self {
            Self::Int => write!(out, "u.unpackInt()")?,
            Self::Bool => write!(out, "u.unpackBool()")?,
            Self::String => write!(out, "u.unpackString()")?,
            Self::DateTime => write!(out, "_decodeDateTime(u)")?,
            Self::Duration => write!(out, "_decodeDuration(u)")?,
            Self::Other(s) => write!(out, "{s}.decode(u)")?,
        }

        Ok(())
    }

    fn write_decode_bang(&self, out: &mut dyn Write) -> Result<()> {
        write!(out, "(")?;
        self.write_decode(out)?;
        write!(out, ")!")?;

        Ok(())
    }

    fn write_encode(&self, out: &mut dyn Write, name: &str) -> Result<()> {
        match self {
            Self::Int => write!(out, "p.packInt({name})")?,
            Self::Bool => write!(out, "p.packBool({name})")?,
            Self::String => write!(out, "p.packString({name})")?,
            Self::DateTime => write!(out, "_encodeDateTime(p, {name})")?,
            Self::Duration => write!(out, "_encodeDuration(p, {name})")?,
            Self::Other(_) => write!(out, "{name}.encode(p)")?,
        }

        Ok(())
    }
}

impl fmt::Display for DartScalar {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Int => write!(f, "int"),
            Self::Bool => write!(f, "bool"),
            Self::String => write!(f, "String"),
            Self::DateTime => write!(f, "DateTime"),
            Self::Duration => write!(f, "Duration"),
            Self::Other(s) => write!(f, "{s}"),
        }
    }
}

impl From<&'_ str> for DartScalar {
    fn from(name: &str) -> Self {
        match name {
            "u8" | "u16" | "u32" | "u64" | "usize" | "i8" | "i16" | "i32" | "i64" | "isize" => {
                Self::Int
            }
            "bool" => Self::Bool,
            "PathBuf" | "PeerAddr" | "ShareToken" | "SocketAddr" | "String" => Self::String,
            "SystemTime" => Self::DateTime,
            "Duration" => Self::Duration,
            "StateMonitor" => Self::Other("StateMonitorNode".to_owned()),
            _ => Self::Other(name.to_owned()),
        }
    }
}

const I: &str = "  ";
const THROW_DECODE_ERROR: &str = "throw DecodeError()";
