use std::{fmt, str::FromStr};

use serde::{Deserialize, Serialize};

/// How to color log messages
#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
pub enum LogColor {
    /// Awlays color
    Always,
    /// Never color
    Never,
    /// Color only when printing to a terminal but not when redirected to a file or a pipe.
    Auto,
}

impl Default for LogColor {
    fn default() -> Self {
        Self::Auto
    }
}

impl fmt::Display for LogColor {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::Always => write!(f, "always"),
            Self::Never => write!(f, "never"),
            Self::Auto => write!(f, "auto"),
        }
    }
}

impl FromStr for LogColor {
    type Err = ParseLogColorError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_lowercase().as_str() {
            "always" => Ok(Self::Always),
            "never" => Ok(Self::Never),
            "auto" => Ok(Self::Auto),
            _ => Err(ParseLogColorError),
        }
    }
}

#[derive(Debug)]
pub struct ParseLogColorError;

impl fmt::Display for ParseLogColorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid log color")
    }
}

impl std::error::Error for ParseLogColorError {}
