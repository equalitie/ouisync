use cbindgen::{Builder, EnumConfig, Language, RenameRule};
use std::path::Path;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Generate the C bindings header

    let output_path = Path::new("target").join("bindings.h");

    Builder::new()
        .with_config(cbindgen::Config {
            language: Language::C,
            enumeration: EnumConfig {
                rename_variants: RenameRule::CamelCase,
                ..Default::default()
            },
            ..Default::default()
        })
        .with_src(Path::new("ffi").join("src").join("lib.rs"))
        .with_src(Path::new("bridge").join("src").join("constants.rs"))
        .with_src(Path::new("bridge").join("src").join("error.rs"))
        .with_src(Path::new("bridge").join("src").join("registry.rs"))
        .generate()?
        .write_to_file(output_path);

    Ok(())
}
