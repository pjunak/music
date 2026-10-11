use super::{
    AssistantFuture, ModelTaskError, ProviderExecutionTarget, StructuredModelRequest,
    StructuredModelResult,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug)]
pub struct ModelBatchServices {
    pub repository: std::sync::Arc<dyn ModelBatchRepository>,
    pub transport: std::sync::Arc<dyn ModelBatchTransport>,
    pub providers: std::sync::Arc<super::ProviderService>,
}

pub const MAX_MODEL_BATCH_REQUESTS: usize = 500;
pub const MAX_MODEL_BATCH_BYTES: usize = 32 * 1024 * 1024;

pub(super) fn serialize_track_id<S: serde::Serializer>(
    id: &music_domain::TrackId,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.serialize_i64(id.get())
}
pub(super) fn deserialize_track_id<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<music_domain::TrackId, D::Error> {
    music_domain::TrackId::new(i64::deserialize(deserializer)?).map_err(serde::de::Error::custom)
}

/// Where a provider batch is in its lifecycle. Local steps and terminal
/// states are explicit; the provider's own in-flight states (validating,
/// in_progress, finalizing, ...) are kept verbatim because the provider owns
/// that list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "String", into = "String")]
pub enum BatchState {
    Uploading,
    Uploaded,
    Submitting,
    Submitted,
    Cancelling,
    ResultsSaved,
    Completed,
    Failed,
    Expired,
    Cancelled,
    Provider(String),
}

impl BatchState {
    #[must_use]
    pub fn parse(value: &str) -> Self {
        match value {
            "uploading" => Self::Uploading,
            "uploaded" => Self::Uploaded,
            "submitting" => Self::Submitting,
            "submitted" => Self::Submitted,
            "cancelling" => Self::Cancelling,
            "results_saved" => Self::ResultsSaved,
            "completed" => Self::Completed,
            "failed" => Self::Failed,
            "expired" => Self::Expired,
            "cancelled" => Self::Cancelled,
            other => Self::Provider(other.to_owned()),
        }
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            Self::Uploading => "uploading",
            Self::Uploaded => "uploaded",
            Self::Submitting => "submitting",
            Self::Submitted => "submitted",
            Self::Cancelling => "cancelling",
            Self::ResultsSaved => "results_saved",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Expired => "expired",
            Self::Cancelled => "cancelled",
            Self::Provider(state) => state,
        }
    }

    /// The provider will not change a batch in this state again.
    #[must_use]
    pub const fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Failed | Self::Expired | Self::Cancelled
        )
    }
}

impl PartialEq<&str> for BatchState {
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == *other
    }
}

impl From<String> for BatchState {
    fn from(value: String) -> Self {
        Self::parse(&value)
    }
}

impl From<BatchState> for String {
    fn from(value: BatchState) -> Self {
        value.as_str().to_owned()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelBatchRecord {
    pub id: String,
    pub connection_id: String,
    pub state: BatchState,
    pub input_file_id: Option<String>,
    pub remote_batch_id: Option<String>,
    pub document: Value,
}

impl ModelBatchRecord {
    #[must_use]
    pub fn pending(&self) -> bool {
        !self.state.is_terminal()
    }
}

pub trait ModelBatchRepository: std::fmt::Debug + Send + Sync {
    fn model_batch(&self, id: &str) -> AssistantFuture<'_, Option<ModelBatchRecord>>;
    /// Polling view excludes frozen input and write templates.
    fn model_batch_summary(&self, id: &str) -> AssistantFuture<'_, Option<ModelBatchRecord>>;
    fn pending_model_batch(&self) -> AssistantFuture<'_, Option<ModelBatchRecord>>;
    fn create_model_batch<'a>(&'a self, record: &'a ModelBatchRecord) -> AssistantFuture<'a, bool>;
    fn update_model_batch<'a>(
        &'a self,
        expected: &'a str,
        record: &'a ModelBatchRecord,
    ) -> AssistantFuture<'a, bool>;
}

#[derive(Debug)]
pub struct ProviderBatchStatus {
    pub run_id: String,
    pub state: BatchState,
    pub input_file_id: String,
    pub output_file_id: Option<String>,
    pub error_file_id: Option<String>,
}

#[derive(Debug)]
pub struct ProviderBatchResult {
    pub custom_id: String,
    pub result: StructuredModelResult,
}

pub type BatchTransportFuture<'a, T> =
    std::pin::Pin<Box<dyn std::future::Future<Output = Result<T, ModelTaskError>> + Send + 'a>>;

/// Submission is deliberately split so file and batch identifiers can be
/// persisted between network calls. Implementations never retry submission.
pub trait ModelBatchTransport: std::fmt::Debug + Send + Sync {
    fn validate(
        &self,
        target: &ProviderExecutionTarget,
        requests: &[StructuredModelRequest],
    ) -> Result<(), ModelTaskError>;
    fn upload<'a>(
        &'a self,
        target: &'a ProviderExecutionTarget,
        requests: &'a [StructuredModelRequest],
    ) -> BatchTransportFuture<'a, String>;
    fn submit<'a>(
        &'a self,
        target: &'a ProviderExecutionTarget,
        file_id: &'a str,
        run_id: &'a str,
    ) -> BatchTransportFuture<'a, String>;
    fn status<'a>(
        &'a self,
        target: &'a ProviderExecutionTarget,
        batch_id: &'a str,
    ) -> BatchTransportFuture<'a, ProviderBatchStatus>;
    fn results<'a>(
        &'a self,
        target: &'a ProviderExecutionTarget,
        file_id: &'a str,
    ) -> BatchTransportFuture<'a, Vec<ProviderBatchResult>>;
    fn cancel<'a>(
        &'a self,
        target: &'a ProviderExecutionTarget,
        batch_id: &'a str,
    ) -> BatchTransportFuture<'a, ()>;
    fn delete_file<'a>(
        &'a self,
        target: &'a ProviderExecutionTarget,
        file_id: &'a str,
    ) -> BatchTransportFuture<'a, ()>;
}
