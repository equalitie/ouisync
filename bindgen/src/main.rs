use cbindgen::{Builder, Language};
use std::path::Path;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Generate the C bindings header

    let lib_dir = Path::new("lib");
    let output_path = Path::new("target").join("bindings.h");

    Builder::new()
        .with_config(cbindgen::Config {
            language: Language::C,
            ..Default::default()
        })
        .with_src(lib_dir.join("src").join("ffi").join("mod.rs"))
        .with_src(
            lib_dir
                .join("src")
                .join("access_control")
                .join("access_mode.rs"),
        )
        .exclude_item("DartCObject")
        .exclude_item("DartCObjectType")
        .exclude_item("DartCObjectValue")
        .generate()?
        .write_to_file(output_path);

    Ok(())
}
