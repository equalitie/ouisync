use heck::AsShoutySnakeCase;

use crate::parse::{Enum, EnumRepr, Source};
use std::io::{self, Write};

pub(crate) fn generate(source: &Source, out: &mut dyn Write) -> io::Result<()> {
    writeln!(out, "package org.equalitie.ouisync.lib")?;
    writeln!(out)?;

    for (name, value) in &source.enums {
        generate_enum(name, value, out)?;
    }

    Ok(())
}

fn generate_enum(name: &str, value: &Enum, out: &mut dyn Write) -> io::Result<()> {
    let repr = match value.repr {
        EnumRepr::U8 => "Byte",
        EnumRepr::U16 => "Short",
        EnumRepr::U32 => "Int",
        EnumRepr::U64 => "Long",
    };

    write_doc(out, "", &value.doc)?;
    writeln!(out, "enum class {name} {{")?;

    for variant in &value.variants {
        write_doc(out, "    ", &variant.doc)?;
        writeln!(out, "    {},", AsShoutySnakeCase(&variant.name))?;
    }

    writeln!(out)?;
    writeln!(out, "    ;")?;
    writeln!(out)?;

    // decode
    writeln!(out, "    companion object {{")?;

    writeln!(out, "        fun decode(n: {repr}): {name} = when (n) {{")?;

    for variant in &value.variants {
        writeln!(
            out,
            "            {}.to{repr}() -> {}",
            variant.value,
            AsShoutySnakeCase(&variant.name)
        )?;
    }

    writeln!(out, "            else -> throw IllegalArgumentException()")?;
    writeln!(out, "        }}")?;

    writeln!(out, "    }}")?;
    writeln!(out)?;

    // encode
    writeln!(out, "    fun encode(): {repr} = when (this) {{")?;

    for variant in &value.variants {
        writeln!(
            out,
            "        {} -> {}",
            AsShoutySnakeCase(&variant.name),
            variant.value
        )?;
    }

    writeln!(out, "    }}.to{repr}()")?;

    writeln!(out, "}}")?;
    writeln!(out)?;

    Ok(())
}

fn write_doc(out: &mut dyn Write, prefix: &str, doc: &str) -> io::Result<()> {
    if doc.is_empty() {
        return Ok(());
    }

    writeln!(out, "{prefix}/**")?;

    for line in doc.lines() {
        writeln!(out, "{prefix} *{}", line)?;
    }

    writeln!(out, "{prefix} */")?;

    Ok(())
}
