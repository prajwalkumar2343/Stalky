use std::collections::BTreeMap;

use serde_json::Value;
use sha2::{Digest, Sha256};
use thiserror::Error;
use uuid::Uuid;

use crate::{auth::Principal, protocol::*};

pub const MAX_MEMORY_TITLE: usize = 240;
pub const MAX_MEMORY_CONTENT: usize = 8_000;
pub const MAX_TODO_TITLE: usize = 500;
pub const MAX_RECORD_FIELDS: usize = 60;
pub const MAX_RECORD_FIELD_NAME: usize = 80;
pub const MAX_RECORD_VALUE_CHARS: usize = 4_000;
pub const MAX_IDEMPOTENCY_KEY: usize = 200;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ValidationError {
    #[error("{field} is required")]
    Required { field: &'static str },
    #[error("{field} is too long")]
    TooLong { field: &'static str },
    #[error("{field} must be between {min} and {max}")]
    OutOfRange {
        field: &'static str,
        min: usize,
        max: usize,
    },
    #[error("{field} contains unsupported data")]
    Unsupported { field: String },
    #[error("{field} must be a valid UUID")]
    InvalidUuid { field: &'static str },
}

pub fn principal_user_id(principal: &Principal) -> Result<Uuid, ValidationError> {
    Uuid::parse_str(&principal.user_id)
        .map_err(|_| ValidationError::InvalidUuid { field: "user_id" })
}

pub fn validate_memory_create(input: &MemoryCreate) -> Result<(), ValidationError> {
    validate_text(&input.title, "title", 1, MAX_MEMORY_TITLE)?;
    validate_text(&input.content, "content", 1, MAX_MEMORY_CONTENT)
}

pub fn validate_memory_search(input: &MemorySearchRequest) -> Result<(), ValidationError> {
    validate_text(&input.query, "query", 1, MAX_MEMORY_CONTENT)?;
    if !(1..=50).contains(&input.limit) {
        return Err(ValidationError::OutOfRange {
            field: "limit",
            min: 1,
            max: 50,
        });
    }
    Ok(())
}

pub fn validate_todo_create(input: &TodoCreate) -> Result<(), ValidationError> {
    validate_text(&input.title, "title", 1, MAX_TODO_TITLE)
}

pub fn validate_todo_update(input: &TodoUpdate) -> Result<(), ValidationError> {
    if input.title.is_none() && input.done.is_none() {
        return Err(ValidationError::Required { field: "update" });
    }
    if let Some(title) = input.title.as_deref() {
        validate_text(title, "title", 1, MAX_TODO_TITLE)?;
    }
    Ok(())
}

pub fn validate_record_create(
    input: &MiniAppRecordCreate,
) -> Result<BTreeMap<String, Value>, ValidationError> {
    let record_type = input.record_type.trim();
    if record_type.is_empty() {
        return Err(ValidationError::Required {
            field: "recordType",
        });
    }
    sanitize_record_values(&input.values)
}

pub fn validate_record_update(
    input: &MiniAppRecordUpdate,
) -> Result<BTreeMap<String, Value>, ValidationError> {
    sanitize_record_values(&input.values)
}

pub fn validate_idempotency_key(value: Option<&str>) -> Result<Option<String>, ValidationError> {
    let Some(value) = value else {
        return Ok(None);
    };
    let value = value.trim();
    if value.is_empty() || value.len() > MAX_IDEMPOTENCY_KEY {
        return Err(ValidationError::OutOfRange {
            field: "Idempotency-Key",
            min: 1,
            max: MAX_IDEMPOTENCY_KEY,
        });
    }
    Ok(Some(value.to_owned()))
}

pub fn validate_agent_run(input: &AgentRunRequest) -> Result<(), ValidationError> {
    if input.message.trim().is_empty() && input.image_base64.as_deref().unwrap_or("").is_empty() {
        return Err(ValidationError::Required {
            field: "message or image_base64",
        });
    }
    if input.message.chars().count() > 12_000 {
        return Err(ValidationError::TooLong { field: "message" });
    }
    if input
        .image_base64
        .as_deref()
        .is_some_and(|value| value.len() > 8_000_000)
    {
        return Err(ValidationError::TooLong {
            field: "image_base64",
        });
    }
    Ok(())
}

pub fn validate_profile_patch(input: &ProfilePatch) -> Result<(), ValidationError> {
    if input
        .display_name
        .as_deref()
        .is_some_and(|value| value.chars().count() > 120)
    {
        return Err(ValidationError::TooLong {
            field: "displayName",
        });
    }
    if input
        .avatar_url
        .as_deref()
        .is_some_and(|value| value.chars().count() > 2_048)
    {
        return Err(ValidationError::TooLong { field: "avatarUrl" });
    }
    Ok(())
}

pub fn sanitize_record_values(
    values: &BTreeMap<String, Value>,
) -> Result<BTreeMap<String, Value>, ValidationError> {
    if values.len() > MAX_RECORD_FIELDS {
        return Err(ValidationError::TooLong {
            field: "record values",
        });
    }
    let mut clean = BTreeMap::new();
    for (raw_name, value) in values {
        let name = raw_name.trim();
        if name.is_empty() || name.len() > MAX_RECORD_FIELD_NAME {
            return Err(ValidationError::Unsupported {
                field: raw_name.clone(),
            });
        }
        if !matches!(
            value,
            Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_)
        ) {
            return Err(ValidationError::Unsupported {
                field: name.to_owned(),
            });
        }
        let encoded = value.to_string();
        if encoded.chars().count() > MAX_RECORD_VALUE_CHARS {
            return Err(ValidationError::TooLong {
                field: "record value",
            });
        }
        clean.insert(name.to_owned(), value.clone());
    }
    Ok(clean)
}

pub fn request_fingerprint(input: &AgentRunRequest) -> String {
    let json = serde_json::to_vec(&input.without_secret()).expect("agent request is serializable");
    let mut hasher = Sha256::new();
    hasher.update(json);
    format!("{:x}", hasher.finalize())
}

fn validate_text(
    value: &str,
    field: &'static str,
    min: usize,
    max: usize,
) -> Result<(), ValidationError> {
    let length = value.trim().chars().count();
    if length < min {
        return Err(ValidationError::Required { field });
    }
    if length > max {
        return Err(ValidationError::TooLong { field });
    }
    Ok(())
}
