use std::{env, path::PathBuf};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Generate the C bindings header

    let project_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR")?);
    let project_name = env::var("CARGO_PKG_NAME")?;

    cbindgen::Builder::new()
        .with_config(cbindgen::Config {
            language: cbindgen::Language::C,
            ..Default::default()
        })
        .with_src(project_dir.join("src").join("ffi").join("mod.rs"))
        .exclude_item("DartCObject")
        .exclude_item("DartCObjectType")
        .exclude_item("DartCObjectValue")
        .generate()?
        .write_to_file(
            project_dir
                .join("target")
                .join(format!("{}_bindings.h", project_name)),
        );

    Ok(())
}
