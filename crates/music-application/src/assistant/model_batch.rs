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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelBatchRecord {
    pub id: String,
    pub connection_id: String,
    pub state: String,
    pub input_file_id: Option<String>,
    pub remote_batch_id: Option<String>,
    pub document: Value,
}

impl ModelBatchRecord {
    #[must_use]
    pub fn pending(&self) -> bool {
        !matches!(
            self.state.as_str(),
            "completed" | "cancelled" | "failed" | "expired"
        )
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
    pub state: String,
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
