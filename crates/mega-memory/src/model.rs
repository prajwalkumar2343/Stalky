use serde::{Deserialize, Serialize};

macro_rules! string_id {
    ($name:ident) => {
        #[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
        #[serde(transparent)]
        pub struct $name(pub String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Self {
                Self(value.into())
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                Self(value.to_owned())
            }
        }
    };
}

string_id!(MemoryId);
string_id!(SourceEventId);
string_id!(AppId);
string_id!(ScopeId);
string_id!(EntityId);
string_id!(ExtractionRunId);

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryType {
    Preference,
    Fact,
    Decision,
    Episode,
    Task,
    Procedure,
}

impl MemoryType {
    pub const fn freshness_sensitive(self) -> bool {
        matches!(self, Self::Episode | Self::Task)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AssertionMode {
    Explicit,
    Observed,
    Inferred,
    Imported,
    Manual,
}

impl AssertionMode {
    pub const fn confidence_ceiling(self) -> f32 {
        match self {
            Self::Explicit | Self::Manual => 1.0,
            Self::Observed => 0.9,
            Self::Inferred => 0.75,
            Self::Imported => 0.95,
        }
    }

    pub const fn trust_rank(self) -> u8 {
        match self {
            Self::Manual => 5,
            Self::Explicit => 4,
            Self::Imported => 3,
            Self::Observed => 2,
            Self::Inferred => 1,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryStatus {
    Active,
    Superseded,
    Expired,
    Forgotten,
    PendingReview,
    Rejected,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Sensitivity {
    Public,
    Private,
    Sensitive,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ScopeType {
    Global,
    App,
    Project,
    Entity,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct MemoryScope {
    pub id: ScopeId,
    pub scope_type: ScopeType,
    pub scope_key: String,
    pub display_name: String,
}

impl MemoryScope {
    pub fn label(&self) -> String {
        match self.scope_type {
            ScopeType::Global => "global".to_owned(),
            ScopeType::App => format!("app:{}", self.scope_key),
            ScopeType::Project => format!("project:{}", self.scope_key),
            ScopeType::Entity => format!("entity:{}", self.scope_key),
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EntityType {
    Person,
    Organization,
    Project,
    Place,
    Product,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EntityRole {
    Subject,
    Object,
    Participant,
    Mentioned,
    Scope,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct EntityReference {
    pub entity_id: EntityId,
    pub canonical_name: String,
    pub role: EntityRole,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryRelationType {
    Updates,
    Extends,
    Supports,
    Contradicts,
    DerivedFrom,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AppRole {
    Source,
    AppliesTo,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SupportKind {
    Primary,
    Supporting,
    Contradicting,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Memory {
    pub id: MemoryId,
    pub normalized_content: String,
    pub display_content: String,
    pub memory_type: MemoryType,
    pub assertion_mode: AssertionMode,
    pub status: MemoryStatus,
    pub scope: MemoryScope,
    pub source_app_ids: Vec<AppId>,
    pub applicable_app_ids: Vec<AppId>,
    pub category_slugs: Vec<String>,
    pub entities: Vec<EntityReference>,
    pub source_event_ids: Vec<SourceEventId>,
    pub importance: f32,
    pub confidence: f32,
    pub sensitivity: Sensitivity,
    pub valid_from_ms: Option<i64>,
    pub valid_until_ms: Option<i64>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    pub revision: u32,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HistoricalMode {
    ActiveOnly,
    IncludeHistory,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SensitivityAllowance {
    PublicOnly,
    IncludePrivate,
    IncludeSensitive,
}

impl SensitivityAllowance {
    pub const fn allows(self, sensitivity: Sensitivity) -> bool {
        matches!(sensitivity, Sensitivity::Public)
            || matches!(self, Self::IncludePrivate | Self::IncludeSensitive)
                && matches!(sensitivity, Sensitivity::Private)
            || matches!(self, Self::IncludeSensitive)
    }
}
