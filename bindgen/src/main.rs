use cbindgen::{Builder, EnumConfig, Language, RenameRule};
use std::path::Path;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Generate the C bindings header

    env_logger::init();

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
        .generate()?
        .write_to_file(output_path);

    Ok(())
}
