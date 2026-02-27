import type { PlatformPreset } from "$lib/types";

export const PLATFORM_PRESETS: PlatformPreset[] = [
  // ── LLM Providers ──
  {
    id: "anthropic",
    name: "Anthropic",
    base_url: "",
    auth_env_var: "ANTHROPIC_API_KEY",
    description: "Claude official API",
    key_placeholder: "your-anthropic-api-key",
    category: "provider",
  },
  {
    id: "deepseek",
    name: "DeepSeek",
    base_url: "https://api.deepseek.com/anthropic",
    auth_env_var: "ANTHROPIC_API_KEY",
    description: "DeepSeek API",
    key_placeholder: "your-deepseek-key",
    category: "provider",
    models: ["deepseek-chat"],
    extra_env: { API_TIMEOUT_MS: "600000" },
    docs_url: "https://api-docs.deepseek.com/guides/anthropic_api",
  },
  {
    id: "kimi",
    name: "Kimi (Moonshot)",
    base_url: "https://api.moonshot.cn/anthropic",
    auth_env_var: "ANTHROPIC_AUTH_TOKEN",
    description: "Moonshot AI",
    key_placeholder: "your-kimi-key",
    category: "provider",
    models: ["kimi-k2.5", "kimi-k2"],
    docs_url: "https://platform.moonshot.ai/docs/guide/agent-support",
  },
  {
    id: "kimi-coding",
    name: "Kimi For Coding",
    base_url: "https://api.kimi.com/coding/",
    auth_env_var: "ANTHROPIC_API_KEY",
    description: "Kimi Code membership",
    key_placeholder: "your-kimi-coding-key",
    category: "provider",
  },
  {
    id: "zhipu",
    name: "Zhipu (\u667a\u8c31)",
    base_url: "https://open.bigmodel.cn/api/anthropic",
    auth_env_var: "ANTHROPIC_API_KEY",
    description: "Zhipu AI",
    key_placeholder: "your-zhipu-key",
    category: "provider",
    models: ["glm-4.7", "glm-4.5-air", "glm-4.5-flash"],
    docs_url: "https://docs.bigmodel.cn/cn/guide/develop/claude/introduction",
  },
  {
    id: "bailian",
    name: "Bailian (\u767e\u70bc)",
    base_url: "https://dashscope.aliyuncs.com/apps/anthropic",
    auth_env_var: "ANTHROPIC_AUTH_TOKEN",
    description: "Alibaba DashScope",
    key_placeholder: "your-bailian-key",
    category: "provider",
    models: ["qwen3-max", "qwen3.5-plus", "qwen-plus", "qwen-flash"],
    docs_url: "https://help.aliyun.com/zh/model-studio/anthropic-api-messages",
  },
  {
    id: "doubao",
    name: "DouBao (\u8c46\u5305)",
    base_url: "https://ark.cn-beijing.volces.com/api/coding",
    auth_env_var: "ANTHROPIC_API_KEY",
    description: "ByteDance Volcengine",
    key_placeholder: "your-doubao-key",
    category: "provider",
    models: ["doubao-seed-code-preview-latest"],
    docs_url: "https://www.volcengine.com/docs/82379/1949118",
  },
  {
    id: "minimax",
    name: "MiniMax (International)",
    base_url: "https://api.minimax.io/anthropic",
    auth_env_var: "ANTHROPIC_API_KEY",
    description: "MiniMax AI — api.minimax.io",
    key_placeholder: "your-minimax-key",
    category: "provider",
    models: ["MiniMax-M2.5", "MiniMax-M2.5-highspeed"],
    docs_url: "https://platform.minimax.io/docs/api-reference/text-anthropic-api",
  },
  {
    id: "minimax-cn",
    name: "MiniMax (China)",
    base_url: "https://api.minimaxi.com/anthropic",
    auth_env_var: "ANTHROPIC_API_KEY",
    description: "MiniMax AI — api.minimaxi.com",
    key_placeholder: "your-minimax-key",
    category: "provider",
    models: ["MiniMax-M2.5", "MiniMax-M2.5-highspeed"],
    docs_url: "https://platform.minimax.io/docs/api-reference/text-anthropic-api",
  },
  {
    id: "mimo",
    name: "Xiaomi MiMo (\u5c0f\u7c73)",
    base_url: "https://api.xiaomimimo.com/anthropic",
    auth_env_var: "ANTHROPIC_API_KEY",
    description: "Xiaomi AI",
    key_placeholder: "your-mimo-key",
    category: "provider",
    models: ["mimo-v2-flash"],
  },

  // ── API Proxy ──
  {
    id: "vercel",
    name: "Vercel AI Gateway",
    base_url: "https://ai-gateway.vercel.sh",
    auth_env_var: "ANTHROPIC_AUTH_TOKEN",
    description: "Vercel unified gateway",
    key_placeholder: "your-ai-gateway-api-key",
    category: "proxy",
    docs_url: "https://vercel.com/docs/ai-gateway/anthropic-compat",
  },
  {
    id: "openrouter",
    name: "OpenRouter",
    base_url: "https://openrouter.ai/api",
    auth_env_var: "ANTHROPIC_AUTH_TOKEN",
    description: "Multi-provider gateway",
    key_placeholder: "your-openrouter-key",
    category: "proxy",
    docs_url: "https://openrouter.ai/docs/guides/guides/claude-code-integration",
  },
  {
    id: "aihubmix",
    name: "AiHubMix",
    base_url: "https://aihubmix.com",
    auth_env_var: "ANTHROPIC_API_KEY",
    description: "AI aggregation platform",
    key_placeholder: "your-aihubmix-key",
    category: "proxy",
  },

  // ── Local Inference ──
  {
    id: "ollama",
    name: "Ollama",
    base_url: "http://localhost:11434",
    auth_env_var: "ANTHROPIC_AUTH_TOKEN",
    description: "Local LLM (no key needed)",
    key_placeholder: "(leave empty for local)",
    category: "local",
  },

  // ── Custom ──
  {
    id: "custom",
    name: "Custom",
    base_url: "",
    auth_env_var: "ANTHROPIC_AUTH_TOKEN",
    description: "Custom API endpoint",
    key_placeholder: "your-api-key",
    category: "custom",
  },
];

export const PRESET_CATEGORIES = [
  { id: "provider", label: "LLM Providers" },
  { id: "proxy", label: "API Proxy" },
  { id: "local", label: "Local" },
  { id: "custom", label: "Custom" },
] as const;

/**
 * Build a merged platform list from static presets + dynamic custom credentials.
 * Excludes the single "custom" placeholder — custom entries use "custom-{timestamp}" ids.
 */
export function buildPlatformList(
  credentials: import("$lib/types").PlatformCredential[],
): PlatformPreset[] {
  const builtins = PLATFORM_PRESETS.filter((p) => p.id !== "custom");
  const customs: PlatformPreset[] = credentials
    .filter((c) => c.platform_id.startsWith("custom-"))
    .map((c) => ({
      id: c.platform_id,
      name: c.name ?? "Custom",
      base_url: c.base_url ?? "",
      auth_env_var: (c.auth_env_var ?? "ANTHROPIC_AUTH_TOKEN") as PlatformPreset["auth_env_var"],
      description: "Custom endpoint",
      key_placeholder: "your-api-key",
      category: "custom" as const,
      models: c.models,
      extra_env: c.extra_env,
    }));
  return [...builtins, ...customs];
}

/** Check if a platform_id represents a user-created custom endpoint. */
export function isCustomPlatform(platformId: string): boolean {
  return platformId.startsWith("custom-");
}

/** Find a credential by platform_id. */
export function findCredential(
  credentials: import("$lib/types").PlatformCredential[],
  platformId: string,
): import("$lib/types").PlatformCredential | undefined {
  return credentials.find((c) => c.platform_id === platformId);
}
