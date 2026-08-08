use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Lifecycle state for the application or an individual supervised subsystem.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub enum LifecycleState {
    #[default]
    Stopped,
    Starting,
    Running,
    Degraded {
        reason: String,
    },
    Stopping,
    Failed {
        retryable: bool,
        reason: String,
    },
}

impl LifecycleState {
    pub fn can_transition_to(&self, next: &Self) -> bool {
        use LifecycleState::{Degraded, Failed, Running, Starting, Stopped, Stopping};

        matches!(
            (self, next),
            (Stopped, Starting)
                | (
                    Starting,
                    Running | Degraded { .. } | Failed { .. } | Stopping
                )
                | (Running, Degraded { .. } | Failed { .. } | Stopping)
                | (Degraded { .. }, Running | Failed { .. } | Stopping)
                | (Failed { .. }, Starting | Stopping | Stopped)
                | (Stopping, Stopped | Failed { .. })
        )
    }

    pub fn is_active(&self) -> bool {
        matches!(self, Self::Starting | Self::Running | Self::Degraded { .. })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LifecycleTransition {
    pub from: LifecycleState,
    pub to: LifecycleState,
}

#[derive(Debug, Error, Eq, PartialEq)]
#[error("invalid lifecycle transition from {from:?} to {to:?}")]
pub struct LifecycleTransitionError {
    pub from: LifecycleState,
    pub to: LifecycleState,
}

impl LifecycleTransition {
    pub fn apply(
        current: &mut LifecycleState,
        next: LifecycleState,
    ) -> Result<Self, LifecycleTransitionError> {
        if !current.can_transition_to(&next) {
            return Err(LifecycleTransitionError {
                from: current.clone(),
                to: next,
            });
        }

        let transition = Self {
            from: current.clone(),
            to: next.clone(),
        };
        *current = next;
        Ok(transition)
    }
}

#[cfg(test)]
mod tests {
    use super::{LifecycleState, LifecycleTransition};

    #[test]
    fn normal_start_and_stop_are_valid() {
        let mut state = LifecycleState::Stopped;

        LifecycleTransition::apply(&mut state, LifecycleState::Starting).unwrap();
        LifecycleTransition::apply(&mut state, LifecycleState::Running).unwrap();
        LifecycleTransition::apply(&mut state, LifecycleState::Stopping).unwrap();
        LifecycleTransition::apply(&mut state, LifecycleState::Stopped).unwrap();

        assert_eq!(state, LifecycleState::Stopped);
    }

    #[test]
    fn invalid_transition_does_not_mutate_state() {
        let mut state = LifecycleState::Stopped;
        let result = LifecycleTransition::apply(&mut state, LifecycleState::Running);

        assert!(result.is_err());
        assert_eq!(state, LifecycleState::Stopped);
    }

    #[test]
    fn degraded_state_can_recover_or_stop() {
        let mut state = LifecycleState::Starting;
        LifecycleTransition::apply(
            &mut state,
            LifecycleState::Degraded {
                reason: "permission pending".to_owned(),
            },
        )
        .unwrap();
        LifecycleTransition::apply(&mut state, LifecycleState::Running).unwrap();
        assert!(state.is_active());
    }
}
