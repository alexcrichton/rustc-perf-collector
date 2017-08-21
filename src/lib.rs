#[macro_use] extern crate serde_derive;
extern crate chrono;

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Pass {
    pub name: String,
    pub time: f64,
    pub mem: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Stat {
    pub name: String,
    pub cnt: f64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Run {
    pub name: String,
    pub passes: Vec<Pass>,
    #[serde(default)]
    pub stats: Vec<Stat>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Patch {
    pub patch: String,
    pub name: String,
    pub runs: Vec<Run>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Commit {
    pub sha: String,
    pub date: DateTime<Utc>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CommitData {
    pub commit: Commit,
    // String in Result is the output of the command that failed
    pub benchmarks: BTreeMap<String, Result<Vec<Patch>, String>>,
    pub triple: String,
}
