use anyhow::Result;
use heck::{AsLowerCamelCase, AsPascalCase, AsShoutySnakeCase, AsSnakeCase};
use ouisync_api_parser::{
    ComplexEnum, Context, Docs, EnumRepr, Fields, Item, RequestVariant, SimpleEnum, Struct,
    ToResponseVariantName, Type,
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
    writeln!(out, "import java.util.Objects")?;
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

    writeln!(
        out,
        "open class UnexpectedResponse: OuisyncException.InvalidData(\"unexpected response\")"
    )?;

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
                    writeln!(
                        out,
                        "{I}{I}val {}: {},",
                        AsLowerCamelCase(field_name),
                        KotlinType(&field.ty),
                    )?;
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
                writeln!(
                    out,
                    "{I}val {}: {},",
                    AsLowerCamelCase(field_name),
                    KotlinType(&field.ty)
                )?;
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
        writeln!(
            out,
            "{I}override fun toString() = \"${{this::class.simpleName}}(******)\""
        )?;
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

fn write_api_class(
    out: &mut dyn Write,
    name: &str,
    inner: Option<(&str, &str)>,
    request_variants: &[(String, RequestVariant)],
) -> Result<()> {
    write!(
        out,
        "open class {name} internal constructor (internal val client: Client"
    )?;

    if let Some((name, ty)) = inner {
        write!(out, ", internal val {name}: {ty}")?;
    }

    writeln!(out, ") {{")?;

    writeln!(out, "{I}companion object {{ }}")?;
    writeln!(out)?;

    let prefix = format!("{}_", AsSnakeCase(name));

    for (variant_name, variant) in request_variants {
        // Event subscription / unsubscription is handled manually
        if variant_name.contains("subscribe") {
            continue;
        }

        let Some(op_name) = variant_name.strip_prefix(&prefix) else {
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

        // Use default argument only if there is at least one other non-default argument.
        let use_default_args = variant
            .fields
            .iter()
            .skip(if inner.is_some() { 1 } else { 0 })
            .any(|(_, field)| KotlinType(&field.ty).default().is_none());

        write_docs(out, I, &variant.docs)?;
        writeln!(out, "{I}suspend fun {}(", AsLowerCamelCase(op_name))?;

        for (index, (arg_name, field)) in variant.fields.iter().enumerate() {
            if index == 0 && inner.is_some() {
                continue;
            }

            let ty = KotlinType(&field.ty);

            write!(
                out,
                "{I}{I}{}: {}",
                AsLowerCamelCase(arg_name.unwrap_or(DEFAULT_FIELD_NAME)),
                ty
            )?;

            if use_default_args {
                if let Some(default) = ty.default() {
                    write!(out, " = {default}")?;
                }
            }

            writeln!(out, ",")?;
        }

        write!(out, "{I})")?;

        match ret {
            Type::Unit => (),
            _ => write!(
                out,
                ": {}",
                ret_stripped
                    .as_ref()
                    .map(KotlinType)
                    .unwrap_or(KotlinType(ret))
            )?,
        }

        writeln!(out, " {{")?;

        // request
        write!(
            out,
            "{I}{I}val request = Request.{}",
            AsPascalCase(variant_name)
        )?;

        if !variant.fields.is_empty() {
            writeln!(out, "(")?;

            let inner_name = inner.map(|(name, _)| AsLowerCamelCase(name));

            for (index, (arg_name, _)) in variant.fields.iter().enumerate() {
                let arg_name = AsLowerCamelCase(arg_name.unwrap_or(DEFAULT_FIELD_NAME));

                if index == 0 {
                    if let Some(inner_name) = &inner_name {
                        writeln!(out, "{I}{I}{I}{inner_name},")?;
                        continue;
                    }
                }

                writeln!(out, "{I}{I}{I}{arg_name},")?;
            }

            writeln!(out, "{I}{I})")?;
        } else {
            writeln!(out)?;
        }

        // response
        writeln!(out, "{I}{I}val response = client.invoke(request)")?;
        writeln!(out, "{I}{I}when (response) {{")?;

        match ret {
            Type::Unit => writeln!(out, "{I}{I}{I}is Response.None -> return")?,
            Type::Option(_) => {
                write!(
                    out,
                    "{I}{I}{I}is Response.{} -> return ",
                    response_variant_name,
                )?;

                match ret_stripped {
                    Some(Type::Option(w)) => {
                        write!(
                            out,
                            "if (response.value != null) {w}(client, response.value) else null"
                        )?;
                    }
                    Some(_) => unreachable!(),
                    None => write!(out, "response.value")?,
                }

                writeln!(out)?;
                writeln!(out, "{I}{I}{I}is Response.None -> return null")?;
            }
            _ => {
                write!(
                    out,
                    "{I}{I}{I}is Response.{} -> return ",
                    response_variant_name,
                )?;

                match ret_stripped {
                    Some(Type::Scalar(w)) => write!(out, "{w}(client, response.value)")?,
                    Some(Type::Vec(w)) => write!(out, "response.value.map {{ {w}(client, it) }}")?,
                    Some(Type::Map(_, w)) => {
                        write!(out, "response.value.mapValues {{ {w}(client, it.value) }}")?
                    }
                    Some(_) => unreachable!(),
                    None => write!(out, "response.value")?,
                }

                writeln!(out)?;
            }
        }

        writeln!(out, "{I}{I}{I}else -> throw UnexpectedResponse()")?;
        writeln!(out, "{I}{I}}}")?;

        writeln!(out, "{I}}}")?;
        writeln!(out)?;
    }

    // equals + hashCode + toString
    if let Some((inner_name, _)) = inner {
        writeln!(out, "{I}override fun equals(other: Any?): Boolean =")?;
        writeln!(
            out,
            "{I}{I}other is {name} && client == other.client && {inner_name} == other.{inner_name}"
        )?;

        writeln!(out)?;
        writeln!(
            out,
            "{I}override fun hashCode(): Int = Objects.hash(client, {inner_name})"
        )?;

        writeln!(out)?;
        writeln!(
            out,
            "{I}override fun toString(): String = \"${{this::class.simpleName}}(${inner_name})\""
        )?;
    }

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

impl KotlinType<'_> {
    fn default(&self) -> Option<&str> {
        match self.0 {
            Type::Option(_) => Some("null"),
            Type::Scalar(s) if s == "bool" => Some("false"),
            _ => None,
        }
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

const I: &str = "    ";
const DEFAULT_FIELD_NAME: &str = "value";
