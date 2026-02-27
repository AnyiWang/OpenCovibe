/// Model pricing (per million tokens, USD).
pub struct ModelPricing {
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    pub cache_write: f64,
}

/// Get pricing for a model. Matches known Claude models, third-party provider models,
/// and falls back to Sonnet pricing for unknown models.
pub fn get_pricing(model: &str) -> ModelPricing {
    // ── Claude models ──
    // Opus 4.5 / 4.6 → $5 / $25
    if model.contains("opus-4-6")
        || model.contains("opus-4-5")
        || model.contains("opus-4.5")
        || model.contains("opus-4.6")
    {
        return claude_pricing(5.0, 25.0);
    }
    // Opus 4.0 / 4.1 (legacy)
    if model.contains("opus") {
        return claude_pricing(15.0, 75.0);
    }
    if model.contains("haiku") {
        return claude_pricing(0.80, 4.0);
    }
    if model.contains("sonnet") {
        return claude_pricing(3.0, 15.0);
    }
    // OpenAI models
    if model.contains("gpt-4o") {
        return claude_pricing(2.5, 10.0);
    }
    if model.contains("gpt-4") {
        return claude_pricing(10.0, 30.0);
    }
    if model.contains("o1") || model.contains("o3") {
        return claude_pricing(15.0, 60.0);
    }

    // ── Third-party provider models ──
    // DeepSeek: deepseek-chat, deepseek-reasoner (V3.2 unified pricing)
    if model.contains("deepseek") {
        return ModelPricing {
            input: 0.28,
            output: 0.42,
            cache_read: 0.028,
            cache_write: 0.28,
        };
    }
    // Kimi / Moonshot
    if model.contains("kimi-k2.5") || model.contains("kimi-k25") {
        return ModelPricing {
            input: 0.60,
            output: 3.0,
            cache_read: 0.10,
            cache_write: 0.60,
        };
    }
    if model.contains("kimi") {
        return ModelPricing {
            input: 0.60,
            output: 2.50,
            cache_read: 0.15,
            cache_write: 0.60,
        };
    }
    // Zhipu GLM
    if model.contains("glm-4.5-flash") || model.contains("glm-4-5-flash") {
        return ModelPricing {
            input: 0.0,
            output: 0.0,
            cache_read: 0.0,
            cache_write: 0.0,
        };
    }
    if model.contains("glm-4.5-air") || model.contains("glm-4-5-air") {
        return ModelPricing {
            input: 0.20,
            output: 1.10,
            cache_read: 0.03,
            cache_write: 0.20,
        };
    }
    if model.contains("glm-4.7") || model.contains("glm-4-7") || model.contains("glm") {
        return ModelPricing {
            input: 0.60,
            output: 2.20,
            cache_read: 0.11,
            cache_write: 0.60,
        };
    }
    // Qwen / Bailian (lowest tier pricing)
    if model.contains("qwen3-max") {
        return ModelPricing {
            input: 1.20,
            output: 6.0,
            cache_read: 0.12,
            cache_write: 1.20,
        };
    }
    if model.contains("qwen3.5-plus") || model.contains("qwen35-plus") {
        return ModelPricing {
            input: 0.40,
            output: 2.40,
            cache_read: 0.04,
            cache_write: 0.40,
        };
    }
    if model.contains("qwen-plus") {
        return ModelPricing {
            input: 0.40,
            output: 1.20,
            cache_read: 0.04,
            cache_write: 0.40,
        };
    }
    if model.contains("qwen-flash") || model.contains("qwen") {
        return ModelPricing {
            input: 0.05,
            output: 0.40,
            cache_read: 0.005,
            cache_write: 0.05,
        };
    }
    // DouBao / Volcengine (lowest tier, CNY→USD @ ~7.2)
    if model.contains("doubao") {
        return ModelPricing {
            input: 0.17,
            output: 1.11,
            cache_read: 0.034,
            cache_write: 0.17,
        };
    }
    // MiniMax
    if model.contains("MiniMax-M2.5-highspeed") || model.contains("minimax-m2.5-highspeed") {
        return ModelPricing {
            input: 0.30,
            output: 2.40,
            cache_read: 0.03,
            cache_write: 0.30,
        };
    }
    if model.contains("MiniMax") || model.contains("minimax") {
        return ModelPricing {
            input: 0.30,
            output: 1.20,
            cache_read: 0.03,
            cache_write: 0.30,
        };
    }
    // MiMo / Xiaomi
    if model.contains("mimo") {
        return ModelPricing {
            input: 0.10,
            output: 0.30,
            cache_read: 0.01,
            cache_write: 0.10,
        };
    }

    // Default: Sonnet pricing
    claude_pricing(3.0, 15.0)
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

/// Estimate cost from token counts (input, output, cache read, cache write).
pub fn estimate_cost(
    model: &str,
    input_tokens: u64,
    output_tokens: u64,
    cache_read_tokens: u64,
    cache_write_tokens: u64,
) -> f64 {
    let p = get_pricing(model);
    (input_tokens as f64 * p.input
        + output_tokens as f64 * p.output
        + cache_read_tokens as f64 * p.cache_read
        + cache_write_tokens as f64 * p.cache_write)
        / 1_000_000.0
}
