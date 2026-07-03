//! WebSocket event envelope (Contract C4) shared by every crate that emits or
//! consumes realtime updates.
//!
//! The deploy engine and server-setup handler broadcast [`WsEvent`]s onto a
//! `tokio::sync::broadcast` channel; `rustify-server` serialises them straight
//! to clients. The struct maps 1:1 onto the pinned C4 JSON shape
//! `{ "channel": ..., "event": ..., "data": {...} }`, so serialisation is the
//! identity transform and no bespoke encoder is needed.

use serde_json::{Value, json};

use crate::deployment::DeploymentStatus;
use crate::logline::LogLine;

/// A realtime event addressed to a channel (Contract C4). Serialises verbatim
/// to the C4 envelope.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct WsEvent {
    pub channel: String,
    pub event: String,
    pub data: Value,
}

impl WsEvent {
    /// Construct an arbitrary envelope. Prefer the named constructors below.
    pub fn new(channel: impl Into<String>, event: impl Into<String>, data: Value) -> Self {
        Self {
            channel: channel.into(),
            event: event.into(),
            data,
        }
    }

    /// A log line was appended to a deployment (`deployment:<uuid>`).
    pub fn deployment_log_appended(deployment_uuid: &str, line: &LogLine) -> Self {
        Self::new(
            format!("deployment:{deployment_uuid}"),
            "deployment_log_appended",
            serde_json::to_value(line).unwrap_or(Value::Null),
        )
    }

    /// A deployment changed state (`deployment:<uuid>`).
    pub fn deployment_status_changed(deployment_uuid: &str, status: DeploymentStatus) -> Self {
        Self::new(
            format!("deployment:{deployment_uuid}"),
            "deployment_status_changed",
            json!({ "uuid": deployment_uuid, "status": status }),
        )
    }

    /// An application's container status changed (`application:<uuid>`).
    pub fn application_status_changed(application_uuid: &str, status: &str) -> Self {
        Self::new(
            format!("application:{application_uuid}"),
            "application_status_changed",
            json!({ "uuid": application_uuid, "status": status }),
        )
    }

    /// A server's reachability/usability changed (`server:<uuid>`).
    pub fn server_reachability_changed(server_uuid: &str, reachable: bool, usable: bool) -> Self {
        Self::new(
            format!("server:{server_uuid}"),
            "server_reachability_changed",
            json!({ "uuid": server_uuid, "reachable": reachable, "usable": usable }),
        )
    }

    /// A standalone database's container status changed (`database:<uuid>`).
    pub fn database_status_changed(database_uuid: &str, status: &str) -> Self {
        Self::new(
            format!("database:{database_uuid}"),
            "database_status_changed",
            json!({ "uuid": database_uuid, "status": status }),
        )
    }

    /// A line of output from a server-level task (validation/setup), streamed
    /// to `server:<uuid>`.
    pub fn server_log(server_uuid: &str, kind: &str, content: &str) -> Self {
        Self::new(
            format!("server:{server_uuid}"),
            "server_log",
            json!({ "uuid": server_uuid, "kind": kind, "content": content }),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serialises_to_c4_envelope() {
        let ev = WsEvent::deployment_status_changed("dep1", DeploymentStatus::InProgress);
        let v: Value = serde_json::to_value(&ev).unwrap();
        assert_eq!(v["channel"], "deployment:dep1");
        assert_eq!(v["event"], "deployment_status_changed");
        assert_eq!(v["data"]["status"], "in_progress");
    }

    #[test]
    fn reachability_event_shape() {
        let ev = WsEvent::server_reachability_changed("srv1", true, false);
        let v = serde_json::to_value(&ev).unwrap();
        assert_eq!(v["channel"], "server:srv1");
        assert_eq!(v["data"]["reachable"], true);
        assert_eq!(v["data"]["usable"], false);
    }
}
