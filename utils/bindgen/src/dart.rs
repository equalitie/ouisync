use anyhow::{bail, Result};
use heck::AsLowerCamelCase;
use ouisync_api_parser::{ComplexEnum, Context, Docs, SimpleEnum, Type};
use std::io::Write;

pub(crate) fn generate(ctx: &Context, out: &mut dyn Write) -> Result<()> {
    for (name, item) in &ctx.simple_enums {
        write_simple_enum(out, name, item)?;
    }

    for (name, item) in &ctx.complex_enums {
        write_complex_enum(out, name, item)?;
    }

    Ok(())
}

fn write_simple_enum(out: &mut dyn Write, name: &str, item: &SimpleEnum) -> Result<()> {
    write_docs(out, "", &item.docs)?;
    writeln!(out, "enum {name} {{")?;

    for (name, variant) in &item.variants {
        write_docs(out, "  ", &variant.docs)?;
        writeln!(out, "  {},", AsLowerCamelCase(name))?;
    }

    writeln!(out, "  ;")?;
    writeln!(out)?;

    // decode
    writeln!(out, "  static {} decode(int n) {{", name)?;
    writeln!(out, "    switch (n) {{")?;

    for (variant_name, variant) in &item.variants {
        writeln!(
            out,
            "      case {}: return {}.{};",
            variant.value,
            name,
            AsLowerCamelCase(variant_name)
        )?;
    }

    writeln!(
        out,
        "      default: throw ArgumentError('invalid value: $n');"
    )?;
    writeln!(out, "    }}")?;
    writeln!(out, "  }}")?;
    writeln!(out)?;

    // encode
    writeln!(out, "  int encode() {{")?;
    writeln!(out, "    switch (this) {{")?;

    for (variant_name, variant) in &item.variants {
        writeln!(
            out,
            "      case {}.{}: return {};",
            name,
            AsLowerCamelCase(variant_name),
            variant.value
        )?;
    }

    writeln!(out, "    }}")?;
    writeln!(out, "  }}")?;
    writeln!(out)?;

    writeln!(out, "}}")?;
    writeln!(out)?;

    Ok(())
}

fn write_complex_enum(out: &mut dyn Write, name: &str, item: &ComplexEnum) -> Result<()> {
    write_docs(out, "", &item.docs)?;
    writeln!(out, "sealed class {name} {{")?;

    // decode
    writeln!(out, "  {name} decode(Object? input) {{")?;
    writeln!(out, "    switch (input) {{")?;

    for (variant_name, variant) in &item.variants {
        if variant.fields.is_empty() {
            writeln!(out, "      case \"{variant_name}\":")?;
            writeln!(out, "        return {name}{variant_name}();")?;
        } else {
            writeln!(
                out,
                "      case Map() when input.keys.first == \"{variant_name}\":"
            )?;

            writeln!(
                out,
                "        final fields = input.values.first as List<Object?>;"
            )?;
            writeln!(out, "        return {name}{variant_name}(")?;

            for (index, (field_name, field)) in variant.fields.iter().enumerate() {
                write!(out, "          {}: ", AsLowerCamelCase(field_name))?;
                write_invoke_decode(out, &field.ty, &format!("input[{index}]"))?;
                writeln!(out, ",")?
            }

            writeln!(out, "        );")?;
        }
    }
    writeln!(out, "      default:")?;
    writeln!(
        out,
        "        throw ArgumentError(\"failed to decode {name} from $input\");"
    )?;
    writeln!(out, "  }}")?;

    writeln!(out, "}}")?;
    writeln!(out)?;

    for (variant_name, variant) in &item.variants {
        write_docs(out, "", &variant.docs)?;
        writeln!(out, "class {name}{variant_name} extends {name} {{")?;

        // fields
        for (field_name, field) in &variant.fields {
            write_docs(out, "  ", &field.docs)?;
            write!(out, "  final ")?;
            write_type(out, &field.ty)?;
            writeln!(out, " {};", AsLowerCamelCase(field_name))?;
        }

        if !variant.fields.is_empty() {
            writeln!(out)?;
        }

        // constructor
        writeln!(out, "  {name}{variant_name}({{")?;

        for (field_name, _) in &variant.fields {
            writeln!(out, "    required this.{},", AsLowerCamelCase(field_name))?;
        }

        writeln!(out, "  }})")?;

        writeln!(out, "}}")?;
        writeln!(out)?;
    }

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

fn write_invoke_decode(out: &mut dyn Write, ty: &Type, expr: &str) -> Result<()> {
    match ty {
        Type::Scalar(name) => match name.as_str() {
            "u8" | "u16" | "u32" | "u64" | "i8" | "i16" | "i32" | "i64" => {
                write!(out, "{expr} as int")?
            }
            "PublicRuntimeId" => write!(out, "HEX.encode({expr} as Uint8List)")?,
            "String" => write!(out, "{expr} as String")?,
            "SystemTime" => write!(out, "DateTime.fromMillisecondsSinceEpoch({expr} as int)")?,
            _ => write!(out, "{name}.decode({expr})")?,
        },
        Type::Bytes => write!(out, "{expr} as Uint8List")?,
        Type::Unit | Type::Result(..) => bail!("unsupported type: {:?}", ty),
        Type::Option(_) | Type::Vec(_) | Type::Map(_, _) => todo!(),
    }

    Ok(())
}

fn map_type(src: &str) -> &str {
    match src {
        "u8" | "u16" | "u32" | "u63" | "i8" | "i16" | "i32" | "i64" => "int",
        "PublicRuntimeId" => "String",
        "SystemTime" => "DateTime",
        _ => src,
    }
}
