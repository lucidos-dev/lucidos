use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const MAX_COMPRESSED_BYTES: usize = 32 * 1024 * 1024;
pub const MAX_EXPANDED_BYTES: usize = 256 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Source {
    Engine,
    Gateway,
    Client,
    ClientRelaunch,
    Events,
    Faults,
    EngineStatus,
    Config,
    LastDeath,
    BootFailure,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Missing {
    NotPresent,
    Unreadable,
    Invalid,
    EngineUnavailable,
    Unsupported,
    NotRecorded,
}
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct SourceStatus {
    pub missing: Option<Missing>,
    pub lines: usize,
    pub invalid_lines: usize,
    pub unreadable_segments: usize,
}
#[derive(Debug, Serialize, Deserialize)]
pub struct Manifest {
    pub format_version: u32,
    pub sources: BTreeMap<Source, SourceStatus>,
    pub trimmed_lines: usize,
    pub gateway_version: String,
    pub gateway_build: String,
    pub release: String,
    pub versions: BTreeMap<String, String>,
    pub missing_versions: BTreeMap<String, Missing>,
    pub failure_window_days: i32,
    pub failure_row_limit: i64,
    pub configured: BTreeMap<String, bool>,
    pub engine_healthy: Option<bool>,
}
#[derive(Debug, Serialize, Deserialize)]
pub struct TimelineLine {
    pub ts: String,
    pub source: Source,
    pub pid: u32,
    pub message: String,
}
#[derive(Debug, Serialize, Deserialize)]
pub struct Bundle {
    pub manifest: Manifest,
    pub timeline: Vec<TimelineLine>,
}
