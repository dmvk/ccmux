use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Starting,
    Working,
    Waiting,
    Idle,
    Done,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub status: Status,
    pub tool: Option<String>,
    pub msg: Option<String>,
    pub ts: u64,
    pub seq: u64,
    pub dir: Option<String>,
}
