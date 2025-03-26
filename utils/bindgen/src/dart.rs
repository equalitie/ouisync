use heck::AsLowerCamelCase;
use ouisync_api_parser::{Context, Docs, SimpleEnum};
use std::io::{self, Write};

pub(crate) fn generate(ctx: &Context, out: &mut dyn Write) -> io::Result<()> {
    for (name, item) in &ctx.simple_enums {
        generate_enum(out, name, item)?;
    }

    Ok(())
}

fn generate_enum(out: &mut dyn Write, name: &str, item: &SimpleEnum) -> io::Result<()> {
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

fn write_docs(out: &mut dyn Write, prefix: &str, docs: &Docs) -> io::Result<()> {
    for line in &docs.lines {
        writeln!(out, "{prefix}///{}", line)?;
    }

    Ok(())
}
