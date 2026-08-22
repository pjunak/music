import type {
  ProviderConnection,
  ProviderVerificationStatus,
} from "@/core/assistantProvidersApi";

const VERIFICATION_FAILURES: Record<string, string> = {
  unauthorized: "The provider rejected this API key.",
  forbidden: "This API key cannot list the provider's models.",
  models_endpoint_not_found: "The provider does not expose a compatible model list.",
  rate_limited: "The provider asked us to slow down. Try verification again later.",
  destination_blocked:
    "The address resolves to a private or otherwise unsafe destination.",
  redirect_blocked: "The provider redirected the verification request.",
  response_too_large: "The provider returned an unexpectedly large model list.",
  invalid_response: "The provider returned a model list we could not understand.",
  timeout: "The provider did not respond within ten seconds.",
  tls_error: "A secure connection to the provider could not be established.",
  network_error: "The provider could not be reached from the server.",
  upstream_error: "The provider returned an unexpected error.",
  unsupported_adapter: "This connection type is not supported by this server.",
};

const MODEL_TEST_FAILURES: Record<string, string> = {
  unauthorized: "The provider rejected this API key.",
  forbidden: "This API key cannot use the selected model.",
  completion_endpoint_not_found:
    "The provider does not expose a compatible chat-completions endpoint.",
  rate_limited: "The provider asked us to slow down. Try the model test later.",
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
  conformance_mismatch:
    "The model did not copy the one-time test values exactly.",
  timeout: "The model did not respond within this task's timeout.",
  tls_error: "A secure connection to the provider could not be established.",
  network_error: "The provider could not be reached from the server.",
  upstream_error: "The provider returned an unexpected error.",
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

export function roleConnection(
  connections: ProviderConnection[],
  connectionId: string,
): ProviderConnection | undefined {
  return connections.find((connection) => connection.id === connectionId);
}
