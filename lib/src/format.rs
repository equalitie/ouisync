use std::fmt;

// Format the byte slice as hex with optional truncation.
// This is a helper for implementing the `LowerHex` trait.
pub(crate) fn hex(f: &mut fmt::Formatter, bytes: &[u8]) -> fmt::Result {
    let len = f
        .width()
        .map(|w| w / 2)
        .unwrap_or(bytes.len())
        .min(bytes.len());

    let (len, ellipsis) = match (len, f.sign_minus()) {
        (0, _) => (0, false),
        (len, _) if len == bytes.len() => (len, false),
        (len, true) => (len, false),
        (len, false) => (len - 1, true),
    };

    for byte in &bytes[..len] {
        write!(f, "{:02x}", byte)?;
    }

    if ellipsis {
        write!(f, "..")?;
    }

    Ok(())
}

pub(crate) struct Hex<'a>(pub &'a [u8]);

impl fmt::LowerHex for Hex<'_> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        hex(f, self.0)
    }
}
