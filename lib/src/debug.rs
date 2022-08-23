use std::{fmt, future::Future, time::Duration};
use tokio::{pin, time};

#[derive(Clone)]
pub struct DebugPrinter {
    // Used for indentation
    level: usize,
    prefix: String,
}

impl DebugPrinter {
    pub fn new() -> Self {
        Self {
            level: 0,
            prefix: "".into(),
        }
    }

    pub fn debug<T: std::fmt::Debug>(&self, v: &T) {
        // https://stackoverflow.com/a/42273813/273348
        println!(
            "{}{:indent$}{:?}",
            self.prefix,
            "",
            v,
            indent = (2 * self.level)
        );
    }

    pub fn display<T: std::fmt::Display>(&self, v: &T) {
        println!(
            "{}{:indent$}{}",
            self.prefix,
            "",
            v,
            indent = (2 * self.level)
        );
    }

    pub fn indent(&self) -> Self {
        Self {
            level: self.level + 1,
            prefix: self.prefix.clone(),
        }
    }

    pub fn prefix(mut self, p: &str) -> Self {
        self.prefix = p.into();
        self
    }
}

impl Default for DebugPrinter {
    fn default() -> Self {
        DebugPrinter::new()
    }
}

/// Run `fut` into completion but if it takes more than `timeout`, log the given warning
/// message.
pub(crate) async fn warn_slow<F, M>(timeout: Duration, message: M, fut: F) -> F::Output
where
    F: Future,
    M: fmt::Display,
{
    pin!(fut);

    match time::timeout(timeout, &mut fut).await {
        Ok(output) => output,
        Err(_) => {
            tracing::warn!("{}", message);
            fut.await
        }
    }
}
