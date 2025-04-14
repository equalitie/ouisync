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
    writeln!(
        out,
        "@file:UseSerializers(DurationSerializer::class, InstantSerializer::class)"
    )?;
    writeln!(out)?;
    writeln!(out, "package {PACKAGE}")?;
    writeln!(out)?;
    writeln!(out, "import kotlinx.datetime.Instant")?;
    writeln!(out, "import kotlinx.serialization.Serializable")?;
    writeln!(out, "import kotlinx.serialization.UseSerializers")?;
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
    writeln!(out, "@Serializable")?;
    writeln!(out, "enum class {name} {{")?;

    for (variant_name, variant) in &item.variants {
        write_docs(out, I, &variant.docs)?;
        writeln!(out, "{I}@Value({})", variant.value)?;
        writeln!(out, "{I}{},", AsShoutySnakeCase(variant_name))?;
    }

    writeln!(out, "{I};")?;
    writeln!(out)?;

    writeln!(out, "{I}companion object {{")?;
    writeln!(
        out,
        "{I}{I}fun fromValue(value: {repr}): {name} = when (value) {{"
    )?;

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
    writeln!(out, "{I}}}")?;

    writeln!(out, "}}")?;
    writeln!(out)?;

    Ok(())
}

fn write_complex_enum(out: &mut dyn Write, name: &str, item: &ComplexEnum) -> Result<()> {
    write_docs(out, "", &item.docs)?;
    writeln!(out, "@Serializable")?;
    writeln!(out, "sealed interface {name} {{")?;

    // variants
    for (variant_name, variant) in &item.variants {
        write_docs(out, I, &variant.docs)?;
        writeln!(out, "{I}@Serializable")?;

        match &variant.fields {
            Fields::Named(fields) => {
                writeln!(out, "{I}class {variant_name}(")?;

                for (field_name, field) in fields {
                    writeln!(out, "{I}{I}val {field_name}: {},", KotlinType(&field.ty),)?;
                }

                writeln!(out, "{I}) : {name}")?;
            }
            Fields::Unnamed(field) => {
                writeln!(out, "{I}@JvmInline")?;
                writeln!(
                    out,
                    "{I}value class {variant_name}(val {DEFAULT_FIELD_NAME}: {}) : {name}",
                    KotlinType(&field.ty)
                )?;
            }
            Fields::Unit => {
                writeln!(out, "{I}object {variant_name} : {name}")?;
            }
        }

        writeln!(out)?;
    }

    writeln!(out, "}}")?;
    writeln!(out)?;

    Ok(())
}

fn write_struct(out: &mut dyn Write, name: &str, item: &Struct) -> Result<()> {
    write_docs(out, "", &item.docs)?;
    writeln!(out, "@Serializable")?;

    match &item.fields {
        Fields::Named(fields) => {
            writeln!(out, "data class {name}(")?;

            for (field_name, field) in fields {
                writeln!(out, "{I}val {field_name}: {},", KotlinType(&field.ty))?;
            }

            write!(out, ")")?;
        }
        Fields::Unnamed(field) => {
            writeln!(out, "@JvmInline")?;
            write!(
                out,
                "value class {name}(val {DEFAULT_FIELD_NAME}: {})",
                KotlinType(&field.ty)
            )?;
        }
        Fields::Unit => {
            writeln!(out, "class {name}()")?;
        }
    }

    if item.secret {
        writeln!(out, " {{")?;
        writeln!(out, "{I}override fun toString() = \"****\"")?;
        writeln!(out, "}}")?;
    } else {
        writeln!(out)?;
    }

    writeln!(out)?;

    Ok(())
}

fn write_exception(out: &mut dyn Write, item: &SimpleEnum) -> Result<()> {
    writeln!(out, "open class OuisyncException internal constructor(")?;
    writeln!(out, "{I}val code: ErrorCode,")?;
    writeln!(out, "{I}message: String?,")?;
    writeln!(out, "{I}val sources: List<String> = emptyList(),")?;
    writeln!(out, ") : Exception(message ?: code.toString()){{")?;

    for (variant_name, variant) in &item.variants {
        if variant_name == "Ok" || variant_name == "Other" {
            continue;
        }

        write_docs(out, I, &variant.docs)?;
        writeln!(
            out,
            "{I}open class {variant_name}(message: String? = null, sources: List<String> = emptyList()):"
        )?;
        writeln!(
            out,
            "{I}{I}OuisyncException(ErrorCode.{}, message, sources)",
            AsShoutySnakeCase(variant_name),
        )?;
        writeln!(out)?;
    }

    // companion
    writeln!(out, "{I}companion object {{")?;

    // fun dispatch
    writeln!(out, "{I}{I}internal fun dispatch(")?;
    writeln!(out, "{I}{I}{I}code: ErrorCode,")?;
    writeln!(out, "{I}{I}{I}message: String? = null,")?;
    writeln!(out, "{I}{I}{I}sources: List<String> = emptyList(),")?;
    writeln!(out, "{I}{I}): OuisyncException = when (code) {{")?;

    for (variant_name, _) in &item.variants {
        if variant_name == "Ok" || variant_name == "Other" {
            continue;
        }

        writeln!(
            out,
            "{I}{I}{I}ErrorCode.{} -> {}(message, sources)",
            AsShoutySnakeCase(variant_name),
            variant_name,
        )?;
    }
    writeln!(
        out,
        "{I}{I}{I}else -> OuisyncException(code, message, sources)"
    )?;

    writeln!(out, "{I}{I}}}")?;
    writeln!(out)?;
    // end fun dispatch

    writeln!(out, "{I}}}")?;
    // end companion

    writeln!(out)?;
    writeln!(out, "{I}override fun toString() =")?;
    writeln!(out, "{I}{I}\"${{this::class.simpleName}}: ${{(sequenceOf(message) + sources.asSequence()).joinToString(\" → \")}}\"")?;

    writeln!(out, "}}")?;
    writeln!(out)?;

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

const I: &str = "    ";
const DEFAULT_FIELD_NAME: &str = "value";
