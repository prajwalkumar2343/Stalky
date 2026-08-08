use serde::Serialize;

use crate::{AccessibilityAction, AccessibilityActionRequest, AccessibilityElementId};

pub const MAX_ACTION_VALUE_CHARS: usize = 256;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActionBinding {
    pub element: AccessibilityElementId,
    pub supported_actions: Vec<AccessibilityAction>,
    pub value_settable: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionPolicyError {
    StaleElement,
    UnsupportedAction,
    ValueRequired,
    ValueNotSettable,
    ValueTooLong,
    UnexpectedValue,
}

pub fn validate_action(
    request: &AccessibilityActionRequest,
    binding: &ActionBinding,
) -> Result<(), ActionPolicyError> {
    if request.element != binding.element {
        return Err(ActionPolicyError::StaleElement);
    }
    if request.action == AccessibilityAction::SetValue {
        if !binding.value_settable {
            return Err(ActionPolicyError::ValueNotSettable);
        }
        let value = request
            .value
            .as_deref()
            .ok_or(ActionPolicyError::ValueRequired)?;
        if value.chars().count() > MAX_ACTION_VALUE_CHARS {
            return Err(ActionPolicyError::ValueTooLong);
        }
        return Ok(());
    }
    if request.value.is_some() {
        return Err(ActionPolicyError::UnexpectedValue);
    }
    if binding.supported_actions.contains(&request.action) {
        Ok(())
    } else {
        Err(ActionPolicyError::UnsupportedAction)
    }
}

/// Returns whether a focused process may become the observed external target.
/// Stalky's own process is intentionally ignored so using the Accessibility UI
/// does not replace the external target or expose Stalky's own AX tree.
pub fn should_rebind_focused_application(
    focused_pid: i32,
    own_pid: i32,
    current_external_pid: Option<i32>,
) -> bool {
    focused_pid != own_pid && current_external_pid != Some(focused_pid)
}

#[cfg(test)]
mod tests {
    use super::{ActionBinding, ActionPolicyError, MAX_ACTION_VALUE_CHARS, validate_action};
    use crate::{AccessibilityAction, AccessibilityActionRequest, AccessibilityElementId};

    fn request(action: AccessibilityAction, value: Option<String>) -> AccessibilityActionRequest {
        AccessibilityActionRequest {
            element: AccessibilityElementId {
                id: "e1".into(),
                generation: 4,
            },
            action,
            value,
        }
    }

    fn binding() -> ActionBinding {
        ActionBinding {
            element: AccessibilityElementId {
                id: "e1".into(),
                generation: 4,
            },
            supported_actions: vec![AccessibilityAction::Press],
            value_settable: true,
        }
    }

    #[test]
    fn allows_only_advertised_actions() {
        assert!(validate_action(&request(AccessibilityAction::Press, None), &binding()).is_ok());
        assert_eq!(
            validate_action(&request(AccessibilityAction::Raise, None), &binding()),
            Err(ActionPolicyError::UnsupportedAction)
        );
    }

    #[test]
    fn rejects_stale_ids_and_unbounded_values() {
        let mut stale = request(AccessibilityAction::Press, None);
        stale.element.generation = 3;
        assert_eq!(
            validate_action(&stale, &binding()),
            Err(ActionPolicyError::StaleElement)
        );
        let too_long = "x".repeat(MAX_ACTION_VALUE_CHARS + 1);
        assert_eq!(
            validate_action(
                &request(AccessibilityAction::SetValue, Some(too_long)),
                &binding()
            ),
            Err(ActionPolicyError::ValueTooLong)
        );
    }

    #[test]
    fn ignores_own_process_focus() {
        assert!(!super::should_rebind_focused_application(42, 42, Some(7)));
        assert!(super::should_rebind_focused_application(9, 42, Some(7)));
        assert!(!super::should_rebind_focused_application(9, 42, Some(9)));
    }
}
