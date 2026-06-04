/**
 * Tests for the Codex model catalog cache + live-refresh fallback (B2).
 *
 * The live `model/list` path (loadCodexModelsLive) is authoritative for a running session, but
 * an empty result (old CLI without model/list, or a not-yet-ready session) must NOT blank the
 * pickers — it has to keep whatever the pre-session catalog (loadCodexModels) already populated.
 * This is the load-bearing fallback in cli-info.svelte.ts; we exercise it through the public
 * getters since normalizeCodexModels itself is module-private.
 */
import { describe, it, expect, vi, beforeEach } from "vitest";
import type { CodexModel } from "$lib/api";

// Mock the Tauri API surface used by cli-info. Only the two model entry points matter here.
vi.mock("$lib/api", () => ({
  getCodexModels: vi.fn(),
  listCodexModels: vi.fn(),
  // The remaining cli-info dependencies are unused by these tests but must exist as importable
  // names so the module loads.
  getCliInfo: vi.fn(),
  checkAgentCli: vi.fn(),
  getCliDistTags: vi.fn(),
  getCliConfig: vi.fn(),
}));

// debug utils touch localStorage which doesn't exist in node.
vi.mock("$lib/utils/debug", () => ({
  dbg: vi.fn(),
  dbgWarn: vi.fn(),
}));

import * as api from "$lib/api";
import {
  loadCodexModels,
  loadCodexModelsLive,
  getCodexModels,
  getCodexDefaultModel,
} from "./cli-info.svelte";

/** Build a CodexModel with sane defaults; override only what the case cares about. */
function model(overrides: Partial<CodexModel>): CodexModel {
  return {
    id: "id-x",
    model: "",
    displayName: "",
    description: "",
    hidden: false,
    supportedReasoningEfforts: [],
    defaultReasoningEffort: "medium",
    supportsPersonality: false,
    isDefault: false,
    ...overrides,
  };
}

describe("Codex model catalog", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("normalizes the pre-session catalog: filters hidden, prefers model over id, captures default", async () => {
    // cli-info caches across tests (module state); force=true bypasses the dedupe/loaded gate.
    vi.mocked(api.getCodexModels).mockResolvedValue({
      models: [
        { value: "gpt-5", displayName: "GPT-5", description: "", supportsEffort: false },
        {
          value: "gpt-5-codex",
          displayName: "GPT-5 Codex",
          description: "",
          supportsEffort: true,
          supportedEffortLevels: ["low", "high"],
        },
      ],
      defaultModel: "gpt-5",
    });

    await loadCodexModels(true);
    const models = getCodexModels();
    expect(models.map((m) => m.value)).toEqual(["gpt-5", "gpt-5-codex"]);
    expect(getCodexDefaultModel()).toBe("gpt-5");
  });

  it("live model/list upgrades the cached catalog", async () => {
    // Seed a pre-session cache first.
    vi.mocked(api.getCodexModels).mockResolvedValue({
      models: [{ value: "old", displayName: "Old", description: "", supportsEffort: false }],
      defaultModel: "old",
    });
    await loadCodexModels(true);

    // A live session returns the authoritative catalog → it replaces the cache.
    vi.mocked(api.listCodexModels).mockResolvedValue({
      data: [
        model({ id: "raw-id", model: "gpt-5.5", displayName: "GPT-5.5", isDefault: true }),
        model({ id: "hidden-one", model: "secret", hidden: true }),
        model({
          id: "fallback-id",
          model: "",
          supportedReasoningEfforts: [
            { reasoningEffort: "low", description: "" },
            { reasoningEffort: "high", description: "" },
          ],
        }),
      ],
    });

    await loadCodexModelsLive("run-1");
    const models = getCodexModels();
    // hidden filtered out; `model` preferred over `id`; empty model falls back to id.
    expect(models.map((m) => m.value)).toEqual(["gpt-5.5", "fallback-id"]);
    expect(getCodexDefaultModel()).toBe("gpt-5.5");
    // supportedReasoningEfforts flattened into supportedEffortLevels.
    const withEffort = models.find((m) => m.value === "fallback-id");
    expect(withEffort?.supportsEffort).toBe(true);
    expect(withEffort?.supportedEffortLevels).toEqual(["low", "high"]);
  });

  it("keeps the existing cache when live model/list returns empty (old CLI / session not ready)", async () => {
    // Seed a known-good cache.
    vi.mocked(api.getCodexModels).mockResolvedValue({
      models: [{ value: "gpt-5", displayName: "GPT-5", description: "", supportsEffort: false }],
      defaultModel: "gpt-5",
    });
    await loadCodexModels(true);

    // Live returns nothing → fallback MUST keep the cache, not blank the pickers.
    vi.mocked(api.listCodexModels).mockResolvedValue({ data: [] });
    await loadCodexModelsLive("run-2");

    expect(getCodexModels().map((m) => m.value)).toEqual(["gpt-5"]);
    expect(getCodexDefaultModel()).toBe("gpt-5");
  });

  it("swallows a live model/list failure and keeps the cache (fire-and-forget)", async () => {
    vi.mocked(api.getCodexModels).mockResolvedValue({
      models: [{ value: "gpt-5", displayName: "GPT-5", description: "", supportsEffort: false }],
      defaultModel: "gpt-5",
    });
    await loadCodexModels(true);

    vi.mocked(api.listCodexModels).mockRejectedValue(new Error("session gone"));
    // Must not throw.
    await expect(loadCodexModelsLive("run-3")).resolves.toBeUndefined();
    expect(getCodexModels().map((m) => m.value)).toEqual(["gpt-5"]);
  });
});
