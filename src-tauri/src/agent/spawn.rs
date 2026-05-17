use crate::agent::adapter::{self, AdapterSettings};

/// Build the command + args for a given agent (pipe-exec mode, not stream session)
pub fn build_agent_command(
    agent: &str,
    prompt: &str,
    settings: &AdapterSettings,
    print: bool,
    resume_thread_id: Option<&str>,
) -> Result<(String, Vec<String>), String> {
    log::debug!(
        "[spawn] build_agent_command: agent={}, print={}, model={:?}, perm={:?}, allowed={}, disallowed={}, resume={:?}",
        agent, print, settings.model, settings.permission_mode, settings.allowed_tools.len(), settings.disallowed_tools.len(), resume_thread_id
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
            let mut args: Vec<String> = vec!["exec".to_string()];
            // Resume: `codex exec resume <thread_id> --json "prompt"`
            if let Some(tid) = resume_thread_id {
                args.push("resume".to_string());
                args.push(tid.to_string());
            }
            args.push("--json".to_string());
            args.push("--skip-git-repo-check".to_string());

            // Codex per-session flags (AgentSettings).
            // `--ephemeral` MUST go before resume target rejection — but since
            // resume_thread_id was already added earlier, ordering here just
            // affects ergonomics. Codex parses flags positionally before the
            // optional prompt.
            if settings.ephemeral {
                args.push("--ephemeral".to_string());
            }
            if settings.ignore_user_config {
                args.push("--ignore-user-config".to_string());
            }
            if settings.ignore_rules {
                args.push("--ignore-rules".to_string());
            }
            if let Some(p) = &settings.profile {
                args.push("--profile".to_string());
                args.push(p.clone());
            }
            // model_reasoning_effort overrides config.toml on a per-session
            // basis. Empty string treated as unset (UI sends "" to clear).
            if let Some(e) = &settings.effort {
                if !e.is_empty() {
                    args.push("-c".to_string());
                    args.push(format!("model_reasoning_effort=\"{}\"", e));
                }
            }

            // Only pass --model if it's a Codex-compatible model.
            // The adapter fallback chain (agent.model → user.default_model) may
            // resolve to a Claude model name (e.g. "opus", "claude-*") which Codex
            // rejects. Skip those — let Codex use its own default.
            if let Some(ref m) = settings.model {
                let is_claude_model = m.is_empty()
                    || m.contains("claude")
                    || m.contains("opus")
                    || m.contains("sonnet")
                    || m.contains("haiku");
                if !is_claude_model {
                    args.push("--model".to_string());
                    args.push(m.to_string());
                }
            }

            // Map permission_mode → Codex sandbox/approval flags
            let is_read_only = matches!(settings.permission_mode.as_deref(), Some("plan"));
            if let Some(ref perm) = settings.permission_mode {
                match perm.as_str() {
                    "plan" => {
                        args.push("--sandbox".to_string());
                        args.push("read-only".to_string());
                    }
                    "bypassPermissions" | "dontAsk" => {
                        args.push("--dangerously-bypass-approvals-and-sandbox".to_string());
                    }
                    // "default" / "acceptEdits" / "auto" → Codex default (workspace-write sandbox)
                    _ => {}
                }
            }

            // Inject --add-dir (skip in read-only/plan mode — Codex ignores writable dirs when read-only)
            if !is_read_only {
                for dir in &settings.add_dirs {
                    args.push("--add-dir".to_string());
                    args.push(dir.clone());
                }
            } else if !settings.add_dirs.is_empty() {
                log::debug!("[spawn] skipping --add-dir in read-only/plan mode");
            }

            // Prompt must always be the last arg
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
    use super::*;
    use crate::agent::adapter::AdapterSettings;

    fn make_settings() -> AdapterSettings {
        AdapterSettings {
            model: None,
            allowed_tools: vec![],
            disallowed_tools: vec![],
            permission_mode: None,
            append_system_prompt: None,
            max_budget_usd: None,
            fallback_model: None,
            system_prompt: None,
            tool_set: None,
            add_dirs: vec![],
            json_schema: None,
            include_partial_messages: false,
            cli_debug: None,
            no_session_persistence: false,
            max_turns: None,
            effort: None,
            betas: vec![],
            agents_json: None,
            ephemeral: false,
            profile: None,
            ignore_user_config: false,
            ignore_rules: false,
        }
    }

    #[test]
    fn codex_resume_thread_id() {
        let s = make_settings();
        let (cmd, args) =
            build_agent_command("codex", "hello", &s, false, Some("tid_123")).unwrap();
        assert_eq!(cmd, "codex");
        assert!(args.contains(&"exec".to_string()));
        assert!(args.contains(&"resume".to_string()));
        assert!(args.contains(&"tid_123".to_string()));
        // prompt must be last
        assert_eq!(args.last().unwrap(), "hello");
    }

    #[test]
    fn codex_add_dirs() {
        let mut s = make_settings();
        s.add_dirs = vec!["/tmp/a".into(), "/tmp/b".into()];
        let (_, args) = build_agent_command("codex", "hi", &s, false, None).unwrap();
        let add_dir_count = args.iter().filter(|a| *a == "--add-dir").count();
        assert_eq!(add_dir_count, 2);
        assert!(args.contains(&"/tmp/a".to_string()));
        assert!(args.contains(&"/tmp/b".to_string()));
        assert_eq!(args.last().unwrap(), "hi");
    }

    #[test]
    fn codex_plan_mode_sandbox_read_only() {
        let mut s = make_settings();
        s.permission_mode = Some("plan".into());
        let (_, args) = build_agent_command("codex", "q", &s, false, None).unwrap();
        assert!(args.contains(&"--sandbox".to_string()));
        assert!(args.contains(&"read-only".to_string()));
        assert!(!args.contains(&"--dangerously-bypass-approvals-and-sandbox".to_string()));
    }

    #[test]
    fn codex_plan_mode_skips_add_dirs() {
        let mut s = make_settings();
        s.permission_mode = Some("plan".into());
        s.add_dirs = vec!["/extra".into()];
        let (_, args) = build_agent_command("codex", "q", &s, false, None).unwrap();
        assert!(args.contains(&"--sandbox".to_string()));
        assert!(!args.contains(&"--add-dir".to_string()));
    }

    #[test]
    fn codex_bypass_permissions() {
        let mut s = make_settings();
        s.permission_mode = Some("bypassPermissions".into());
        let (_, args) = build_agent_command("codex", "", &s, false, None).unwrap();
        assert!(args.contains(&"--dangerously-bypass-approvals-and-sandbox".to_string()));
        assert!(!args.contains(&"--sandbox".to_string()));
    }

    #[test]
    fn codex_dont_ask_bypass() {
        let mut s = make_settings();
        s.permission_mode = Some("dontAsk".into());
        let (_, args) = build_agent_command("codex", "", &s, false, None).unwrap();
        assert!(args.contains(&"--dangerously-bypass-approvals-and-sandbox".to_string()));
    }

    #[test]
    fn codex_prompt_always_last() {
        let mut s = make_settings();
        s.permission_mode = Some("plan".into());
        s.add_dirs = vec!["/dir".into()];
        let (_, args) = build_agent_command("codex", "my prompt", &s, false, Some("t1")).unwrap();
        assert_eq!(args.last().unwrap(), "my prompt");
    }

    #[test]
    fn codex_default_mode_no_extra_flags() {
        let mut s = make_settings();
        s.permission_mode = Some("default".into());
        let (_, args) = build_agent_command("codex", "q", &s, false, None).unwrap();
        assert!(!args.contains(&"--sandbox".to_string()));
        assert!(!args.contains(&"--dangerously-bypass-approvals-and-sandbox".to_string()));
    }

    // ── Codex per-session flags ──

    #[test]
    fn codex_no_per_session_flags_by_default() {
        let s = make_settings();
        let (_, args) = build_agent_command("codex", "q", &s, false, None).unwrap();
        assert!(!args.contains(&"--ephemeral".to_string()));
        assert!(!args.contains(&"--profile".to_string()));
        assert!(!args.contains(&"--ignore-user-config".to_string()));
        assert!(!args.contains(&"--ignore-rules".to_string()));
        assert!(!args.iter().any(|a| a.contains("model_reasoning_effort")));
    }

    #[test]
    fn codex_ephemeral_flag() {
        let mut s = make_settings();
        s.ephemeral = true;
        let (_, args) = build_agent_command("codex", "q", &s, false, None).unwrap();
        assert!(args.contains(&"--ephemeral".to_string()));
    }

    #[test]
    fn codex_ignore_user_config_flag() {
        let mut s = make_settings();
        s.ignore_user_config = true;
        let (_, args) = build_agent_command("codex", "q", &s, false, None).unwrap();
        assert!(args.contains(&"--ignore-user-config".to_string()));
    }

    #[test]
    fn codex_ignore_rules_flag() {
        let mut s = make_settings();
        s.ignore_rules = true;
        let (_, args) = build_agent_command("codex", "q", &s, false, None).unwrap();
        assert!(args.contains(&"--ignore-rules".to_string()));
    }

    #[test]
    fn codex_profile_flag() {
        let mut s = make_settings();
        s.profile = Some("dev".into());
        let (_, args) = build_agent_command("codex", "q", &s, false, None).unwrap();
        let idx = args
            .iter()
            .position(|a| a == "--profile")
            .expect("--profile");
        assert_eq!(args[idx + 1], "dev");
    }

    #[test]
    fn codex_profile_empty_string_skipped() {
        // build_adapter_settings filters empty strings to None, but spawn.rs only
        // checks Some(&p) without re-validating. Guard against future regressions
        // by asserting spawn doesn't emit --profile when the value is None.
        let mut s = make_settings();
        s.profile = None;
        let (_, args) = build_agent_command("codex", "q", &s, false, None).unwrap();
        assert!(!args.contains(&"--profile".to_string()));
    }

    #[test]
    fn codex_effort_emits_config_override() {
        for effort in ["minimal", "low", "medium", "high", "xhigh"] {
            let mut s = make_settings();
            s.effort = Some(effort.into());
            let (_, args) = build_agent_command("codex", "q", &s, false, None).unwrap();
            let expected = format!("model_reasoning_effort=\"{}\"", effort);
            assert!(
                args.iter().any(|a| a == &expected),
                "expected {} in args for effort={}",
                expected,
                effort
            );
        }
    }

    #[test]
    fn codex_effort_empty_skipped() {
        let mut s = make_settings();
        s.effort = Some("".into());
        let (_, args) = build_agent_command("codex", "q", &s, false, None).unwrap();
        assert!(!args.iter().any(|a| a.contains("model_reasoning_effort")));
    }

    #[test]
    fn codex_all_per_session_flags_together() {
        let mut s = make_settings();
        s.ephemeral = true;
        s.ignore_user_config = true;
        s.ignore_rules = true;
        s.profile = Some("ci".into());
        s.effort = Some("high".into());
        let (_, args) = build_agent_command("codex", "q", &s, false, None).unwrap();
        assert!(args.contains(&"--ephemeral".to_string()));
        assert!(args.contains(&"--ignore-user-config".to_string()));
        assert!(args.contains(&"--ignore-rules".to_string()));
        assert!(args.contains(&"--profile".to_string()));
        assert!(args.contains(&"ci".to_string()));
        assert!(args.iter().any(|a| a == "model_reasoning_effort=\"high\""));
        assert_eq!(args.last().unwrap(), "q"); // prompt still last
    }
}
