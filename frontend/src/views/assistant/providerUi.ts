import type {
  ProviderConnection,
  ProviderVerificationStatus,
} from "@/core/assistantProvidersApi";

const FIXED_PROVIDER_ADDRESSES: Record<string, string> = {
  "openai-responses/v1": "https://api.openai.com/v1",
  "google-gemini-openai/v1":
    "https://generativelanguage.googleapis.com/v1beta/openai",
  "google-gemini-openai-json-schema/v1":
    "https://generativelanguage.googleapis.com/v1beta/openai",
};

const VERIFICATION_FAILURES: Record<string, string> = {
  unauthorized: "The provider rejected this API key.",
  forbidden: "This API key cannot list the provider's models.",
  models_endpoint_not_found: "The provider does not expose a compatible model list.",
  rate_limited: "The provider asked us to slow down. Try verification again later.",
  quota_exceeded: "This provider account has no available request quota.",
  invalid_request: "The provider rejected these connection settings.",
  parameter_unknown: "The provider does not support one of these request settings.",
  failed_precondition: "The provider account or model is not ready for this request.",
  model_not_found: "The selected provider model is no longer available.",
  destination_blocked:
    "The address resolves to a private or otherwise unsafe destination.",
  redirect_blocked: "The provider redirected the verification request.",
  response_too_large: "The provider returned an unexpectedly large model list.",
  invalid_response: "The provider returned a model list we could not understand.",
  timeout: "The provider did not respond within ten seconds.",
  provider_timeout: "The provider stopped the request after its own deadline.",
  tls_error: "A secure connection to the provider could not be established.",
  network_error: "The provider could not be reached from the server.",
  upstream_error: "The provider returned an unexpected error.",
  service_unavailable: "The provider service is temporarily unavailable.",
  unsupported_provider_feature:
    "The provider does not support a feature required by this connection type.",
  unsupported_adapter: "This connection type is not supported by this server.",
};

const MODEL_TEST_FAILURES: Record<string, string> = {
  unauthorized: "The provider rejected this API key.",
  forbidden: "This API key cannot use the selected model.",
  completion_endpoint_not_found:
    "The provider does not expose the model endpoint required by this connection type.",
  rate_limited: "The provider asked us to slow down. Try the model test later.",
  quota_exceeded: "This provider account has no available request quota.",
  invalid_request:
    "The provider rejected this model or one of its request settings.",
  parameter_unknown:
    "The provider does not support one of the request settings used by this connection type.",
  failed_precondition:
    "The provider account or model is not ready for this request.",
  model_not_found: "The selected provider model is no longer available.",
  destination_blocked:
    "The address resolves to a private or otherwise unsafe destination.",
  redirect_blocked: "The provider redirected the model request.",
  request_too_large: "The model request exceeded the server safety limit.",
  response_too_large: "The model returned an unexpectedly large response.",
  invalid_response: "The provider returned a response we could not understand.",
  invalid_structured_output:
    "The model did not return the required machine-readable JSON object.",
  empty_structured_output:
    "The model returned an empty response instead of the required JSON object.",
  incomplete_structured_output:
    "The model ran out of response tokens before completing the JSON object.",
  model_refusal: "The model declined to produce the required structured result.",
  conformance_mismatch:
    "The model did not copy the one-time test values exactly.",
  timeout: "The model did not respond within this task's timeout.",
  provider_timeout: "The provider stopped the model request after its own deadline.",
  tls_error: "A secure connection to the provider could not be established.",
  network_error: "The provider could not be reached from the server.",
  upstream_error: "The provider returned an unexpected error.",
  service_unavailable: "The provider service is temporarily unavailable.",
  unsupported_provider_feature:
    "The provider does not support a feature required by this connection type.",
  unsupported_adapter: "This connection type cannot run model requests yet.",
};

export function verificationFailureMessage(code: string | null): string {
  if (code === null) return "Verification failed for an unknown reason.";
  return VERIFICATION_FAILURES[code] ?? `Verification failed (${code}).`;
}

export function verificationStatusLabel(status: ProviderVerificationStatus): string {
  if (status === "verified") return "Verified";
  if (status === "failed") return "Needs attention";
  return "Not verified";
}

export function modelTestFailureMessage(code: string | null): string {
  if (code === null) return "The model test failed for an unknown reason.";
  return MODEL_TEST_FAILURES[code] ?? `The model test failed (${code}).`;
}

export function defaultProviderAddress(adapterId: string): string {
  return FIXED_PROVIDER_ADDRESSES[adapterId] ?? "";
}

export function providerAddressAfterAdapterChange(
  currentAddress: string,
  previousAdapterId: string,
  nextAdapterId: string,
): string {
  const normalizedCurrent = currentAddress.replace(/\/$/, "");
  const previousFixedAddress = defaultProviderAddress(previousAdapterId);
  if (!normalizedCurrent || normalizedCurrent === previousFixedAddress) {
    return defaultProviderAddress(nextAdapterId);
  }
  return currentAddress;
}

export function roleConnection(
  connections: ProviderConnection[],
  connectionId: string,
): ProviderConnection | undefined {
  return connections.find((connection) => connection.id === connectionId);
}
