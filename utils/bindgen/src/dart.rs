use anyhow::{bail, Context as _, Error, Result};
use heck::{AsLowerCamelCase, AsPascalCase, ToLowerCamelCase, ToSnakeCase};
use ouisync_api_parser::{
    ComplexEnum, Context, Docs, Fields, Item, RequestVariant, SimpleEnum, Struct,
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
        fn write_field_encode(out: &mut dyn Write, ty: &Type, name: &str) -> Result<()> {
            write!(out, "{I}{I}{I}{I}")?;
            DartType::try_from(ty)?.write_encode(out, name)?;
            writeln!(out, ";")?;
            Ok(())
        }

        writeln!(out, "{I}void encode(Packer p) {{")?;
        writeln!(out, "{I}{I}switch (this) {{")?;

        for (variant_name, variant) in &item.variants {
            writeln!(out, "{I}{I}{I}case {name}{variant_name}(")?;

            for (field_name, _) in &variant.fields {
                writeln!(
                    out,
                    "{I}{I}{I}{I}{0}: final {0},",
                    AsLowerCamelCase(field_name.unwrap_or(DEFAULT_FIELD_NAME))
                )?;
            }

            writeln!(out, "{I}{I}{I}):")?;

            match &variant.fields {
                Fields::Named(fields) => {
                    writeln!(out, "{I}{I}{I}{I}p.packMapLength(1);")?;
                    writeln!(out, "{I}{I}{I}{I}p.packString('{variant_name}');")?;
                    writeln!(out, "{I}{I}{I}{I}p.packListLength({});", fields.len())?;

                    for (field_name, field) in fields {
                        write_field_encode(out, &field.ty, &field_name.to_lower_camel_case())?;
                    }
                }
                Fields::Unnamed(field) => {
                    writeln!(out, "{I}{I}{I}{I}p.packMapLength(1);")?;
                    writeln!(out, "{I}{I}{I}{I}p.packString('{variant_name}');")?;
                    write_field_encode(out, &field.ty, DEFAULT_FIELD_NAME)?;
                }
                Fields::Unit => {
                    writeln!(out, "{I}{I}{I}{I}p.packString('{variant_name}');")?;
                }
            }
        }

        writeln!(out, "{I}{I}}}")?;
        writeln!(out, "{I}}}")?;
        writeln!(out, "{I}")?;
    }

    if decode {
        writeln!(out, "{I}static {name}? decode(Unpacker u) {{")?;

        // Variants without fields
        writeln!(out, "{I}{I}try {{")?;
        writeln!(out, "{I}{I}{I}switch (u.unpackString()) {{")?;

        for (variant_name, variant) in &item.variants {
            if !matches!(variant.fields, Fields::Unit) {
                continue;
            }

            writeln!(
                out,
                "{I}{I}{I}{I}case \"{variant_name}\": return {name}{variant_name}();"
            )?;
        }

        writeln!(out, "{I}{I}{I}{I}case null: return null;")?;
        writeln!(out, "{I}{I}{I}{I}default: throw DecodeError();")?;

        writeln!(out, "{I}{I}{I}}}")?;
        writeln!(out, "{I}{I}}} on FormatException {{")?;

        // Variants with fields
        writeln!(
            out,
            "{I}{I}{I}if (u.unpackMapLength() != 1) throw DecodeError();"
        )?;
        writeln!(out, "{I}{I}{I}switch (u.unpackString()) {{")?;

        for (variant_name, variant) in &item.variants {
            if matches!(variant.fields, Fields::Unit) {
                continue;
            }

            writeln!(out, "{I}{I}{I}{I}case \"{variant_name}\":")?;

            if matches!(variant.fields, Fields::Named(_)) {
                writeln!(
                    out,
                    "{I}{I}{I}{I}{I}if (u.unpackListLength() != {}) throw DecodeError();",
                    variant.fields.len()
                )?;
            }
            writeln!(out, "{I}{I}{I}{I}{I}return {name}{variant_name}(")?;

            for (field_name, field) in &variant.fields {
                write!(out, "{I}{I}{I}{I}{I}{I}")?;

                if let Some(field_name) = field_name {
                    write!(out, "{}: ", AsLowerCamelCase(field_name))?;
                }

                DartType::try_from(&field.ty)?
                    .write_decode(out)
                    .with_context(|| {
                        format!(
                            "failed to generate decode for {name}::{variant_name}::{}",
                            field_name.unwrap_or(DEFAULT_FIELD_NAME)
                        )
                    })?;
                writeln!(out, ",")?;
            }

            writeln!(out, "{I}{I}{I}{I}{I});")?;
        }

        writeln!(out, "{I}{I}{I}{I}case null: return null;")?;
        writeln!(out, "{I}{I}{I}{I}default: throw DecodeError();")?;

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
    writeln!(out, "final class {name} {{")?;

    write_class_body(out, name, &item.fields)?;
    writeln!(out, "{I}")?;

    // encode
    writeln!(out, "{I}void encode(Packer p) {{")?;

    if matches!(item.fields, Fields::Named(_)) {
        writeln!(out, "{I}{I}p.packListLength({});", item.fields.len())?;
    }

    for (field_name, field) in &item.fields {
        write!(out, "{I}{I}")?;
        DartType::try_from(&field.ty)?.write_encode(
            out,
            &field_name
                .map(|f| Cow::Owned(f.to_lower_camel_case()))
                .unwrap_or(Cow::Borrowed("value")),
        )?;
        writeln!(out, ";")?;
    }

    writeln!(out, "{I}}}")?;
    writeln!(out, "{I}")?;

    // decode
    writeln!(out, "{I}static {name}? decode(Unpacker u) {{")?;

    match &item.fields {
        Fields::Named(fields) => {
            writeln!(out, "{I}{I}switch (u.unpackListLength()) {{")?;
            writeln!(out, "{I}{I}{I}case 0: return null;")?;
            writeln!(out, "{I}{I}{I}case {}: break;", fields.len())?;
            writeln!(out, "{I}{I}{I}default: throw DecodeError();")?;
            writeln!(out, "{I}{I}}}")?;

            writeln!(out, "{I}{I}return {name}(")?;

            for (field_name, field) in fields {
                write!(out, "{I}{I}{I}{}: ", AsLowerCamelCase(field_name))?;
                DartType::try_from(&field.ty)?.write_decode(out)?;
                writeln!(out, ",")?
            }

            writeln!(out, "{I}{I});")?;
        }
        Fields::Unnamed(field) => {
            let ty = DartType::try_from(&field.ty)?;

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
        }
        Fields::Unit => todo!(),
    }

    writeln!(out, "{I}}}")?;

    if !item.fields.is_empty() {
        // operator ==
        writeln!(out, "{I}")?;
        writeln!(out, "{I}@override")?;
        writeln!(out, "{I}operator==(Object other) =>")?;
        writeln!(out, "{I}{I}{I}other is {name} &&")?;

        for (index, (field_name, _)) in item.fields.iter().enumerate() {
            write!(
                out,
                "{I}{I}{I}other.{0} == {0}",
                AsLowerCamelCase(field_name.unwrap_or(DEFAULT_FIELD_NAME))
            )?;

            if index < item.fields.len() - 1 {
                writeln!(out, " &&")?;
            } else {
                writeln!(out, ";")?;
            }
        }

        // hashCode
        writeln!(out, "{I}")?;
        writeln!(out, "{I}@override")?;
        write!(out, "{I}int get hashCode => ")?;

        if item.fields.len() == 1 {
            writeln!(
                out,
                "{}.hashCode;",
                AsLowerCamelCase(
                    &item
                        .fields
                        .iter()
                        .next()
                        .unwrap()
                        .0
                        .unwrap_or(DEFAULT_FIELD_NAME)
                )
            )?;
        } else {
            writeln!(out, "Object.hash(")?;

            for (field_name, _) in &item.fields {
                writeln!(
                    out,
                    "{I}{I}{I}{I}{},",
                    AsLowerCamelCase(field_name.unwrap_or(DEFAULT_FIELD_NAME))
                )?;
            }

            writeln!(out, "{I}{I}{I});")?;
        }
    }

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
    writeln!(out, "{I}final Client client;")?;

    if let Some((name, ty)) = inner {
        writeln!(out, "{I}final {ty} {name};")?;
    }

    writeln!(out, "{I}")?;

    // constructor
    write!(out, "{I}{name}(this.client")?;

    if let Some((name, _)) = inner {
        write!(out, ", this.{name}")?;
    }

    writeln!(out, ");")?;
    writeln!(out, "{I}")?;

    let prefix = format!("{}_", name.to_snake_case());

    for (variant_name, variant) in request_variants {
        // Event subscription / unsubscription is handled manually
        if variant_name.contains("subscribe") {
            continue;
        }

        let op_name = if let Some(op_name) = variant_name.strip_prefix(&prefix) {
            op_name.to_lower_camel_case()
        } else {
            continue;
        };

        // Return type
        let ret = match &variant.ret {
            Type::Result(ty, _) => ty,
            ty => ty,
        };
        // Return type with the "Handle" suffix stripped. Useful for mapping handles to their target
        // types (e.g., `RepositoryHandle` -> `Repository`).
        let ret_stripped = ret.strip_suffix("Handle");
        let response_variant_name = ret.to_response_variant_name();

        let ret = DartType::try_from(ret)?;
        let ret_stripped = ret_stripped.as_ref().map(DartType::try_from).transpose()?;

        let kw_args = use_keyword_args(name, &op_name);

        write_docs(out, I, &variant.docs)?;
        write!(
            out,
            "{I}Future<{}> {op_name}(",
            ret_stripped.as_ref().unwrap_or(&ret),
        )?;

        if kw_args {
            writeln!(out, "{{")?;
        } else {
            writeln!(out)?;
        }

        for (index, (arg_name, field)) in variant.fields.iter().enumerate() {
            if index == 0 && inner.is_some() {
                continue;
            }

            let kw_arg = kw_args.then(|| KeywordArg::from(&field.ty));

            write!(out, "{I}{I}")?;

            if let Some(kw_arg) = kw_arg {
                write!(out, "{}", kw_arg.prefix())?;
            }

            write!(
                out,
                "{} {}",
                DartType::try_from(&field.ty)?,
                AsLowerCamelCase(arg_name.unwrap_or(DEFAULT_FIELD_NAME))
            )?;

            if let Some(kw_arg) = kw_arg {
                write!(out, "{}", kw_arg.suffix())?;
            }

            writeln!(out, ",")?;
        }

        write!(out, "{I}")?;

        if kw_args {
            write!(out, "}}")?;
        }

        writeln!(out, ") async {{")?;

        writeln!(
            out,
            "{I}{I}final request = Request{}(",
            AsPascalCase(variant_name)
        )?;

        let inner_name = inner.map(|(name, _)| AsLowerCamelCase(name));

        for (index, (arg_name, _)) in variant.fields.iter().enumerate() {
            let arg_name = AsLowerCamelCase(arg_name.unwrap_or(DEFAULT_FIELD_NAME));

            if index == 0 {
                if let Some(inner_name) = &inner_name {
                    writeln!(out, "{I}{I}{I}{arg_name}: {inner_name},",)?;
                    continue;
                }
            }

            writeln!(out, "{I}{I}{I}{arg_name}: {arg_name},")?;
        }

        writeln!(out, "{I}{I});")?;

        writeln!(out, "{I}{I}final response = await client.invoke(request);")?;
        writeln!(out, "{I}{I}switch (response) {{")?;

        match ret {
            DartType::Void => writeln!(out, "{I}{I}{I}case ResponseNone(): return;")?,
            DartType::Nullable(_) => {
                write!(
                    out,
                    "{I}{I}{I}case Response{}(value: final value): return ",
                    response_variant_name,
                )?;

                match ret_stripped {
                    Some(DartType::Nullable(w)) => {
                        write!(out, "value != null ? {w}(client, value) : null")?;
                    }
                    Some(_) => unreachable!(),
                    None => write!(out, "value")?,
                }

                writeln!(out, ";")?;
                writeln!(out, "{I}{I}{I}case ResponseNone(): return null;")?;
            }
            _ => {
                write!(
                    out,
                    "{I}{I}{I}case Response{}(value: final value): return ",
                    response_variant_name,
                )?;

                match ret_stripped {
                    Some(DartType::Scalar(w)) => write!(out, "{w}(client, value)")?,
                    Some(DartType::List(w)) => write!(out, "value.map((e) => {w}(client, e))")?,
                    Some(DartType::Map(_, w)) => {
                        write!(out, "value.map((k, v) => MapEntry(k, {w}(client, v)))")?
                    }
                    Some(_) => unreachable!(),
                    None => write!(out, "value")?,
                }

                writeln!(out, ";")?;
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

    if let Some((inner_name, _)) = inner {
        // operator ==
        writeln!(out, "{I}@override")?;
        writeln!(out, "{I}bool operator ==(Object other) =>")?;
        writeln!(out, "{I}{I}{I}other is {name} &&")?;
        writeln!(out, "{I}{I}{I}other.client == client &&")?;
        writeln!(out, "{I}{I}{I}other.{inner_name} == {inner_name};")?;
        writeln!(out, "{I}")?;

        // hashCode
        writeln!(out, "{I}@override")?;
        writeln!(
            out,
            "{I}int get hashCode => Object.hash(client, {inner_name});"
        )?;
        writeln!(out, "{I}")?;

        // toString
        writeln!(out, "{I}@override")?;
        writeln!(
            out,
            "{I}String toString() => '$runtimeType(${inner_name})';"
        )?;
        writeln!(out, "{I}")?;
    }

    writeln!(out, "}}")?;
    writeln!(out)?;

    Ok(())
}

fn use_keyword_args(class_name: &str, method_name: &str) -> bool {
    matches!(
        (class_name, method_name),
        ("Session", "createRepository" | "openRepository") | ("Repository", "share" | "setAccess")
    )
}

#[derive(Clone, Copy)]
enum KeywordArg {
    Required,
    Nullable,
    Bool,
}

impl<'a> From<&'a Type> for KeywordArg {
    fn from(ty: &'a Type) -> Self {
        match ty {
            Type::Option(_) => Self::Nullable,
            Type::Scalar(s) if s == "bool" => Self::Bool,
            _ => Self::Required,
        }
    }
}

impl KeywordArg {
    fn prefix(&self) -> &str {
        match self {
            Self::Required => "required ",
            Self::Nullable => "",
            Self::Bool => "",
        }
    }

    fn suffix(&self) -> &str {
        match self {
            Self::Required => "",
            Self::Nullable => "",
            Self::Bool => " = false",
        }
    }
}

fn write_class_body(out: &mut dyn Write, name: &str, fields: &Fields) -> Result<()> {
    // fields
    for (field_name, field) in fields {
        write_docs(out, I, &field.docs)?;
        write!(out, "{I}final ")?;
        write!(out, "{}", DartType::try_from(&field.ty)?)?;
        writeln!(
            out,
            " {};",
            AsLowerCamelCase(field_name.unwrap_or(DEFAULT_FIELD_NAME))
        )?;
    }

    if !fields.is_empty() {
        writeln!(out)?;
    }

    // constructor
    write!(out, "{I}{name}(")?;

    match fields {
        Fields::Named(fields) => {
            writeln!(out, "{{")?;

            for (field_name, _) in fields {
                writeln!(out, "{I}{I}required this.{},", AsLowerCamelCase(field_name))?;
            }

            write!(out, "{I}}}")?;
        }
        Fields::Unnamed(_) => {
            write!(out, "this.value")?;
        }
        Fields::Unit => (),
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
            "PathBuf" | "PeerAddr" | "SocketAddr" | "String" => Self::String,
            "SystemTime" => Self::DateTime,
            "Duration" => Self::Duration,
            "StateMonitor" => Self::Other("StateMonitorNode".to_owned()),
            _ => Self::Other(name.to_owned()),
        }
    }
}

const I: &str = "  ";
const DEFAULT_FIELD_NAME: &str = "value";
