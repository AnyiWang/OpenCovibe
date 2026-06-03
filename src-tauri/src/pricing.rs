/// Model pricing (per million tokens, USD).
pub struct ModelPricing {
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    pub cache_write: f64,
}

/// Get pricing for a model. Falls back to Sonnet pricing for unknown models.
pub fn get_pricing(model: &str) -> ModelPricing {
    try_get_pricing(model).unwrap_or_else(|| claude_pricing(3.0, 15.0))
}

/// Try to get pricing for a known model. Returns `None` for unknown models
/// (instead of silently falling back to a default).
pub fn try_get_pricing(model: &str) -> Option<ModelPricing> {
    // ── Claude models ──
    // Legacy Opus 4.0 / 4.1 → $15 / $75. Match these explicitly so newer Opus
    // (4.5, 4.6, 4.7, 4.8, and future releases) defaults to current $5/$25 pricing
    // below — otherwise each new Opus version silently inherits legacy pricing (#149).
    if model.contains("opus-4-0")
        || model.contains("opus-4-1")
        || model.contains("opus-4.0")
        || model.contains("opus-4.1")
    {
        return Some(claude_pricing(15.0, 75.0));
    }
    // Opus 4.5+ (current) → $5 / $25
    if model.contains("opus") {
        return Some(claude_pricing(5.0, 25.0));
    }
    if model.contains("haiku") {
        return Some(claude_pricing(0.80, 4.0));
    }
    if model.contains("sonnet") {
        return Some(claude_pricing(3.0, 15.0));
    }
    // OpenAI models
    if model.contains("gpt-4o") {
        return Some(claude_pricing(2.5, 10.0));
    }
    if model.contains("gpt-4") {
        return Some(claude_pricing(10.0, 30.0));
    }
    if model.contains("o1") || model.contains("o3") {
        return Some(claude_pricing(15.0, 60.0));
    }

    // ── OpenAI GPT-5.x / Codex models ──
    // Prices from developers.openai.com/docs/pricing.
    // Long prefix first to avoid false matches.
    //
    // gpt-5-codex-mini / gpt-5.1-codex-mini ($0.25/$2.00)
    if model.contains("gpt-5-codex-mini") || model.contains("gpt-5.1-codex-mini") {
        return Some(ModelPricing {
            input: 0.25,
            output: 2.0,
            cache_read: 0.025,
            cache_write: 0.25,
        });
    }
    // gpt-5-codex / gpt-5.1-codex / gpt-5.1-codex-max ($1.25/$10.00)
    if model.contains("gpt-5-codex") || model.contains("gpt-5.1-codex") {
        return Some(ModelPricing {
            input: 1.25,
            output: 10.0,
            cache_read: 0.125,
            cache_write: 1.25,
        });
    }
    // gpt-5.2-codex ($1.75/$14.00)
    if model.contains("gpt-5.2-codex") {
        return Some(ModelPricing {
            input: 1.75,
            output: 14.0,
            cache_read: 0.175,
            cache_write: 1.75,
        });
    }
    // gpt-5.3-codex / gpt-5.3-chat-latest ($1.75/$14.00)
    if model.contains("gpt-5.3-codex") || model.contains("gpt-5.3-chat") {
        return Some(ModelPricing {
            input: 1.75,
            output: 14.0,
            cache_read: 0.175,
            cache_write: 1.75,
        });
    }
    // gpt-5.4-pro ($30.00/$180.00)
    if model.contains("gpt-5.4-pro") {
        return Some(ModelPricing {
            input: 30.0,
            output: 180.0,
            cache_read: 3.0,
            cache_write: 30.0,
        });
    }
    // gpt-5.4-nano ($0.20/$1.25)
    if model.contains("gpt-5.4-nano") {
        return Some(ModelPricing {
            input: 0.20,
            output: 1.25,
            cache_read: 0.020,
            cache_write: 0.20,
        });
    }
    // gpt-5.4-mini ($0.75/$4.50)
    if model.contains("gpt-5.4-mini") {
        return Some(ModelPricing {
            input: 0.75,
            output: 4.50,
            cache_read: 0.075,
            cache_write: 0.75,
        });
    }
    // gpt-5.4 ($2.50/$15.00)
    if model.contains("gpt-5.4") {
        return Some(ModelPricing {
            input: 2.50,
            output: 15.0,
            cache_read: 0.25,
            cache_write: 2.50,
        });
    }
    // gpt-5.2 ($1.75/$14.00)
    if model.contains("gpt-5.2") {
        return Some(ModelPricing {
            input: 1.75,
            output: 14.0,
            cache_read: 0.175,
            cache_write: 1.75,
        });
    }
    // gpt-5.1 ($1.25/$10.00)
    if model.contains("gpt-5.1") {
        return Some(ModelPricing {
            input: 1.25,
            output: 10.0,
            cache_read: 0.125,
            cache_write: 1.25,
        });
    }
    // gpt-5 ($1.25/$10.00)
    if model.contains("gpt-5") {
        return Some(ModelPricing {
            input: 1.25,
            output: 10.0,
            cache_read: 0.125,
            cache_write: 1.25,
        });
    }

    // ── Third-party provider models ──
    // DeepSeek: deepseek-chat, deepseek-reasoner (V3.2 unified pricing)
    if model.contains("deepseek") {
        return Some(ModelPricing {
            input: 0.28,
            output: 0.42,
            cache_read: 0.028,
            cache_write: 0.28,
        });
    }
    // Kimi / Moonshot
    if model.contains("kimi-k2.5") || model.contains("kimi-k25") {
        return Some(ModelPricing {
            input: 0.60,
            output: 3.0,
            cache_read: 0.10,
            cache_write: 0.60,
        });
    }
    if model.contains("kimi") {
        return Some(ModelPricing {
            input: 0.60,
            output: 2.50,
            cache_read: 0.15,
            cache_write: 0.60,
        });
    }
    // Zhipu GLM
    if model.contains("glm-4.5-flash") || model.contains("glm-4-5-flash") {
        return Some(ModelPricing {
            input: 0.0,
            output: 0.0,
            cache_read: 0.0,
            cache_write: 0.0,
        });
    }
    if model.contains("glm-4.5-air") || model.contains("glm-4-5-air") {
        return Some(ModelPricing {
            input: 0.20,
            output: 1.10,
            cache_read: 0.03,
            cache_write: 0.20,
        });
    }
    if model.contains("glm-4.7") || model.contains("glm-4-7") || model.contains("glm") {
        return Some(ModelPricing {
            input: 0.60,
            output: 2.20,
            cache_read: 0.11,
            cache_write: 0.60,
        });
    }
    // Qwen / Bailian (lowest tier pricing)
    if model.contains("qwen3-max") {
        return Some(ModelPricing {
            input: 1.20,
            output: 6.0,
            cache_read: 0.12,
            cache_write: 1.20,
        });
    }
    if model.contains("qwen3.5-plus") || model.contains("qwen35-plus") {
        return Some(ModelPricing {
            input: 0.40,
            output: 2.40,
            cache_read: 0.04,
            cache_write: 0.40,
        });
    }
    if model.contains("qwen-plus") {
        return Some(ModelPricing {
            input: 0.40,
            output: 1.20,
            cache_read: 0.04,
            cache_write: 0.40,
        });
    }
    if model.contains("qwen-flash") || model.contains("qwen") {
        return Some(ModelPricing {
            input: 0.05,
            output: 0.40,
            cache_read: 0.005,
            cache_write: 0.05,
        });
    }
    // DouBao / Volcengine (lowest tier, CNY→USD @ ~7.2)
    if model.contains("doubao") {
        return Some(ModelPricing {
            input: 0.17,
            output: 1.11,
            cache_read: 0.034,
            cache_write: 0.17,
        });
    }
    // MiniMax
    if model.contains("MiniMax-M2.5-highspeed") || model.contains("minimax-m2.5-highspeed") {
        return Some(ModelPricing {
            input: 0.30,
            output: 2.40,
            cache_read: 0.03,
            cache_write: 0.30,
        });
    }
    if model.contains("MiniMax") || model.contains("minimax") {
        return Some(ModelPricing {
            input: 0.30,
            output: 1.20,
            cache_read: 0.03,
            cache_write: 0.30,
        });
    }
    // MiMo / Xiaomi
    if model.contains("mimo") {
        return Some(ModelPricing {
            input: 0.10,
            output: 0.30,
            cache_read: 0.01,
            cache_write: 0.10,
        });
    }

    None
}

/// Standard Claude pricing: cache_read = 10% of input, cache_write = 125% of input.
fn claude_pricing(input: f64, output: f64) -> ModelPricing {
    ModelPricing {
        input,
        output,
        cache_read: input * 0.1,
        cache_write: input * 1.25,
    }
}

/// True for known third-party (non-Anthropic/OpenAI) provider models. Used to
/// gate provider-specific UI/cost handling. (merged from master)
pub fn is_third_party(model: &str) -> bool {
    model.contains("deepseek")
        || model.contains("kimi")
        || model.contains("glm")
        || model.contains("qwen")
        || model.contains("doubao")
        || model.contains("minimax")
        || model.contains("MiniMax")
        || model.contains("mimo")
}

/// Estimate cost from token counts (input, output, cache read, cache write).
/// Falls back to Sonnet pricing for unknown models.
pub fn estimate_cost(
    model: &str,
    input_tokens: u64,
    output_tokens: u64,
    cache_read_tokens: u64,
    cache_write_tokens: u64,
) -> f64 {
    let p = get_pricing(model);
    compute_cost(
        &p,
        input_tokens,
        output_tokens,
        cache_read_tokens,
        cache_write_tokens,
    )
}

/// Like `estimate_cost` but returns `None` for unknown models instead of
/// falling back to Sonnet pricing. Use this when a wrong estimate is worse
/// than no estimate (e.g. Codex runs with non-standard models like gpt-oss-*).
pub fn try_estimate_cost(
    model: &str,
    input_tokens: u64,
    output_tokens: u64,
    cache_read_tokens: u64,
    cache_write_tokens: u64,
) -> Option<f64> {
    let p = try_get_pricing(model)?;
    Some(compute_cost(
        &p,
        input_tokens,
        output_tokens,
        cache_read_tokens,
        cache_write_tokens,
    ))
}

fn compute_cost(
    p: &ModelPricing,
    input_tokens: u64,
    output_tokens: u64,
    cache_read_tokens: u64,
    cache_write_tokens: u64,
) -> f64 {
    (input_tokens as f64 * p.input
        + output_tokens as f64 * p.output
        + cache_read_tokens as f64 * p.cache_read
        + cache_write_tokens as f64 * p.cache_write)
        / 1_000_000.0
}
