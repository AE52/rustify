//! Contract C4: server→client WebSocket events and their JSON envelope.
//!
//! `rustify-server` emits these; `web` consumes them. Envelope shape is
//! exactly `{ "channel": ..., "event": ..., "data": ... }` with snake_case
//! event names. Channel routing follows Coolify's broadcast channels:
//! team-wide events go to the team channel
//! (coolify/app/Events/ApplicationStatusChanged.php:26-33 broadcasts on
//! `PrivateChannel("team.{teamId}")`), deployment log/status events go to a
//! per-deployment channel, and server validation streams go to
//! `server:<uuid>` (contract C5, `POST /servers/{uuid}/validate`).
//! Phase 1 has a single team, so the team channel is `team:default`.

use crate::deployment::DeploymentStatus;
use crate::logline::LogLine;

#[derive(Debug, Clone)]
pub enum WsEvent {
    DeploymentLogAppended {
        deployment_uuid: String,
        lines: Vec<LogLine>,
    },
    DeploymentStatusChanged {
        deployment_uuid: String,
        application_uuid: String,
        status: DeploymentStatus,
    },
    ApplicationStatusChanged {
        application_uuid: String,
        status: String,
    },
    ServerReachabilityChanged {
        server_uuid: String,
        reachable: bool,
    },
    ServerValidationLog {
        server_uuid: String,
        lines: Vec<LogLine>,
    },
}

impl WsEvent {
    /// The channel this event is published on:
    /// `deployment:<uuid>` / `team:default` / `server:<uuid>` per C4.
    pub fn channel(&self) -> String {
        match self {
            Self::DeploymentLogAppended {
                deployment_uuid, ..
            }
            | Self::DeploymentStatusChanged {
                deployment_uuid, ..
            } => format!("deployment:{deployment_uuid}"),
            Self::ApplicationStatusChanged { .. } | Self::ServerReachabilityChanged { .. } => {
                "team:default".to_string()
            }
            Self::ServerValidationLog { server_uuid, .. } => format!("server:{server_uuid}"),
        }
    }

    /// The snake_case event name per C4.
    pub fn event_name(&self) -> &'static str {
        match self {
            Self::DeploymentLogAppended { .. } => "deployment_log_appended",
            Self::DeploymentStatusChanged { .. } => "deployment_status_changed",
            Self::ApplicationStatusChanged { .. } => "application_status_changed",
            Self::ServerReachabilityChanged { .. } => "server_reachability_changed",
            Self::ServerValidationLog { .. } => "server_validation_log",
        }
    }

    /// The full C4 JSON envelope: `{ "channel": ..., "event": ..., "data": ... }`.
    pub fn envelope(&self) -> serde_json::Value {
        let data = match self {
            Self::DeploymentLogAppended {
                deployment_uuid,
                lines,
            } => serde_json::json!({
                "deployment_uuid": deployment_uuid,
                "lines": lines,
            }),
            Self::DeploymentStatusChanged {
                deployment_uuid,
                application_uuid,
                status,
            } => serde_json::json!({
                "deployment_uuid": deployment_uuid,
                "application_uuid": application_uuid,
                "status": status,
            }),
            Self::ApplicationStatusChanged {
                application_uuid,
                status,
            } => serde_json::json!({
                "application_uuid": application_uuid,
                "status": status,
            }),
            Self::ServerReachabilityChanged {
                server_uuid,
                reachable,
            } => serde_json::json!({
                "server_uuid": server_uuid,
                "reachable": reachable,
            }),
            Self::ServerValidationLog { server_uuid, lines } => serde_json::json!({
                "server_uuid": server_uuid,
                "lines": lines,
            }),
        };
        serde_json::json!({
            "channel": self.channel(),
            "event": self.event_name(),
            "data": data,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use serde_json::json;

    fn line(order: i64) -> LogLine {
        LogLine {
            order,
            kind: "stdout".into(),
            content: "hello".into(),
            hidden: false,
            batch: 1,
            timestamp: chrono::Utc.with_ymd_and_hms(2026, 7, 2, 12, 0, 0).unwrap(),
        }
    }

    #[test]
    fn deployment_log_appended_envelope() {
        let ev = WsEvent::DeploymentLogAppended {
            deployment_uuid: "dep123".into(),
            lines: vec![line(1), line(2)],
        };
        assert_eq!(ev.channel(), "deployment:dep123");
        assert_eq!(
            ev.envelope(),
            json!({
                "channel": "deployment:dep123",
                "event": "deployment_log_appended",
                "data": {
                    "deployment_uuid": "dep123",
                    "lines": [
                        {"order": 1, "kind": "stdout", "content": "hello", "hidden": false,
                         "batch": 1, "timestamp": "2026-07-02T12:00:00Z"},
                        {"order": 2, "kind": "stdout", "content": "hello", "hidden": false,
                         "batch": 1, "timestamp": "2026-07-02T12:00:00Z"}
                    ]
                }
            })
        );
    }

    #[test]
    fn deployment_status_changed_envelope() {
        let ev = WsEvent::DeploymentStatusChanged {
            deployment_uuid: "dep123".into(),
            application_uuid: "app456".into(),
            status: DeploymentStatus::InProgress,
        };
        assert_eq!(ev.channel(), "deployment:dep123");
        assert_eq!(
            ev.envelope(),
            json!({
                "channel": "deployment:dep123",
                "event": "deployment_status_changed",
                "data": {
                    "deployment_uuid": "dep123",
                    "application_uuid": "app456",
                    "status": "in_progress"
                }
            })
        );
    }

    #[test]
    fn application_status_changed_envelope() {
        let ev = WsEvent::ApplicationStatusChanged {
            application_uuid: "app456".into(),
            status: "running:healthy".into(),
        };
        assert_eq!(ev.channel(), "team:default");
        assert_eq!(
            ev.envelope(),
            json!({
                "channel": "team:default",
                "event": "application_status_changed",
                "data": {
                    "application_uuid": "app456",
                    "status": "running:healthy"
                }
            })
        );
    }

    #[test]
    fn server_reachability_changed_envelope() {
        let ev = WsEvent::ServerReachabilityChanged {
            server_uuid: "srv789".into(),
            reachable: false,
        };
        assert_eq!(ev.channel(), "team:default");
        assert_eq!(
            ev.envelope(),
            json!({
                "channel": "team:default",
                "event": "server_reachability_changed",
                "data": {
                    "server_uuid": "srv789",
                    "reachable": false
                }
            })
        );
    }

    #[test]
    fn server_validation_log_envelope() {
        let ev = WsEvent::ServerValidationLog {
            server_uuid: "srv789".into(),
            lines: vec![line(1)],
        };
        assert_eq!(ev.channel(), "server:srv789");
        assert_eq!(
            ev.envelope(),
            json!({
                "channel": "server:srv789",
                "event": "server_validation_log",
                "data": {
                    "server_uuid": "srv789",
                    "lines": [
                        {"order": 1, "kind": "stdout", "content": "hello", "hidden": false,
                         "batch": 1, "timestamp": "2026-07-02T12:00:00Z"}
                    ]
                }
            })
        );
    }
}
