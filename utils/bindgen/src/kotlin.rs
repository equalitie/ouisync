use heck::AsShoutySnakeCase;

use crate::parse::{Enum, EnumRepr, Source};
use std::io::{self, Write};

pub(crate) fn generate(source: &Source, out: &mut dyn Write) -> io::Result<()> {
    writeln!(out, "package org.equalitie.ouisync")?;
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

    writeln!(out, "enum class {name} {{")?;

    for variant in &value.variants {
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
