import { describe, it, expect } from "vitest";
import {
  PLATFORM_PRESETS,
  buildOnboardingPlatformPatch,
  defaultPlatformModel,
  expandModelsToTiers,
  compressModelsFromTiers,
  globalClaudeAuthPatch,
  platformHasConfiguredAuth,
} from "../platform-presets";

describe("Bedrock platform preset", () => {
  it("uses the AWS credential chain defaults mirrored by the backend", () => {
    const preset = PLATFORM_PRESETS.find((candidate) => candidate.id === "bedrock");

    expect(preset).toMatchObject({
      base_url: "",
      models: [
        "us.anthropic.claude-opus-4-8",
        "us.anthropic.claude-sonnet-4-6",
        "us.anthropic.claude-haiku-4-5-20251001-v1:0",
      ],
      extra_env: {
        CLAUDE_CODE_USE_BEDROCK: "1",
        AWS_REGION: "us-east-1",
      },
    });
  });
});

describe("Runcrate platform preset", () => {
  it("mirrors the backend defaults and pins a Runcrate model for every tier", () => {
    const preset = PLATFORM_PRESETS.find((candidate) => candidate.id === "runcrate");

    expect(preset).toMatchObject({
      base_url: "https://api.runcrate.ai",
      auth_env_var: "ANTHROPIC_AUTH_TOKEN",
      category: "proxy",
      models: ["deepseek-ai/DeepSeek-V4.1-Flash"],
      extra_env: { CLAUDE_CODE_MAX_CONTEXT_TOKENS: "1048576" },
    });
    expect(expandModelsToTiers(preset?.models)).toEqual([
      "deepseek-ai/DeepSeek-V4.1-Flash",
      "deepseek-ai/DeepSeek-V4.1-Flash",
      "deepseek-ai/DeepSeek-V4.1-Flash",
    ]);
  });
});

describe("provider runtime helpers", () => {
  it("uses the Sonnet tier as the default for three-model providers", () => {
    expect(defaultPlatformModel(["opus", "sonnet", "haiku"])).toBe("sonnet");
    expect(defaultPlatformModel(["main", "haiku"])).toBe("main");
  });

  it("treats Bedrock as configured without a stored API key", () => {
    expect(platformHasConfiguredAuth([], "bedrock")).toBe(true);
    expect(platformHasConfiguredAuth([], "deepseek")).toBe(false);
  });

  it("does not copy Bedrock credentials into global Anthropic fields", () => {
    expect(globalClaudeAuthPatch("bedrock", "aws-key", "https://stale.example")).toEqual({
      anthropic_api_key: null,
      anthropic_base_url: null,
    });
    expect(globalClaudeAuthPatch("anthropic", "sk-ant", "")).toEqual({
      anthropic_api_key: "sk-ant",
      anthropic_base_url: null,
    });
  });

  it("persists a third-party provider identity and replaces only its credential", () => {
    const preset = PLATFORM_PRESETS.find((candidate) => candidate.id === "deepseek")!;
    const patch = buildOnboardingPlatformPatch(
      [
        { platform_id: "kimi", api_key: "kimi-key" },
        {
          platform_id: "deepseek",
          models: ["private-deployment"],
          extra_env: { API_TIMEOUT_MS: "1234", PRIVATE_FLAG: "1" },
        },
      ],
      preset,
      " deepseek-key ",
      preset.base_url,
    );

    expect(patch.active_platform_id).toBe("deepseek");
    expect(patch.platform_credentials).toEqual([
      { platform_id: "kimi", api_key: "kimi-key" },
      {
        platform_id: "deepseek",
        api_key: "deepseek-key",
        base_url: "https://api.deepseek.com/anthropic",
        auth_env_var: "ANTHROPIC_AUTH_TOKEN",
        models: ["private-deployment"],
        extra_env: { API_TIMEOUT_MS: "1234", PRIVATE_FLAG: "1" },
      },
    ]);
  });

  it("persists keyless local providers so runtime defaults can supply placeholder auth", () => {
    const preset = PLATFORM_PRESETS.find((candidate) => candidate.id === "ollama")!;
    const patch = buildOnboardingPlatformPatch([], preset, "", preset.base_url);

    expect(patch.active_platform_id).toBe("ollama");
    expect(patch.platform_credentials).toEqual([
      expect.objectContaining({
        platform_id: "ollama",
        api_key: undefined,
        base_url: "http://localhost:11434",
      }),
    ]);
  });
});

describe("expandModelsToTiers", () => {
  it("undefined → all empty", () => {
    expect(expandModelsToTiers(undefined)).toEqual(["", "", ""]);
  });

  it("empty array → all empty", () => {
    expect(expandModelsToTiers([])).toEqual(["", "", ""]);
  });

  it("1 model → all same", () => {
    expect(expandModelsToTiers(["m"])).toEqual(["m", "m", "m"]);
  });

  it("2 models → [0]=opus+sonnet, [1]=haiku", () => {
    expect(expandModelsToTiers(["main", "eco"])).toEqual(["main", "main", "eco"]);
  });

  it("3 models → positional", () => {
    expect(expandModelsToTiers(["o", "s", "h"])).toEqual(["o", "s", "h"]);
  });

  it("3 models with empty strings preserved", () => {
    expect(expandModelsToTiers(["", "s", ""])).toEqual(["", "s", ""]);
  });
});

describe("compressModelsFromTiers", () => {
  it("all empty → undefined", () => {
    expect(compressModelsFromTiers("", "", "")).toBeUndefined();
  });

  it("all whitespace → undefined", () => {
    expect(compressModelsFromTiers("  ", " ", "  ")).toBeUndefined();
  });

  it("only sonnet → ['', 's', '']", () => {
    expect(compressModelsFromTiers("", "s", "")).toEqual(["", "s", ""]);
  });

  it("sonnet + haiku → ['', 's', 'h']", () => {
    expect(compressModelsFromTiers("", "s", "h")).toEqual(["", "s", "h"]);
  });

  it("all three → ['o', 's', 'h']", () => {
    expect(compressModelsFromTiers("o", "s", "h")).toEqual(["o", "s", "h"]);
  });

  it("trims whitespace", () => {
    expect(compressModelsFromTiers(" o ", " s ", " h ")).toEqual(["o", "s", "h"]);
  });

  it("opus + sonnet, empty haiku → ['o', 's', '']", () => {
    expect(compressModelsFromTiers("o", "s", "")).toEqual(["o", "s", ""]);
  });

  it("only opus → ['o', '', '']", () => {
    expect(compressModelsFromTiers("o", "", "")).toEqual(["o", "", ""]);
  });
});
