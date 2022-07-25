use crate::error::Result;
use std::{fmt::Debug, future::Future, mem};

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

pub(crate) async fn instrument<F, T>(label: &str, task: F) -> Result<T>
where
    F: Future<Output = Result<T>>,
    T: Debug,
{
    struct LogOnDrop<'a>(&'a str);

    impl<'a> Drop for LogOnDrop<'a> {
        fn drop(&mut self) {
            tracing::trace!("{}: [cancelled]", self.0);
        }
    }

    let log_on_drop = LogOnDrop(label);
    let result = task.await;

    tracing::trace!("{}: {:?}", label, result);
    mem::forget(log_on_drop);

    result
}
