use std::future::Future;
use std::pin::Pin;

use super::{
    ModelTaskError, ProviderExecutionTarget, StructuredModelRequest, StructuredModelResult,
};

pub type ModelTransportFuture<'a> =
    Pin<Box<dyn Future<Output = StructuredModelResult> + Send + 'a>>;

/// Outbound port for application-owned model workflows. Implementations enforce
/// the same serialized request limits during preflight and actual execution.
pub trait StructuredModelTransport: std::fmt::Debug + Send + Sync {
    fn validate_request(
        &self,
        target: &ProviderExecutionTarget,
        request: &StructuredModelRequest,
    ) -> Result<(), ModelTaskError>;

    fn execute_structured_model_request<'a>(
        &'a self,
        target: &'a ProviderExecutionTarget,
        request: &'a StructuredModelRequest,
    ) -> ModelTransportFuture<'a>;
}
