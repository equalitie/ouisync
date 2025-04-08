use proc_macro::TokenStream;

/// Marks items to be used for the client API generation.
#[proc_macro_attribute]
pub fn api(_attr: TokenStream, item: TokenStream) -> TokenStream {
    // Here we only strip the `api` attribute to make the code compilable. The actuall processing
    // happens in utils/bindgen and in service/build.rs
    item
}
