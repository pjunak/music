import { describe, expect, it } from "vitest";

import {
  defaultProviderAddress,
  modelTestFailureMessage,
  providerAddressAfterAdapterChange,
} from "./providerUi";

describe("provider UI helpers", () => {
  it("supplies the pinned addresses for native OpenAI and Gemini adapters", () => {
    expect(defaultProviderAddress("openai-responses/v1")).toBe(
      "https://api.openai.com/v1",
    );
    expect(defaultProviderAddress("google-gemini-openai/v1")).toBe(
      "https://generativelanguage.googleapis.com/v1beta/openai",
    );
    expect(defaultProviderAddress("openai-compatible/v1")).toBe("");
    expect(defaultProviderAddress("typesafe-systemone/v1")).toBe("https://api.typesafe.ai/v1");
    expect(modelTestFailureMessage("pinned_model_required")).toContain("pinned Jev version");
    expect(defaultProviderAddress("deepseek-chat/v1")).toBe("https://api.deepseek.com");
    expect(defaultProviderAddress("deepseek-responses/v1")).toBe("https://api.deepseek.com");
  });

  it("replaces only an empty or prior fixed address when the adapter changes", () => {
    expect(
      providerAddressAfterAdapterChange(
        "https://api.openai.com/v1/",
        "openai-responses/v1",
        "google-gemini-openai/v1",
      ),
    ).toBe("https://generativelanguage.googleapis.com/v1beta/openai");
    expect(
      providerAddressAfterAdapterChange(
        "https://gateway.example/v1",
        "openai-compatible/v1",
        "openai-responses/v1",
      ),
    ).toBe("https://gateway.example/v1");
  });

  it.each([
    ["typed_conformance_positive_failed", "positive Noul", "at least 0.90"],
    ["typed_conformance_negative_failed", "negative Noul", "at most 0.10"],
    ["typed_conformance_choice_failed", "Choice", "at least 0.90"],
  ])("explains the specific Jev conformance failure %s", (code, check, threshold) => {
    expect(modelTestFailureMessage(code)).toContain(check);
    expect(modelTestFailureMessage(code)).toContain(threshold);
  });

  it.each([
    ["typed_response_shape_invalid", "response envelope"],
    ["typed_answer_shape_invalid", "malformed typed answer"],
    ["typed_answer_set_mismatch", "missing or unexpected answers"],
    ["typed_answer_type_mismatch", "wrong answer type"],
    ["typed_probability_invalid", "probability outside"],
    ["typed_choice_options_mismatch", "missing or unexpected Choice options"],
    ["typed_choice_distribution_invalid", "do not sum to one"],
    ["typed_choice_selection_invalid", "not the most probable"],
  ])("explains the rejected Jev response %s", (code, reason) => {
    expect(modelTestFailureMessage(code)).toContain(reason);
    expect(modelTestFailureMessage(code)).toContain("not retried");
  });

  it("explains provider-specific failures without exposing upstream details", () => {
    expect(modelTestFailureMessage("parameter_unknown")).toContain(
      "does not support",
    );
    expect(modelTestFailureMessage("model_refusal")).toContain("declined");
  });
});
