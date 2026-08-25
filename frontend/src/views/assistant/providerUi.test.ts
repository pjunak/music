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

  it("explains provider-specific failures without exposing upstream details", () => {
    expect(modelTestFailureMessage("parameter_unknown")).toContain(
      "does not support",
    );
    expect(modelTestFailureMessage("model_refusal")).toContain("declined");
  });
});
