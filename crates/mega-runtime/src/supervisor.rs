use std::future::Future;

use mega_core::{LifecycleState, LifecycleTransition, LifecycleTransitionError, Subsystem};
use thiserror::Error;
use tokio_util::sync::CancellationToken;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SupervisorTransition {
    pub subsystem: Subsystem,
    pub transition: LifecycleTransition,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SupervisionReport {
    pub subsystem: Subsystem,
    pub final_state: LifecycleState,
    pub cancelled: bool,
    pub worker_error: Option<String>,
    pub transitions: Vec<SupervisorTransition>,
}

#[derive(Debug, Error, Eq, PartialEq)]
pub enum SupervisorError {
    #[error("invalid supervisor transition: {0}")]
    Transition(#[from] LifecycleTransitionError),
}

/// Runs one cooperative subsystem worker and owns every lifecycle transition.
///
/// The worker receives a child token. On shutdown, the supervisor changes to
/// `Stopping` before cancelling that child, then waits for the worker future to
/// finish before publishing `Stopped`. This makes cleanup ordering observable
/// and prevents a late worker result from reviving a stopped subsystem.
pub struct Supervisor {
    subsystem: Subsystem,
    state: LifecycleState,
    transitions: Vec<SupervisorTransition>,
}

impl Supervisor {
    pub fn new(subsystem: Subsystem) -> Self {
        Self {
            subsystem,
            state: LifecycleState::Stopped,
            transitions: Vec::new(),
        }
    }

    pub fn state(&self) -> &LifecycleState {
        &self.state
    }

    pub fn transition(&mut self, next: LifecycleState) -> Result<(), SupervisorError> {
        let transition = LifecycleTransition::apply(&mut self.state, next)?;
        self.transitions.push(SupervisorTransition {
            subsystem: self.subsystem,
            transition,
        });
        Ok(())
    }

    pub async fn run<F, Fut>(
        mut self,
        application_cancellation: CancellationToken,
        worker: F,
    ) -> Result<SupervisionReport, SupervisorError>
    where
        F: FnOnce(CancellationToken) -> Fut,
        Fut: Future<Output = Result<(), String>>,
    {
        self.transition(LifecycleState::Starting)?;
        let child = application_cancellation.child_token();
        self.transition(LifecycleState::Running)?;
        let mut worker_future = Box::pin(worker(child.clone()));

        tokio::select! {
            biased;
            _ = application_cancellation.cancelled() => {
                self.transition(LifecycleState::Stopping)?;
                child.cancel();
                let _ = worker_future.await;
                self.transition(LifecycleState::Stopped)?;
                Ok(self.report(true, None))
            }
            result = &mut worker_future => {
                match result {
                    Ok(()) => {
                        self.transition(LifecycleState::Stopping)?;
                        child.cancel();
                        self.transition(LifecycleState::Stopped)?;
                        Ok(self.report(false, None))
                    }
                    Err(error) => {
                        self.transition(LifecycleState::Failed {
                            retryable: true,
                            reason: error.clone(),
                        })?;
                        child.cancel();
                        Ok(self.report(false, Some(error)))
                    }
                }
            }
        }
    }

    fn report(self, cancelled: bool, worker_error: Option<String>) -> SupervisionReport {
        SupervisionReport {
            subsystem: self.subsystem,
            final_state: self.state,
            cancelled,
            worker_error,
            transitions: self.transitions,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Supervisor;
    use mega_core::{LifecycleState, Subsystem};
    use tokio_util::sync::CancellationToken;

    #[tokio::test]
    async fn cancellation_stops_worker_before_final_stopped_state() {
        let cancellation = CancellationToken::new();
        let worker_cancellation = cancellation.clone();
        let task = tokio::spawn(async move {
            Supervisor::new(Subsystem::Audio)
                .run(cancellation, |child| async move {
                    child.cancelled().await;
                    Ok(())
                })
                .await
                .unwrap()
        });

        worker_cancellation.cancel();
        let report = task.await.unwrap();

        assert!(report.cancelled);
        assert_eq!(report.final_state, LifecycleState::Stopped);
        assert_eq!(report.transitions.len(), 4);
        assert_eq!(
            report.transitions[2].transition.to,
            LifecycleState::Stopping
        );
    }

    #[tokio::test]
    async fn worker_failure_is_terminal_for_this_run() {
        let report = Supervisor::new(Subsystem::ScreenCapture)
            .run(CancellationToken::new(), |_child| async {
                Err("source unavailable".to_owned())
            })
            .await
            .unwrap();

        assert!(!report.cancelled);
        assert_eq!(
            report.final_state,
            LifecycleState::Failed {
                retryable: true,
                reason: "source unavailable".to_owned()
            }
        );
        assert_eq!(report.worker_error.as_deref(), Some("source unavailable"));
    }
}
