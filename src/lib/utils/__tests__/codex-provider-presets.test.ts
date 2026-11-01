import { describe, expect, it } from "vitest";
import {
  bedrockRegionFromBaseUrl,
  bedrockRuntimeBaseUrl,
  CODEX_PROVIDER_PRESETS,
} from "../codex-provider-presets";

describe("Codex provider presets", () => {
  it("keeps provider IDs unique", () => {
    const ids = CODEX_PROVIDER_PRESETS.map((preset) => preset.id);
    expect(new Set(ids).size).toBe(ids.length);
  });

  it("configures OpenRouter's documented Codex endpoint", () => {
    const preset = CODEX_PROVIDER_PRESETS.find((candidate) => candidate.id === "openrouter");

    expect(preset).toMatchObject({
      base_url: "https://openrouter.ai/api/v1",
      env_key: "OPENROUTER_API_KEY",
      model: "~openai/gpt-latest",
    });
  });

  it("configures Runcrate's documented Codex endpoint", () => {
    const preset = CODEX_PROVIDER_PRESETS.find((candidate) => candidate.id === "runcrate");

    expect(preset).toMatchObject({
      base_url: "https://api.runcrate.ai/v1",
      env_key: "RUNCRATE_API_KEY",
      model: "zai-org/GLM-5.3",
    });
  });

  it("leaves Portkey's user-scoped model slug unset", () => {
    const preset = CODEX_PROVIDER_PRESETS.find((candidate) => candidate.id === "portkey");

    expect(preset).toMatchObject({
      base_url: "https://api.portkey.ai/v1",
      env_key: "PORTKEY_API_KEY",
      model: "",
    });
  });

  it("generates and parses the Bedrock Responses endpoint from its region", () => {
    const baseUrl = bedrockRuntimeBaseUrl("eu-west-1");

    expect(baseUrl).toBe("https://bedrock-runtime.eu-west-1.amazonaws.com/openai/v1");
    expect(bedrockRegionFromBaseUrl(baseUrl)).toBe("eu-west-1");
    expect(bedrockRegionFromBaseUrl(`${baseUrl}/`)).toBe("eu-west-1");
    expect(bedrockRegionFromBaseUrl("https://example.com/openai/v1")).toBeNull();
  });

  it("configures Bedrock with bearer-token auth and no account-specific model default", () => {
    const preset = CODEX_PROVIDER_PRESETS.find((candidate) => candidate.id === "bedrock");

    expect(preset).toMatchObject({
      base_url: "https://bedrock-runtime.us-east-1.amazonaws.com/openai/v1",
      env_key: "AWS_BEARER_TOKEN_BEDROCK",
      model: "",
    });
  });
});
