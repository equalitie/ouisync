use std::sync::Arc;
use zeroize::Zeroizing;

/// A simple wrapper over String to avoid certain kinds of attack. For more elaboration please see
/// the documentation for the SecretKey structure.
#[derive(Clone)]
pub struct Password(Arc<Zeroizing<String>>);

impl Password {
    pub fn new(pwd: &str) -> Self {
        Self(Arc::new(Zeroizing::new(pwd.to_owned())))
    }
}

impl AsRef<str> for Password {
    fn as_ref(&self) -> &str {
        self.0.as_ref()
    }
}
