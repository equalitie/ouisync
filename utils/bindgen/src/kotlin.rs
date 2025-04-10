use anyhow::Result;
use heck::AsShoutySnakeCase;
use ouisync_api_parser::{
    ComplexEnum, Context, Docs, EnumRepr, Fields, Item, SimpleEnum, Struct, Type,
};
use std::{
    fmt,
    io::{self, Write},
};

const PACKAGE: &str = "org.equalitie.ouisync.lib";

pub(crate) fn generate(ctx: &Context, out: &mut dyn Write) -> Result<()> {
    writeln!(out, "package {PACKAGE}")?;
    writeln!(out)?;
    writeln!(out, "{}", include_str!("support.kt"))?;
    writeln!(out)?;

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

    // write_api_class(out, "Session", None, &ctx.request.variants)?;
    // write_api_class(
    //     out,
    //     "Repository",
    //     Some(("handle", "RepositoryHandle")),
    //     &ctx.request.variants,
    // )?;
    // write_api_class(
    //     out,
    //     "File",
    //     Some(("handle", "FileHandle")),
    //     &ctx.request.variants,
    // )?;

    Ok(())
}

fn write_simple_enum(out: &mut dyn Write, name: &str, item: &SimpleEnum) -> Result<()> {
    let repr = match item.repr {
        EnumRepr::U8 => "Byte",
        EnumRepr::U16 => "Short",
        EnumRepr::U32 => "Int",
        EnumRepr::U64 => "Long",
    };

    write_docs(out, "", &item.docs)?;
    writeln!(out, "enum class {name} {{")?;

    for (variant_name, variant) in &item.variants {
        write_docs(out, I, &variant.docs)?;
        writeln!(out, "{I}{},", AsShoutySnakeCase(variant_name))?;
    }

    writeln!(out)?;
    writeln!(out, "{I};")?;
    writeln!(out)?;

    writeln!(out, "{I}companion object {{")?;

    // decode
    writeln!(out, "{I}{I}fun fromNum(n: {repr}): {name} = when (n) {{")?;

    for (variant_name, variant) in &item.variants {
        writeln!(
            out,
            "{I}{I}{I}{}.to{repr}() -> {}",
            variant.value,
            AsShoutySnakeCase(variant_name)
        )?;
    }

    writeln!(out, "{I}{I}{I}else -> throw IllegalArgumentException()")?;
    writeln!(out, "{I}{I}}}")?;
    writeln!(out)?;

    writeln!(
        out,
        "{I}{I}fun decode(u: MessageUnpacker): {name} = fromNum(u.unpack{repr}())"
    )?;

    writeln!(out, "{I}}}")?;
    writeln!(out)?;

    // encode
    writeln!(out, "{I}fun toNum(): {repr} = when (this) {{")?;

    for (variant_name, variant) in &item.variants {
        writeln!(
            out,
            "{I}{I}{} -> {}",
            AsShoutySnakeCase(variant_name),
            variant.value
        )?;
    }
    writeln!(out, "{I}}}.to{repr}()")?;
    writeln!(out)?;

    writeln!(
        out,
        "{I}fun encode(p: MessagePacker) = p.pack{repr}(toNum())"
    )?;

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

    // variants
    for (variant_name, variant) in &item.variants {
        write_docs(out, I, &variant.docs)?;

        match &variant.fields {
            Fields::Named(_) | Fields::Unnamed(_) => {
                writeln!(out, "{I}class {variant_name}(")?;

                for (field_name, field) in &variant.fields {
                    writeln!(
                        out,
                        "{I}{I}val {}: {},",
                        field_name.unwrap_or(DEFAULT_FIELD_NAME),
                        KotlinType(&field.ty),
                    )?;
                }

                writeln!(out, "{I}) : {name}()")?;
            }
            Fields::Unit => {
                writeln!(out, "{I}object {variant_name} : {name}()")?;
            }
        }

        writeln!(out)?;
    }

    if decode {
        writeln!(out, "{I}companion object {{")?;

        writeln!(out, "{I}{I}fun decode(u: MessageUnpacker): {name} {{")?;
        writeln!(out, "{I}{I}{I}val type = u.getNextFormat().getValueType()")?;
        writeln!(out, "{I}{I}{I}when (type) {{")?;

        // Variants without fields
        if item
            .variants
            .iter()
            .any(|(_, v)| matches!(v.fields, Fields::Unit))
        {
            writeln!(out, "{I}{I}{I}{I}ValueType.STRING -> {{")?;
            writeln!(out, "{I}{I}{I}{I}{I}when (u.unpackString()) {{")?;

            for (variant_name, _) in item
                .variants
                .iter()
                .filter(|(_, v)| matches!(v.fields, Fields::Unit))
            {
                writeln!(
                    out,
                    "{I}{I}{I}{I}{I}{I}\"{variant_name}\" -> return {name}.{variant_name}"
                )?;
            }

            writeln!(out, "{I}{I}{I}{I}{I}{I}else -> throw DecodeError()")?;

            writeln!(out, "{I}{I}{I}{I}{I}}}")?;
            writeln!(out, "{I}{I}{I}{I}}}")?;
        }

        // Variants with fields
        writeln!(out, "{I}{I}{I}{I}ValueType.MAP -> {{")?;
        writeln!(
            out,
            "{I}{I}{I}{I}{I}if (u.unpackMapHeader() != 1) throw DecodeError()"
        )?;
        writeln!(out, "{I}{I}{I}{I}{I}when (u.unpackString()) {{")?;

        for (variant_name, variant) in item
            .variants
            .iter()
            .filter(|(_, v)| !matches!(v.fields, Fields::Unit))
        {
            writeln!(out, "{I}{I}{I}{I}{I}{I}\"{variant_name}\" -> {{")?;

            match &variant.fields {
                Fields::Named(fields) => {
                    writeln!(
                        out,
                        "{I}{I}{I}{I}{I}{I}{I}if (u.unpackArrayHeader() != {}) throw DecodeError()",
                        variant.fields.len(),
                    )?;
                    writeln!(out, "{I}{I}{I}{I}{I}{I}{I}return {name}.{variant_name}(")?;

                    for (field_name, field) in fields {
                        writeln!(
                            out,
                            "{I}{I}{I}{I}{I}{I}{I}{I}{field_name} = {},",
                            KotlinType(&field.ty).decode(),
                        )?;
                    }

                    writeln!(out, "{I}{I}{I}{I}{I}{I}{I})")?;
                }
                Fields::Unnamed(field) => {
                    writeln!(out, "{I}{I}{I}{I}{I}{I}{I}return {name}.{variant_name}(")?;

                    writeln!(
                        out,
                        "{I}{I}{I}{I}{I}{I}{I}{I}{DEFAULT_FIELD_NAME} = {},",
                        KotlinType(&field.ty).decode()
                    )?;

                    writeln!(out, "{I}{I}{I}{I}{I}{I}{I})")?;
                }
                Fields::Unit => unreachable!(),
            }

            writeln!(out, "{I}{I}{I}{I}{I}{I}}}")?;
        }

        writeln!(out, "{I}{I}{I}{I}{I}{I}else -> throw DecodeError()")?;

        writeln!(out, "{I}{I}{I}{I}{I}}}")?;

        writeln!(out, "{I}{I}{I}{I}}}")?;
        writeln!(out, "{I}{I}{I}{I}else -> throw DecodeError()")?;
        writeln!(out, "{I}{I}{I}}}")?;

        writeln!(out, "{I}{I}}}")?;

        writeln!(out, "{I}}}")?;
        writeln!(out)?;
    }

    if encode {
        writeln!(out, "{I}fun encode(p: MessagePacker) {{")?;
        writeln!(out, "{I}{I}when (this) {{")?;

        for (variant_name, variant) in &item.variants {
            writeln!(out, "{I}{I}{I}is {name}.{variant_name} -> {{")?;

            match &variant.fields {
                Fields::Named(fields) => {
                    writeln!(out, "{I}{I}{I}{I}p.packMapHeader(1)")?;
                    writeln!(out, "{I}{I}{I}{I}p.packString(\"{variant_name}\")")?;
                    writeln!(out, "{I}{I}{I}{I}p.packArrayHeader({})", fields.len())?;

                    for (field_name, field) in fields {
                        writeln!(
                            out,
                            "{I}{I}{I}{I}{}",
                            KotlinType(&field.ty).encode(field_name),
                        )?;
                    }
                }
                Fields::Unnamed(field) => {
                    writeln!(out, "{I}{I}{I}{I}p.packMapHeader(1)")?;
                    writeln!(out, "{I}{I}{I}{I}p.packString(\"{variant_name}\")")?;
                    writeln!(
                        out,
                        "{I}{I}{I}{I}{}",
                        KotlinType(&field.ty).encode(DEFAULT_FIELD_NAME)
                    )?;
                    writeln!(out)?;
                }
                Fields::Unit => {
                    writeln!(out, "{I}{I}{I}{I}p.packString(\"{variant_name}\")")?;
                }
            }

            writeln!(out, "{I}{I}{I}}}")?;
        }

        writeln!(out, "{I}{I}}}")?;
        writeln!(out, "{I}}}")?;
        writeln!(out)?;
    }

    writeln!(out, "}}")?;
    writeln!(out)?;

    Ok(())
}

fn write_struct(out: &mut dyn Write, name: &str, item: &Struct) -> Result<()> {
    write_docs(out, "", &item.docs)?;
    writeln!(out, "data class {name}(")?;

    for (field_name, field) in &item.fields {
        writeln!(
            out,
            "{I}val {}: {},",
            field_name.unwrap_or(DEFAULT_FIELD_NAME),
            KotlinType(&field.ty)
        )?;
    }

    writeln!(out, ") {{")?;

    // companion
    writeln!(out, "{I}companion object {{")?;

    // decode
    writeln!(out, "{I}{I}fun decode(u: MessageUnpacker): {name} {{")?;

    match &item.fields {
        Fields::Named(fields) => {
            writeln!(
                out,
                "{I}{I}{I}if (u.unpackArrayHeader() != {}) throw DecodeError()",
                fields.len()
            )?;

            writeln!(out, "{I}{I}{I}return {name}(")?;

            for (field_name, field) in fields {
                writeln!(
                    out,
                    "{I}{I}{I}{I}{field_name} = {},",
                    KotlinType(&field.ty).decode()
                )?;
            }

            writeln!(out, "{I}{I}{I})")?;
        }
        Fields::Unnamed(field) => {
            writeln!(
                out,
                "{I}{I}{I}return {name}(value = {})",
                KotlinType(&field.ty).decode()
            )?;
        }
        Fields::Unit => unimplemented!(),
    }

    writeln!(out, "{I}{I}}}")?;

    writeln!(out, "{I}}}")?; // end companion

    // encode
    writeln!(out)?;
    writeln!(out, "{I}fun encode(p: MessagePacker) {{")?;

    match &item.fields {
        Fields::Named(fields) => {
            writeln!(out, "{I}{I}p.packArrayHeader({})", fields.len())?;

            for (field_name, field) in fields {
                writeln!(out, "{I}{I}{}", KotlinType(&field.ty).encode(field_name))?;
            }
        }
        Fields::Unnamed(field) => {
            writeln!(
                out,
                "{I}{I}{}",
                KotlinType(&field.ty).encode(DEFAULT_FIELD_NAME)
            )?;
        }
        Fields::Unit => unimplemented!(),
    }

    writeln!(out, "{I}}}")?;

    if item.secret {
        writeln!(out)?;
        writeln!(out, "{I}override fun toString() = \"****\"")?;
    }

    writeln!(out, "}}")?;
    writeln!(out)?;

    Ok(())
}

fn write_exception(_out: &mut dyn Write, _item: &SimpleEnum) -> Result<()> {
    Ok(())
}

fn write_docs(out: &mut dyn Write, prefix: &str, docs: &Docs) -> io::Result<()> {
    if docs.lines.is_empty() {
        return Ok(());
    }

    writeln!(out, "{prefix}/**")?;

    for line in &docs.lines {
        writeln!(out, "{prefix} *{}", line)?;
    }

    writeln!(out, "{prefix} */")?;

    Ok(())
}

struct KotlinType<'a>(&'a Type);

impl<'a> KotlinType<'a> {
    fn encode(self, name: &'a str) -> Encode<'a> {
        Encode(self, name)
    }

    fn decode(self) -> Decode<'a> {
        Decode(self)
    }
}

impl fmt::Display for KotlinType<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            Type::Unit => write!(f, "Unit"),
            Type::Scalar(s) => write!(f, "{}", KotlinScalar(s)),
            Type::Option(s) => write!(f, "{}?", KotlinScalar(s)),
            Type::Vec(s) => write!(f, "List<{}>", KotlinScalar(s)),
            Type::Map(k, v) => write!(f, "Map<{}, {}>", KotlinScalar(k), KotlinScalar(v)),
            Type::Bytes => write!(f, "ByteArray"),
            Type::Result(..) => Err(fmt::Error),
        }
    }
}

struct KotlinScalar<'a>(&'a str);

impl fmt::Display for KotlinScalar<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            "u8" | "i8" => write!(f, "Byte"),
            "u16" | "i16" => write!(f, "Short"),
            "u32" | "i32" => write!(f, "Int"),
            "u64" | "i64" | "usize" | "isize" => write!(f, "Long"),
            "bool" => write!(f, "Boolean"),
            "PathBuf" | "PeerAddr" | "SocketAddr" | "String" => write!(f, "kotlin.String"),
            "Duration" => write!(f, "kotlin.time.Duration"),
            "SystemTime" => write!(f, "Instant"),
            "StateMonitor" => write!(f, "{PACKAGE}.StateMonitorNode"),
            _ => write!(f, "{PACKAGE}.{}", self.0),
        }
    }
}

struct Encode<'a>(KotlinType<'a>, &'a str);

impl fmt::Display for Encode<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 .0 {
            Type::Unit => Ok(()),
            Type::Scalar(_) => write!(f, "{}.encode(p)", self.1),
            Type::Option(_) => {
                write!(f, "if ({0} == null) p.packNil() else {0}.encode(p)", self.1,)
            }
            Type::Vec(_) => write!(
                f,
                "{}.let {{ p.packArrayHeader(it.size); it.forEach {{ it.encode(p) }} }}",
                self.1,
            ),
            Type::Map(_, _) => write!(
                f,
                "{}.let {{ p.packMapHeader(it.size); it.forEach {{ key.encode(); value.encode() }}",
                self.1,
            ),
            Type::Bytes => write!(
                f,
                "{}.let {{ p.packBinaryHeader(it.size); p.addPayload(it) }}",
                self.1,
            ),
            Type::Result(..) => Err(fmt::Error),
        }
    }
}

struct Decode<'a>(KotlinType<'a>);

impl fmt::Display for Decode<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 .0 {
            Type::Unit => write!(f, "()"),
            Type::Scalar(s) => write!(f, "{}.decode(u)", KotlinScalar(s)),
            Type::Option(s) => write!(
                f,
                "if (u.tryUnpackNil()) null else {}.decode(u)",
                KotlinScalar(s)
            ),
            Type::Vec(s) => write!(
                f,
                "buildList {{ repeat(u.unpackArrayHeader()) {{ add({}.decode(u)) }} }}",
                KotlinScalar(s),
            ),
            Type::Map(k, v) => write!(
                f,
                "buildMap {{ repeat(u.unpackMapHeader()) {{ put({}.decode(u), {}.decode(u)) }} }}",
                KotlinScalar(k),
                KotlinScalar(v),
            ),
            Type::Bytes => write!(f, "u.readPayload(u.unpackBinaryHeader())"),
            Type::Result(..) => Err(fmt::Error),
        }
    }
}

const I: &str = "    ";
const DEFAULT_FIELD_NAME: &str = "value";
