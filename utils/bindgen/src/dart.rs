use crate::parse::{Enum, Source};
use heck::AsLowerCamelCase;
use std::io::{self, Write};

pub(crate) fn generate(source: &Source, out: &mut dyn Write) -> io::Result<()> {
    for (name, value) in &source.enums {
        generate_enum(name, value, out)?;
    }

    Ok(())
}

fn generate_enum(name: &str, value: &Enum, out: &mut dyn Write) -> io::Result<()> {
    writeln!(out, "enum {name} {{")?;

    for variant in &value.variants {
        writeln!(out, "  {},", AsLowerCamelCase(&variant.name))?;
    }

    writeln!(out, "  ;")?;
    writeln!(out)?;

    // decode
    writeln!(out, "  static {} decode(int n) {{", name)?;
    writeln!(out, "    switch (n) {{")?;

    for variant in &value.variants {
        writeln!(
            out,
            "      case {}: return {}.{};",
            variant.value,
            name,
            AsLowerCamelCase(&variant.name)
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

    for variant in &value.variants {
        writeln!(
            out,
            "      case {}.{}: return {};",
            name,
            AsLowerCamelCase(&variant.name),
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
