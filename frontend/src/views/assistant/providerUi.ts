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

export function verificationFailureMessage(code: string | null): string {
  if (code === null) return "Verification failed for an unknown reason.";
  return VERIFICATION_FAILURES[code] ?? `Verification failed (${code}).`;
}

export function verificationStatusLabel(status: ProviderVerificationStatus): string {
  if (status === "verified") return "Verified";
  if (status === "failed") return "Needs attention";
  return "Not verified";
}

export function roleConnection(
  connections: ProviderConnection[],
  connectionId: string,
): ProviderConnection | undefined {
  return connections.find((connection) => connection.id === connectionId);
}
