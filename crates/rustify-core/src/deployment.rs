// Contract C2: deployment state machine. Transcribed verbatim from the
// pinned contracts; `can_transition_to` is the ONLY legality check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, sqlx::Type, serde::Serialize, serde::Deserialize)]
#[sqlx(type_name = "deployment_status", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum DeploymentStatus {
    Queued,
    InProgress,
    Finished,
    Failed,
    Cancelled,
}

impl DeploymentStatus {
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Finished | Self::Failed | Self::Cancelled)
    }

    /// The ONLY legality check. Queued→InProgress|Cancelled; InProgress→Finished|Failed|Cancelled; terminal→nothing.
    pub fn can_transition_to(self, next: Self) -> bool {
        matches!(
            (self, next),
            (Self::Queued, Self::InProgress | Self::Cancelled)
                | (
                    Self::InProgress,
                    Self::Finished | Self::Failed | Self::Cancelled
                )
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BuildPack {
    Nixpacks,
    Dockerfile,
    Static,
    DockerImage,
    DockerCompose,
}

#[cfg(test)]
mod tests {
    use super::DeploymentStatus::{self, *};

    const ALL: [DeploymentStatus; 5] = [Queued, InProgress, Finished, Failed, Cancelled];

    #[test]
    fn is_terminal_matches_contract() {
        assert!(!Queued.is_terminal());
        assert!(!InProgress.is_terminal());
        assert!(Finished.is_terminal());
        assert!(Failed.is_terminal());
        assert!(Cancelled.is_terminal());
    }

    /// Table-tests every (from, to) pair — all 25 combinations, explicit.
    #[test]
    fn transition_table_all_pairs() {
        #[rustfmt::skip]
        let expected: [(DeploymentStatus, DeploymentStatus, bool); 25] = [
            (Queued,     Queued,     false),
            (Queued,     InProgress, true),
            (Queued,     Finished,   false), // Queued→Finished is illegal
            (Queued,     Failed,     false),
            (Queued,     Cancelled,  true), // Queued→Cancelled is legal
            (InProgress, Queued,     false),
            (InProgress, InProgress, false),
            (InProgress, Finished,   true),
            (InProgress, Failed,     true),
            (InProgress, Cancelled,  true),
            (Finished,   Queued,     false),
            (Finished,   InProgress, false),
            (Finished,   Finished,   false),
            (Finished,   Failed,     false),
            (Finished,   Cancelled,  false),
            (Failed,     Queued,     false),
            (Failed,     InProgress, false),
            (Failed,     Finished,   false),
            (Failed,     Failed,     false),
            (Failed,     Cancelled,  false),
            (Cancelled,  Queued,     false),
            (Cancelled,  InProgress, false),
            (Cancelled,  Finished,   false),
            (Cancelled,  Failed,     false),
            (Cancelled,  Cancelled,  false),
        ];
        // The table above must cover the full cartesian product exactly once.
        for from in ALL {
            for to in ALL {
                assert_eq!(
                    expected
                        .iter()
                        .filter(|(f, t, _)| *f == from && *t == to)
                        .count(),
                    1,
                    "table must contain ({from:?}, {to:?}) exactly once"
                );
            }
        }
        for (from, to, legal) in expected {
            assert_eq!(
                from.can_transition_to(to),
                legal,
                "can_transition_to({from:?} -> {to:?}) should be {legal}"
            );
        }
    }

    #[test]
    fn terminal_states_reject_all_transitions() {
        for from in ALL.into_iter().filter(|s| s.is_terminal()) {
            for to in ALL {
                assert!(
                    !from.can_transition_to(to),
                    "terminal {from:?} must not transition to {to:?}"
                );
            }
        }
    }

    #[test]
    fn status_serializes_snake_case() {
        let json = |s: DeploymentStatus| serde_json::to_string(&s).unwrap();
        assert_eq!(json(Queued), "\"queued\"");
        assert_eq!(json(InProgress), "\"in_progress\"");
        assert_eq!(json(Finished), "\"finished\"");
        assert_eq!(json(Failed), "\"failed\"");
        assert_eq!(json(Cancelled), "\"cancelled\"");
    }

    #[test]
    fn buildpack_serializes_snake_case() {
        use super::BuildPack;
        let json = |b: BuildPack| serde_json::to_string(&b).unwrap();
        assert_eq!(json(BuildPack::Nixpacks), "\"nixpacks\"");
        assert_eq!(json(BuildPack::Dockerfile), "\"dockerfile\"");
        assert_eq!(json(BuildPack::Static), "\"static\"");
        assert_eq!(json(BuildPack::DockerImage), "\"docker_image\"");
        assert_eq!(json(BuildPack::DockerCompose), "\"docker_compose\"");
    }
}
