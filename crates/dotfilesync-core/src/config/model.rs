use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigFileState {
    Matching,
    Changed,
    Missing,
    MachineLocal,
    Excluded,
    SymlinkToSource,
    SymlinkExternal,
    BrokenSymlink,
    SourceMissing,
    Unreadable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObservedConfigFile {
    pub source_path: PathBuf,
    pub target_path: PathBuf,
    pub state: ConfigFileState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_size: Option<u64>,
    pub is_symlink: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub symlink_target: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diagnostic: Option<String>,
}
