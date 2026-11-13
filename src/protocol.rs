//! Wire types shared by the coordinator, native workers and browser workers.
//!
//! The protocol is deliberately pull-based plain HTTP + JSON, so that anything
//! with a browser (phones, tablets, smart TVs) can join without installing anything:
//!
//! 1. `POST /api/register` -> worker id
//! 2. `POST /api/sync` every second: worker sends its stats delta, gets back its
//!    current assignment (how many virtual users to run, against which paths).

use crate::stats::Stats;
use serde::{Deserialize, Serialize};

/// Header (or `?token=` query parameter) carrying the fleet join token.
pub const TOKEN_HEADER: &str = "x-strezz-token";

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum WorkerKind {
    Native,
    Browser,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegisterRequest {
    pub name: String,
    pub kind: WorkerKind,
    /// Maximum concurrent virtual users this device can hold
    /// (for native workers: derived from the open-file limit).
    pub capacity: u32,
    /// Free-form platform description (OS, arch, user agent...).
    #[serde(default)]
    pub platform: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegisterResponse {
    pub worker_id: String,
    pub sync_interval_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncRequest {
    pub worker_id: String,
    /// The run the stats delta belongs to (0 = none).
    pub run_id: u64,
    /// Virtual users currently alive on the worker.
    pub active_users: u32,
    /// Stats collected since the previous sync.
    #[serde(default)]
    pub delta: Stats,
    /// Updated capacity, for workers that tune themselves (browser auto-tune).
    #[serde(default)]
    pub capacity: Option<u32>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Assignment {
    /// 0 when the fleet is idle.
    pub run_id: u64,
    /// Base URL of the site under test.
    pub target: String,
    /// Paths (relative to `target`) that virtual users pick from at random.
    pub paths: Vec<String>,
    /// Virtual users this worker should be running right now.
    pub users: u32,
    /// How many users per second this worker may add while ramping up.
    pub spawn_per_sec: f64,
    /// Think time between two requests of one virtual user.
    pub wait_min_ms: u64,
    pub wait_max_ms: u64,
    pub timeout_ms: u64,
}

impl Assignment {
    /// No run: zero users, nothing to hit.
    pub fn idle() -> Self {
        Self::default()
    }
}

/// Body of `POST /api/run/start`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StartRun {
    /// Total virtual users across the whole fleet; 0 = every device runs at
    /// its full capacity (follows capacity changes and devices joining).
    pub users: u32,
    /// Fleet-wide users added per second while ramping up.
    #[serde(default = "default_spawn_rate")]
    pub spawn_rate: f64,
    /// Stop automatically after this many seconds (0 = run until stopped).
    #[serde(default)]
    pub duration_secs: u64,
    #[serde(default = "default_wait_min")]
    pub wait_min_ms: u64,
    #[serde(default = "default_wait_max")]
    pub wait_max_ms: u64,
    #[serde(default = "default_timeout")]
    pub timeout_ms: u64,
}

fn default_spawn_rate() -> f64 {
    50.0
}
fn default_wait_min() -> u64 {
    1000
}
fn default_wait_max() -> u64 {
    3000
}
fn default_timeout() -> u64 {
    30_000
}
