use crate::agent::adapter::{self, AdapterSettings};

/// True if `model` looks like a Claude model name (opus/sonnet/haiku/claude-*).
/// Codex CLI rejects these; real Codex models (gpt-*, o3, …) never contain these
/// substrings, so there are no false positives.
fn is_claude_model_name(model: &str) -> bool {
    let m = model.to_ascii_lowercase();
    m.contains("claude") || m.contains("opus") || m.contains("sonnet") || m.contains("haiku")
}

/// Build the command + args for a given agent (pipe-exec mode, not stream session)
pub fn build_agent_command(
    agent: &str,
    prompt: &str,
    settings: &AdapterSettings,
    print: bool,
) -> Result<(String, Vec<String>), String> {
    log::debug!(
        "[spawn] build_agent_command: agent={}, print={}, model={:?}, perm={:?}, allowed={}, disallowed={}",
        agent, print, settings.model, settings.permission_mode, settings.allowed_tools.len(), settings.disallowed_tools.len()
    );
    match agent {
        "claude" => {
            let mut args: Vec<String> = vec![];
            if print {
                args.push("--print".to_string());
            }

            // Use shared helper for all settings flags
            args.extend(adapter::build_settings_args(settings, print));

            if !prompt.is_empty() {
                args.push(prompt.to_string());
            }
            log::debug!("[spawn] claude command: claude {}", args.join(" "));
            Ok(("claude".to_string(), args))
        }
        "codex" => {
            let mut args: Vec<String> = vec![
                "exec".to_string(),
                "--json".to_string(),
                "--skip-git-repo-check".to_string(),
            ];
            // Codex CLI rejects Claude model names — they leak in when the user's
            // default_model is a Claude model and they switch to Codex without picking a
            // Codex model. Skip --model so Codex uses its own configured default instead of
            // failing on spawn. (audit #13)
            if let Some(ref m) = settings.model {
                if !m.is_empty() {
                    if is_claude_model_name(m) {
                        log::debug!(
                            "[spawn] codex: skipping Claude model name '{}' (Codex would reject it); using Codex default",
                            m
                        );
                    } else {
                        args.push("--model".to_string());
                        args.push(m.to_string());
                    }
                }
            }
            if !prompt.is_empty() {
                args.push(prompt.to_string());
            }
            log::debug!("[spawn] codex command: codex {}", args.join(" "));
            Ok(("codex".to_string(), args))
        }
        _ => Err(format!(
            "Unsupported agent: {}. Supported: claude, codex",
            agent
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::is_claude_model_name;

    #[test]
    fn flags_claude_model_names() {
        for m in [
            "opus",
            "sonnet",
            "haiku",
            "claude-opus-4-8",
            "Claude-Sonnet-4-6",
            "OPUS",
        ] {
            assert!(is_claude_model_name(m), "{m} should be flagged");
        }
    }

    #[test]
    fn allows_codex_model_names() {
        for m in ["gpt-5-codex", "o3", "gpt-4.1", "o4-mini", "openai"] {
            assert!(!is_claude_model_name(m), "{m} should be allowed");
        }
    }
}
