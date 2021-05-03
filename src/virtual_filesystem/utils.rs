use std::{
    fmt,
    ops::{Deref, DerefMut},
};

// Helper for formatting comma-separated list of heterogenous optional values.
pub struct FormatOptionScope {
    sep: bool,
}

impl FormatOptionScope {
    pub fn new() -> Self {
        Self { sep: false }
    }

    pub fn add<'a, T>(&mut self, label: &'a str, value: Option<T>) -> FormatOption<'a, T> {
        let sep = self.sep;

        if value.is_some() {
            self.sep = true;
        }

        FormatOption { sep, label, value }
    }
}

pub struct FormatOption<'a, T> {
    sep: bool,
    label: &'a str,
    value: Option<T>,
}

macro_rules! impl_fmt {
    ($trait:ident, $format_string:literal) => {
        impl<T> fmt::$trait for FormatOption<'_, T>
        where
            T: fmt::$trait,
        {
            fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
                if let Some(value) = &self.value {
                    if self.sep {
                        write!(f, ", ")?;
                    }

                    write!(f, "{}=", self.label)?;
                    write!(f, $format_string, value)?;
                }

                Ok(())
            }
        }
    };
}

impl_fmt!(Display, "{}");
impl_fmt!(Debug, "{:?}");
impl_fmt!(Octal, "{:#o}");
impl_fmt!(LowerHex, "{:#x}");

// A wrapper that can contain either a value or a mutable reference. Similar to `Cow` but mutable
// and it doesn't require `T` to be `ToOwned`.
pub enum MaybeOwnedMut<'a, T> {
    Owned(T),
    Borrowed(&'a mut T),
}

impl<'a, T> Deref for MaybeOwnedMut<'a, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        match self {
            Self::Owned(v) => v,
            Self::Borrowed(r) => *r,
        }
    }
}

impl<'a, T> DerefMut for MaybeOwnedMut<'a, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        match self {
            Self::Owned(v) => v,
            Self::Borrowed(r) => *r,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_option() {
        let mut scope = FormatOptionScope::new();
        assert_eq!(format!("{}", scope.add("foo", None::<i32>)), "");

        let mut scope = FormatOptionScope::new();
        assert_eq!(format!("{}", scope.add("foo", Some(0))), "foo=0");

        let mut scope = FormatOptionScope::new();
        assert_eq!(
            format!(
                "{}{}",
                scope.add("foo", None::<i32>),
                scope.add("bar", None::<i32>)
            ),
            ""
        );

        let mut scope = FormatOptionScope::new();
        assert_eq!(
            format!(
                "{}{}",
                scope.add("foo", Some(0)),
                scope.add("bar", None::<i32>)
            ),
            "foo=0"
        );

        let mut scope = FormatOptionScope::new();
        assert_eq!(
            format!(
                "{}{}",
                scope.add("foo", None::<i32>),
                scope.add("bar", Some(1))
            ),
            "bar=1"
        );

        let mut scope = FormatOptionScope::new();
        assert_eq!(
            format!("{}{}", scope.add("foo", Some(0)), scope.add("bar", Some(1))),
            "foo=0, bar=1"
        );
    }
}
