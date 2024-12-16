use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct PexConfig {
    // Is sending contacts over PEX enabled?
    pub send: bool,
    // Is receiving contacts over PEX enabled?
    pub recv: bool,
}

impl Default for PexConfig {
    fn default() -> Self {
        Self {
            send: true,
            recv: true,
        }
    }
}
