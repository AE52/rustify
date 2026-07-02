// Contract C3: log line shape (DB row + WS payload; used by deploy, db,
// server, web). Transcribed verbatim from the pinned contracts.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct LogLine {
    pub order: i64,      // monotonic per deployment
    pub kind: String,    // "stdout" | "stderr" | "info"
    pub content: String, // redacted already
    pub hidden: bool,    // internal commands hidden from UI by default
    pub batch: i32,      // command batch number
    pub timestamp: chrono::DateTime<chrono::Utc>,
}
