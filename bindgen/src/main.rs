use cbindgen::{Builder, EnumConfig, Language, RenameRule};
use std::path::Path;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Generate the C bindings header

    let ffi_dir = Path::new("ffi");
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
        .with_src(ffi_dir.join("src").join("lib.rs"))
        .exclude_item("DartCObject")
        .exclude_item("DartCObjectType")
        .exclude_item("DartCObjectValue")
        .include_item("ErrorCode")
        .generate()?
        .write_to_file(output_path);

    Ok(())
}
