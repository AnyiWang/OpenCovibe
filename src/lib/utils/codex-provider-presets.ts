// Codex third-party provider presets (OpenAI Responses API).
//
// Codex (≥0.99) ONLY speaks `wire_api = "responses"` — `chat` was removed. So only providers
// that natively implement the OpenAI Responses API (`/v1/responses`) work directly. The Chinese
// first-party providers in the Claude grid are Chat-Completions-only and need a translation proxy,
// so they are intentionally NOT listed here. Verified-working set: Responses-capable gateways +
// Ollama (local) + Custom. See memory codex-integration-audit-2026-06 / the provider research.
//
// Default `model` strings verified against each gateway's own Codex docs (2026-08): Vercel
// documents GPT-5.6 Sol; OpenRouter documents its `~openai/gpt-latest` alias; Requesty documents
// GPT-5.5; AiHubMix documents GPT-5.2; ZenMux's safe default remains GPT-5.2 Codex because its
// GPT-5.6 route requires extra multi-agent config. Portkey has no default because its model IDs
// include a user-created provider slug. Pin to what each gateway actually publishes as a working
// default, not the absolute newest model — a model the gateway hasn't onboarded yet would fail.
//
// At spawn we inject these as `codex exec -c model_providers.<id>.{base_url,env_key,wire_api,
// requires_openai_auth=false}` plus the API key via the env var named by `env_key`.

export interface CodexProviderPreset {
  id: string;
  name: string;
  description: string;
  /** OpenAI-compatible base URL implementing the Responses API. */
  base_url: string;
  /** Env var name the API key is injected under (we set it at spawn). */
  env_key: string;
  /** Suggested model id (provider-side, OpenAI format). Empty → user fills / Codex default. */
  model: string;
  key_placeholder: string;
  /** Keyless (local) providers like Ollama need no API key. */
  keyless?: boolean;
  /** Custom = user supplies base_url/env_key/model. */
  custom?: boolean;
  docs_url?: string;
}

const BEDROCK_RUNTIME_URL =
  /^https:\/\/bedrock-runtime\.([a-z0-9-]+)\.amazonaws\.com\/openai\/v1\/?$/;

export function bedrockRuntimeBaseUrl(region: string): string {
  return `https://bedrock-runtime.${region.trim()}.amazonaws.com/openai/v1`;
}

export function bedrockRegionFromBaseUrl(baseUrl: string): string | null {
  return BEDROCK_RUNTIME_URL.exec(baseUrl.trim())?.[1] ?? null;
}

export const CODEX_PROVIDER_PRESETS: CodexProviderPreset[] = [
  {
    id: "bedrock",
    name: "Amazon Bedrock",
    description: "AWS Responses API",
    base_url: bedrockRuntimeBaseUrl("us-east-1"),
    env_key: "AWS_BEARER_TOKEN_BEDROCK",
    // Bedrock model and inference-profile availability is account and region specific.
    model: "",
    key_placeholder: "your-bedrock-api-key",
    docs_url: "https://docs.aws.amazon.com/bedrock/latest/userguide/bedrock-mantle.html",
  },
  {
    id: "vercel",
    name: "Vercel AI Gateway",
    description: "Responses API gateway",
    base_url: "https://ai-gateway.vercel.sh/v1",
    env_key: "AI_GATEWAY_API_KEY",
    model: "openai/gpt-5.6-sol",
    key_placeholder: "vck_…",
    docs_url: "https://vercel.com/docs/ai-gateway/codex",
  },
  {
    id: "openrouter",
    name: "OpenRouter",
    description: "Multi-provider router (Responses)",
    base_url: "https://openrouter.ai/api/v1",
    env_key: "OPENROUTER_API_KEY",
    model: "~openai/gpt-latest",
    key_placeholder: "sk-or-…",
    docs_url: "https://openrouter.ai/docs/cookbook/coding-agents/codex-cli",
  },
  {
    id: "portkey",
    name: "Portkey",
    description: "AI gateway (Responses)",
    base_url: "https://api.portkey.ai/v1",
    env_key: "PORTKEY_API_KEY",
    // Portkey model IDs include a user-created provider slug, so there is no universal default.
    model: "",
    key_placeholder: "your-portkey-api-key",
    docs_url: "https://docs.portkey.ai/docs/integrations/libraries/codex",
  },
  {
    id: "aihubmix",
    name: "AiHubMix",
    description: "AI aggregation (Responses)",
    base_url: "https://aihubmix.com/v1",
    env_key: "AIHUBMIX_API_KEY",
    model: "gpt-5.2",
    key_placeholder: "sk-…",
    docs_url: "https://docs.aihubmix.com/en/api/Codex-CLI",
  },
  {
    id: "requesty",
    name: "Requesty",
    description: "Multi-provider router (Responses)",
    base_url: "https://router.requesty.ai/v1",
    env_key: "REQUESTY_API_KEY",
    // Requesty requires the openai-responses/ model prefix for Codex.
    model: "openai-responses/gpt-5.5",
    key_placeholder: "sk-…",
    docs_url: "https://docs.requesty.ai/integrations/openai-codex",
  },
  {
    id: "fireworks",
    name: "Fireworks AI",
    description: "Responses API (beta)",
    base_url: "https://api.fireworks.ai/inference/v1",
    env_key: "FIREWORKS_API_KEY",
    model: "",
    key_placeholder: "fw_…",
    docs_url: "https://docs.fireworks.ai/tools-sdks/openai-compatibility",
  },
  {
    id: "zenmux",
    name: "ZenMux",
    description: "Multi-model gateway (Responses)",
    base_url: "https://zenmux.ai/api/v1",
    env_key: "ZENMUX_API_KEY",
    model: "openai/gpt-5.2-codex",
    key_placeholder: "sk-…",
    docs_url: "https://docs.zenmux.ai/guide/quickstart.html",
  },
  {
    id: "runcrate",
    name: "Runcrate",
    description: "Open-weight models (Responses)",
    base_url: "https://api.runcrate.ai/v1",
    env_key: "RUNCRATE_API_KEY",
    model: "zai-org/GLM-5.3",
    key_placeholder: "rc_live_…",
    docs_url: "https://runcrate.ai/docs/models/responses",
  },
  {
    id: "ollama",
    name: "Ollama",
    description: "Local models (no key)",
    base_url: "http://localhost:11434/v1",
    env_key: "",
    model: "",
    key_placeholder: "",
    keyless: true,
    docs_url: "https://docs.ollama.com/integrations/codex",
  },
  {
    id: "custom",
    name: "Custom",
    description: "Custom Responses endpoint",
    base_url: "",
    env_key: "OPENAI_API_KEY",
    model: "",
    key_placeholder: "sk-…",
    custom: true,
  },
];
