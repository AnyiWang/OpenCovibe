use crate::agent::adapter::{self, AdapterSettings};

/// Build the command + args for a given agent (pipe/PTY mode, not stream session)
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
            if let Some(ref m) = settings.model {
                if !m.is_empty() {
                    args.push("--model".to_string());
                    args.push(m.to_string());
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
