use serde::{Deserialize, Serialize};

use crate::brew::BrewDiscoveryResult;
use crate::config::ObservedConfigFile;
use crate::diagnostic::Diagnostic;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScanReport {
    pub homebrew: BrewDiscoveryResult,
    pub configs: Vec<ObservedConfigFile>,
    pub diagnostics: Vec<Diagnostic>,
}

impl ScanReport {
    pub fn new(
        homebrew: BrewDiscoveryResult,
        configs: Vec<ObservedConfigFile>,
        diagnostics: Vec<Diagnostic>,
    ) -> Self {
        Self {
            homebrew,
            configs,
            diagnostics,
        }
    }
}
