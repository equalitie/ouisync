use anyhow::{bail, Result};
use heck::{AsLowerCamelCase, AsPascalCase};
use ouisync_api_parser::{ComplexEnum, Context, Docs, Field, Item, SimpleEnum, Struct, Type};
use std::io::Write;

pub(crate) fn generate(ctx: &Context, out: &mut dyn Write) -> Result<()> {
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

    writeln!(out, "class DecodeError extends ArgumentError {{")?;
    writeln!(out, "{I}DecodeError() : super('decode error');")?;
    writeln!(out, "}}")?;
    writeln!(out)?;
    writeln!(out, "extension _AnyExtension<T> on T {{")?;
    writeln!(out, "{I}R let<R>(R Function(T) f) => f(this);")?;
    writeln!(out, "}}")?;

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

    // decode
    writeln!(out, "{I}static {}? decode(Unpacker u) {{", name)?;
    writeln!(out, "{I}{I}switch (u.unpackInt()) {{")?;

    for (variant_name, variant) in &item.variants {
        writeln!(
            out,
            "{I}{I}{I}case {}: return {}.{};",
            variant.value,
            name,
            AsLowerCamelCase(variant_name)
        )?;
    }

    writeln!(out, "{I}{I}{I}null: return null;")?;
    writeln!(out, "{I}{I}{I}default: {THROW_DECODE_ERROR};")?;
    writeln!(out, "{I}{I}}}")?;
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

fn write_complex_enum(out: &mut dyn Write, name: &str, item: &ComplexEnum) -> Result<()> {
    write_docs(out, "", &item.docs)?;
    writeln!(out, "sealed class {name} {{")?;

    // decode
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

    writeln!(out, "{I}{I}{I}{I}null: return null;")?;
    writeln!(out, "{I}{I}{I}{I}default: {THROW_DECODE_ERROR};")?;

    writeln!(out, "{I}{I}{I}}}")?;
    writeln!(out, "{I}{I}}} catch FormatException {{")?;
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
            write_decode(out, &field.ty)?;
            writeln!(out, ",")?;
        }

        writeln!(out, "{I}{I}{I}{I}{I});")?;
    }

    writeln!(out, "{I}{I}{I}{I}case null: return null;")?;
    writeln!(out, "{I}{I}{I}{I}default: {THROW_DECODE_ERROR};")?;

    writeln!(out, "{I}{I}{I}}}")?;

    writeln!(out, "{I}{I}}}")?;
    writeln!(out, "{I}}}")?;

    writeln!(out, "}}")?;
    writeln!(out)?;

    for (variant_name, variant) in &item.variants {
        let full_name = format!("{name}{variant_name}");

        write_docs(out, "", &variant.docs)?;
        writeln!(out, "class {full_name} {name} {{")?;
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

    // decode
    writeln!(out, "{I}static {name}? decode(Unpacker u) {{")?;
    writeln!(
        out,
        "{I}{I}if (u.unpackListLength() != {}) {{",
        item.fields.len()
    )?;
    writeln!(out, "{I}{I}{I}{THROW_DECODE_ERROR};")?;
    writeln!(out, "{I}{I}}}")?;

    writeln!(out, "{I}{I}return {name}(")?;

    for (field_name, field) in &item.fields {
        write!(out, "{I}{I}{I}{}: ", AsLowerCamelCase(field_name))?;
        write_decode(out, &field.ty)?;
        writeln!(out, ",")?
    }

    writeln!(out, "{I}{I});")?;

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
        if variant_name == "Ok" {
            continue;
        }

        if variant_name == "Other" {
            writeln!(out, "{I}{I}{I}ErrorCode.other => OuisyncException._(ErrorCode.other, message, sources),")?;
        } else {
            writeln!(
                out,
                "{I}{I}{I}ErrorCode.{} => {}(message, sources),",
                AsLowerCamelCase(variant_name),
                AsPascalCase(variant_name)
            )?;
        }
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
            "{I}{I}{I}super._(ErrorCode.{}, message, sources);",
            AsLowerCamelCase(variant_name)
        )?;
        writeln!(out, "}}")?;
        writeln!(out)?;
    }

    Ok(())
}

fn write_class_body(out: &mut dyn Write, name: &str, fields: &[(String, Field)]) -> Result<()> {
    for (field_name, field) in fields {
        write_docs(out, I, &field.docs)?;
        write!(out, "{I}final ")?;
        write_type(out, &field.ty)?;
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

    writeln!(out, ")")?;

    Ok(())
}

fn write_docs(out: &mut dyn Write, prefix: &str, docs: &Docs) -> Result<()> {
    for line in &docs.lines {
        writeln!(out, "{prefix}///{}", line)?;
    }

    Ok(())
}

fn write_type(out: &mut dyn Write, ty: &Type) -> Result<()> {
    match ty {
        Type::Scalar(name) => write!(out, "{}", map_type(name))?,
        Type::Option(name) => write!(out, "{}?", map_type(name))?,
        Type::Vec(name) => write!(out, "List<{}>", map_type(name))?,
        Type::Map(k, v) => write!(out, "Map<{}, {}>", map_type(k), map_type(v))?,
        Type::Bytes => write!(out, "Uint8List")?,
        Type::Unit | Type::Result(..) => bail!("unsupported type: {:?}", ty),
    }

    Ok(())
}

fn write_decode(out: &mut dyn Write, ty: &Type) -> Result<()> {
    match ty {
        Type::Scalar(name) => {
            write_decode_or_null(out, name)?;
            write!(out, "!")?;
        }
        Type::Bytes => write!(out, "u.unpackBinary()")?,
        Type::Option(name) => write_decode_or_null(out, name)?,
        Type::Unit | Type::Result(..) => bail!("unsupported type: {:?}", ty),
        Type::Vec(_) | Type::Map(_, _) => todo!(),
    }

    Ok(())
}

fn write_decode_or_null(out: &mut dyn Write, ty: &str) -> Result<()> {
    match ty {
        "u8" | "u16" | "u32" | "u64" | "i8" | "i16" | "i32" | "i64" => {
            write!(out, "u.unpackInt()")?
        }
        "PublicRuntimeId" => write!(
            out,
            "u.unpackBinary().let((b) => b.isEmpty ? null : HEX.encode(b))"
        )?,
        "PeerAddr" | "String" => write!(out, "u.unpackString()")?,
        "SystemTime" => write!(
            out,
            "u.unpackInt()?.let((n) => DateTime.fromMillisecondsSinceEpoch(n))"
        )?,
        _ => write!(out, "{ty}.decode(u)")?,
    }

    Ok(())
}

fn map_type(src: &str) -> &str {
    match src {
        "u8" | "u16" | "u32" | "u64" | "i8" | "i16" | "i32" | "i64" => "int",
        "PeerAddr" | "PublicRuntimeId" => "String",
        "SystemTime" => "DateTime",
        _ => src,
    }
}

const I: &str = "  ";
const THROW_DECODE_ERROR: &str = "throw DecodeError()";
