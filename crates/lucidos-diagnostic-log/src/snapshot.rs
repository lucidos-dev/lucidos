//! Private engine-to-gateway snapshot. Render every record before exporting it.
use crate::writer::Record;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const FAILURE_WINDOW_DAYS: i32 = 7;
pub const MAX_FAILURE_EVENTS: i64 = 2000;
pub const MAX_EVENT_PAYLOAD_BYTES: i32 = 2048;
pub const MAX_CURRENT_FAULTS: i64 = 32;

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Snapshot {
    pub events: Vec<Record>,
    pub faults: Vec<Record>,
    pub versions: BTreeMap<String, String>,
    pub configured: BTreeMap<String, bool>,
    pub healthy: bool,
}
