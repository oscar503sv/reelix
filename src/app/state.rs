//! The recorder state machine.
//!
//! This is the single source of truth for "what is the recorder doing".
//! It is a pure function of (state, event) so it can be unit-tested without
//! GTK, GStreamer or a portal.

use std::fmt;
use std::path::PathBuf;

use crate::error::AppError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecorderState {
    Idle,
    /// The portal dialog is open; the user is picking a screen/window.
    SelectingSource,
    /// A source was chosen; the pipeline is being built and started.
    Preparing,
    Recording,
    Paused,
    /// EOS was sent; waiting for the muxer to finalize the file.
    Stopping,
    /// The file is finalized; writing metadata.
    Processing,
    Completed {
        path: PathBuf,
    },
    Failed {
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecorderEvent {
    StartRequested,
    SourceSelected,
    SelectionCancelled,
    PipelineStarted,
    PauseRequested,
    ResumeRequested,
    StopRequested,
    /// The muxer finished writing the file (EOS reached the sink).
    Finalized,
    MetadataWritten {
        path: PathBuf,
    },
    /// Discard the current operation and return to idle.
    Cancel,
    Error {
        reason: String,
    },
    /// Acknowledge a terminal state (Completed/Failed) and go back to idle.
    Reset,
}

impl RecorderState {
    pub fn is_busy(&self) -> bool {
        !matches!(
            self,
            Self::Idle | Self::Completed { .. } | Self::Failed { .. }
        )
    }

    /// Whether the capture pipeline is live (recording or paused).
    pub fn is_capturing(&self) -> bool {
        matches!(self, Self::Recording | Self::Paused)
    }

    /// Computes the next state. Returns an error for transitions that make no
    /// sense (e.g. pausing while idle) so callers never silently corrupt state.
    pub fn transition(&self, event: &RecorderEvent) -> Result<RecorderState, AppError> {
        use RecorderEvent as E;
        use RecorderState as S;

        let next = match (self, event) {
            // Errors and cancellation are accepted from any non-terminal state.
            (s, E::Error { reason }) if s.is_busy() => S::Failed {
                reason: reason.clone(),
            },
            (s, E::Cancel) if s.is_busy() => S::Idle,

            (S::Idle | S::Completed { .. } | S::Failed { .. }, E::StartRequested) => {
                S::SelectingSource
            }
            (S::Completed { .. } | S::Failed { .. }, E::Reset) => S::Idle,

            (S::SelectingSource, E::SourceSelected) => S::Preparing,
            (S::SelectingSource, E::SelectionCancelled) => S::Idle,

            (S::Preparing, E::PipelineStarted) => S::Recording,

            (S::Recording, E::PauseRequested) => S::Paused,
            (S::Paused, E::ResumeRequested) => S::Recording,
            (S::Recording | S::Paused, E::StopRequested) => S::Stopping,

            (S::Stopping, E::Finalized) => S::Processing,
            (S::Processing, E::MetadataWritten { path }) => S::Completed { path: path.clone() },

            (s, e) => {
                return Err(AppError::InvalidTransition(format!("{s} + {e:?}")));
            }
        };
        Ok(next)
    }
}

impl fmt::Display for RecorderState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Self::Idle => "Idle",
            Self::SelectingSource => "SelectingSource",
            Self::Preparing => "Preparing",
            Self::Recording => "Recording",
            Self::Paused => "Paused",
            Self::Stopping => "Stopping",
            Self::Processing => "Processing",
            Self::Completed { .. } => "Completed",
            Self::Failed { .. } => "Failed",
        };
        f.write_str(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use RecorderEvent as E;
    use RecorderState as S;

    fn run(events: &[E]) -> Result<S, AppError> {
        events
            .iter()
            .try_fold(S::Idle, |state, event| state.transition(event))
    }

    #[test]
    fn happy_path_reaches_completed() {
        let path = PathBuf::from("/tmp/x.mp4");
        let state = run(&[
            E::StartRequested,
            E::SourceSelected,
            E::PipelineStarted,
            E::PauseRequested,
            E::ResumeRequested,
            E::StopRequested,
            E::Finalized,
            E::MetadataWritten { path: path.clone() },
        ])
        .expect("valid sequence");
        assert_eq!(state, S::Completed { path });
    }

    #[test]
    fn cancelling_selection_returns_to_idle() {
        let state = run(&[E::StartRequested, E::SelectionCancelled]).expect("valid");
        assert_eq!(state, S::Idle);
    }

    #[test]
    fn can_stop_while_paused() {
        let state = run(&[
            E::StartRequested,
            E::SourceSelected,
            E::PipelineStarted,
            E::PauseRequested,
            E::StopRequested,
        ])
        .expect("valid");
        assert_eq!(state, S::Stopping);
    }

    #[test]
    fn invalid_transitions_are_rejected() {
        assert!(S::Idle.transition(&E::PauseRequested).is_err());
        assert!(S::Idle.transition(&E::StopRequested).is_err());
        assert!(S::Recording.transition(&E::StartRequested).is_err());
        assert!(S::Stopping.transition(&E::PauseRequested).is_err());
    }

    #[test]
    fn error_fails_from_busy_states_only() {
        let err = E::Error {
            reason: "boom".into(),
        };
        assert_eq!(
            S::Recording.transition(&err).expect("busy"),
            S::Failed {
                reason: "boom".into()
            }
        );
        assert!(S::Idle.transition(&err).is_err());
    }

    #[test]
    fn cancel_from_recording_goes_idle() {
        assert_eq!(S::Recording.transition(&E::Cancel).expect("busy"), S::Idle);
        assert!(S::Idle.transition(&E::Cancel).is_err());
    }

    #[test]
    fn can_start_again_after_completion_or_failure() {
        let done = S::Completed {
            path: PathBuf::from("a.mp4"),
        };
        assert_eq!(
            done.transition(&E::StartRequested).expect("ok"),
            S::SelectingSource
        );
        let failed = S::Failed { reason: "x".into() };
        assert_eq!(failed.transition(&E::Reset).expect("ok"), S::Idle);
    }

    #[test]
    fn busy_and_capturing_flags() {
        assert!(!S::Idle.is_busy());
        assert!(S::Preparing.is_busy());
        assert!(S::Paused.is_capturing());
        assert!(!S::Stopping.is_capturing());
    }
}
