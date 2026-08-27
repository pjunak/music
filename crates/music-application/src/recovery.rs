use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::future::Future;
use std::pin::Pin;

use serde_json::Value;
use uuid::Uuid;

const JOURNAL_ID_LENGTH: usize = 32;
const MAX_OPERATION_LENGTH: usize = 64;
const MAX_JSON_BYTES: usize = 64 * 1024;
const MAX_JSON_DEPTH: usize = 64;
const MAX_JSON_NODES: usize = 16_384;

pub type RecoveryDependencyError = Box<dyn Error + Send + Sync>;
pub type RecoveryFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, RecoveryDependencyError>> + Send + 'a>>;

#[derive(Debug, Clone, Eq, Ord, PartialEq, PartialOrd)]
pub struct RecoveryJournalId(String);

impl RecoveryJournalId {
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::new_v4().simple().to_string())
    }

    pub fn parse(value: impl Into<String>) -> Result<Self, RecoveryValidationError> {
        let value = value.into();
        if value.len() != JOURNAL_ID_LENGTH
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(RecoveryValidationError::InvalidId);
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for RecoveryJournalId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum RecoveryDomain {
    Library,
    Sfx,
    Modes,
    Authoring,
    Cleanup,
}

impl RecoveryDomain {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Library => "library",
            Self::Sfx => "sfx",
            Self::Modes => "modes",
            Self::Authoring => "authoring",
            Self::Cleanup => "cleanup",
        }
    }

    pub fn parse(value: &str) -> Result<Self, RecoveryValidationError> {
        match value {
            "library" => Ok(Self::Library),
            "sfx" => Ok(Self::Sfx),
            "modes" => Ok(Self::Modes),
            "authoring" => Ok(Self::Authoring),
            "cleanup" => Ok(Self::Cleanup),
            _ => Err(RecoveryValidationError::InvalidDomain),
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct RecoveryOperation(String);

impl RecoveryOperation {
    pub fn parse(value: impl Into<String>) -> Result<Self, RecoveryValidationError> {
        let value = value.into();
        if value.is_empty()
            || value.len() > MAX_OPERATION_LENGTH
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
        {
            return Err(RecoveryValidationError::InvalidOperation);
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum RecoveryState {
    Planned,
    Applying,
    Committed,
    RollingBack,
    RolledBack,
    Failed,
}

impl RecoveryState {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Planned => "planned",
            Self::Applying => "applying",
            Self::Committed => "committed",
            Self::RollingBack => "rolling_back",
            Self::RolledBack => "rolled_back",
            Self::Failed => "failed",
        }
    }

    pub fn parse(value: &str) -> Result<Self, RecoveryValidationError> {
        match value {
            "planned" => Ok(Self::Planned),
            "applying" => Ok(Self::Applying),
            "committed" => Ok(Self::Committed),
            "rolling_back" => Ok(Self::RollingBack),
            "rolled_back" => Ok(Self::RolledBack),
            "failed" => Ok(Self::Failed),
            _ => Err(RecoveryValidationError::InvalidState),
        }
    }

    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Committed | Self::RolledBack | Self::Failed)
    }

    #[must_use]
    pub const fn allows(self, next: Self) -> bool {
        matches!(
            (self, next),
            (
                Self::Planned,
                Self::Applying | Self::RollingBack | Self::Failed
            ) | (
                Self::Applying,
                Self::Committed | Self::RollingBack | Self::Failed
            ) | (Self::RollingBack, Self::RolledBack | Self::Failed)
                | (Self::Failed, Self::RollingBack)
        )
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct RecoveryJournalDraft {
    pub id: RecoveryJournalId,
    pub domain: RecoveryDomain,
    pub operation: RecoveryOperation,
    pub plan: Value,
    pub progress: Value,
}

impl RecoveryJournalDraft {
    pub fn new(
        domain: RecoveryDomain,
        operation: RecoveryOperation,
        plan: Value,
    ) -> Result<Self, RecoveryValidationError> {
        validate_json(&plan)?;
        Ok(Self {
            id: RecoveryJournalId::new(),
            domain,
            operation,
            plan,
            progress: Value::Object(serde_json::Map::new()),
        })
    }

    pub fn with_progress(mut self, progress: Value) -> Result<Self, RecoveryValidationError> {
        validate_json(&progress)?;
        self.progress = progress;
        Ok(self)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct RecoveryJournalEntry {
    pub id: RecoveryJournalId,
    pub domain: RecoveryDomain,
    pub operation: RecoveryOperation,
    pub state: RecoveryState,
    pub plan: Value,
    pub progress: Value,
    pub created_at_unix_seconds: i64,
    pub updated_at_unix_seconds: i64,
    pub completed_at_unix_seconds: Option<i64>,
}

impl RecoveryJournalEntry {
    pub fn validate(&self) -> Result<(), RecoveryValidationError> {
        validate_json(&self.plan)?;
        validate_json(&self.progress)?;
        if self.state.is_terminal() != self.completed_at_unix_seconds.is_some() {
            return Err(RecoveryValidationError::InvalidCompletion);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum RecoveryTransition {
    Applied(RecoveryJournalEntry),
    Conflict(Option<RecoveryJournalEntry>),
}

pub trait RecoveryJournalRepository: std::fmt::Debug + Send + Sync {
    fn create_recovery_journal(
        &self,
        draft: RecoveryJournalDraft,
    ) -> RecoveryFuture<'_, RecoveryJournalEntry>;

    fn unfinished_recovery_journals(
        &self,
        domain: RecoveryDomain,
    ) -> RecoveryFuture<'_, Vec<RecoveryJournalEntry>>;

    fn transition_recovery_journal<'a>(
        &'a self,
        id: &'a RecoveryJournalId,
        expected: RecoveryState,
        next: RecoveryState,
        progress: Value,
    ) -> RecoveryFuture<'a, RecoveryTransition>;
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum RecoveryValidationError {
    InvalidId,
    InvalidDomain,
    InvalidOperation,
    InvalidState,
    InvalidTransition,
    InvalidCompletion,
    JsonTooLarge,
    JsonTooDeep,
    JsonTooComplex,
}

impl Display for RecoveryValidationError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidId => "recovery journal id is invalid",
            Self::InvalidDomain => "recovery journal domain is invalid",
            Self::InvalidOperation => "recovery journal operation is invalid",
            Self::InvalidState => "recovery journal state is invalid",
            Self::InvalidTransition => "recovery journal state transition is invalid",
            Self::InvalidCompletion => "recovery journal completion timestamp is inconsistent",
            Self::JsonTooLarge => "recovery journal JSON exceeds the size limit",
            Self::JsonTooDeep => "recovery journal JSON exceeds the depth limit",
            Self::JsonTooComplex => "recovery journal JSON exceeds the node limit",
        })
    }
}

impl Error for RecoveryValidationError {}

pub fn validate_recovery_progress(progress: &Value) -> Result<(), RecoveryValidationError> {
    validate_json(progress)
}

fn validate_json(value: &Value) -> Result<(), RecoveryValidationError> {
    let encoded = serde_json::to_vec(value).map_err(|_| RecoveryValidationError::JsonTooLarge)?;
    if encoded.len() > MAX_JSON_BYTES {
        return Err(RecoveryValidationError::JsonTooLarge);
    }
    let mut nodes = 0_usize;
    let mut work = vec![(value, 0_usize)];
    while let Some((current, depth)) = work.pop() {
        nodes = nodes
            .checked_add(1)
            .ok_or(RecoveryValidationError::JsonTooComplex)?;
        if nodes > MAX_JSON_NODES {
            return Err(RecoveryValidationError::JsonTooComplex);
        }
        if depth > MAX_JSON_DEPTH {
            return Err(RecoveryValidationError::JsonTooDeep);
        }
        match current {
            Value::Array(values) => {
                work.extend(values.iter().map(|value| (value, depth + 1)));
            }
            Value::Object(values) => {
                work.extend(values.values().map(|value| (value, depth + 1)));
            }
            Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        RecoveryDomain, RecoveryJournalDraft, RecoveryJournalId, RecoveryOperation, RecoveryState,
    };

    #[test]
    fn identifiers_plans_and_state_transitions_are_bounded()
    -> Result<(), Box<dyn std::error::Error>> {
        let id = RecoveryJournalId::new();
        assert_eq!(id.as_str().len(), 32);
        assert!(RecoveryJournalId::parse(id.as_str().to_owned()).is_ok());
        assert!(RecoveryJournalId::parse("ABC").is_err());
        assert!(RecoveryOperation::parse("publish_upload").is_ok());
        assert!(RecoveryOperation::parse("Publish Upload").is_err());
        assert!(
            RecoveryJournalDraft::new(
                RecoveryDomain::Library,
                RecoveryOperation::parse("publish_upload")?,
                json!({"staged": "Uploads/.track.partial", "target": "Uploads/track.wav"}),
            )
            .is_ok()
        );
        assert!(RecoveryState::Planned.allows(RecoveryState::Applying));
        assert!(RecoveryState::Applying.allows(RecoveryState::Committed));
        assert!(!RecoveryState::Committed.allows(RecoveryState::Applying));
        Ok(())
    }
}
