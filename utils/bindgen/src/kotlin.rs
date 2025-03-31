use heck::AsShoutySnakeCase;

use ouisync_api_parser::{Context, Docs, EnumRepr, Item, SimpleEnum};
use std::io::{self, Write};

pub(crate) fn generate(ctx: &Context, out: &mut dyn Write) -> io::Result<()> {
    writeln!(out, "package org.equalitie.ouisync.lib")?;
    writeln!(out)?;

    for (name, item) in &ctx.items {
        match item {
            Item::SimpleEnum(item) => generate_enum(name, item, out)?,
            Item::ComplexEnum(_item) => todo!(),
            Item::Struct(_item) => todo!(),
            Item::Newtype(_item) => todo!(),
        }
    }

    Ok(())
}

fn generate_enum(name: &str, item: &SimpleEnum, out: &mut dyn Write) -> io::Result<()> {
    let repr = match item.repr {
        EnumRepr::U8 => "Byte",
        EnumRepr::U16 => "Short",
        EnumRepr::U32 => "Int",
        EnumRepr::U64 => "Long",
    };

    write_docs(out, "", &item.docs)?;
    writeln!(out, "enum class {name} {{")?;

    for (variant_name, variant) in &item.variants {
        write_docs(out, "    ", &variant.docs)?;
        writeln!(out, "    {},", AsShoutySnakeCase(variant_name))?;
    }

    writeln!(out)?;
    writeln!(out, "    ;")?;
    writeln!(out)?;

    // decode
    writeln!(out, "    companion object {{")?;

    writeln!(out, "        fun decode(n: {repr}): {name} = when (n) {{")?;

    for (variant_name, variant) in &item.variants {
        writeln!(
            out,
            "            {}.to{repr}() -> {}",
            variant.value,
            AsShoutySnakeCase(variant_name)
        )?;
    }

    writeln!(out, "            else -> throw IllegalArgumentException()")?;
    writeln!(out, "        }}")?;

    writeln!(out, "    }}")?;
    writeln!(out)?;

    // encode
    writeln!(out, "    fun encode(): {repr} = when (this) {{")?;

    for (variant_name, variant) in &item.variants {
        writeln!(
            out,
            "        {} -> {}",
            AsShoutySnakeCase(variant_name),
            variant.value
        )?;
    }

    writeln!(out, "    }}.to{repr}()")?;

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
