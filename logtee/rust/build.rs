use std::env;

fn main() {
    // Use 16kB page alignment on android to satisfy play store requirements.
    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("android")
        && env::var("CARGO_CFG_TARGET_POINTER_WIDTH").as_deref() == Ok("64")
    {
        println!("cargo:rustc-link-arg-cdylib=-Wl,-z,max-page-size=16384");
    }
}
