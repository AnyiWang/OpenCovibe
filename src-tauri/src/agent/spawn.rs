use crate::agent::adapter::{self, AdapterSettings};
use crate::models::CodexProviderCredential;

/// Build the `-c model_providers.*` config overrides for a Codex third-party provider
/// (OpenAI Responses API gateway). Accepted on `codex exec`, `codex exec resume`, and
/// `codex app-server`. The API key is supplied separately via [`codex_provider_env`].
/// `requires_openai_auth=false` is required for non-OpenAI gateways, otherwise Codex ignores
/// `env_key` and falls back to ChatGPT login. `wire_api` is always "responses" (no "chat").
/// Does NOT include `--model` — callers handle the model arg differently (the exec path lets the
/// provider model win over the generic model, the side-question path inherits the run's model).
pub fn codex_provider_config_args(p: &CodexProviderCredential) -> Vec<String> {
    let id = &p.id;
    let mut args = vec![
        "-c".to_string(),
        format!("model_provider=\"{}\"", id),
        "-c".to_string(),
        format!("model_providers.{}.name=\"{}\"", id, p.name),
        "-c".to_string(),
        format!("model_providers.{}.base_url=\"{}\"", id, p.base_url),
    ];
    if !p.env_key.is_empty() {
        args.push("-c".to_string());
        args.push(format!("model_providers.{}.env_key=\"{}\"", id, p.env_key));
    }
    args.push("-c".to_string());
    args.push(format!(
        "model_providers.{}.wire_api=\"{}\"",
        id, p.wire_api
    ));
    args.push("-c".to_string());
    args.push(format!("model_providers.{}.requires_openai_auth=false", id));
    args
}

/// The env var entry (`env_key` → `api_key`) Codex reads the provider API key from, if both are
/// set. Returned as `(name, value)` so callers can inject it onto the child process. `None` when
/// the provider has no key or no `env_key` (e.g. a ChatGPT-login provider).
pub fn codex_provider_env(p: &CodexProviderCredential) -> Option<(String, String)> {
    let key = p.api_key.as_ref().filter(|k| !k.is_empty())?;
    if p.env_key.is_empty() {
        return None;
    }
    Some((p.env_key.clone(), key.clone()))
}

/// Build the command + args for a given agent (pipe-exec mode, not stream session)
pub fn build_agent_command(
    agent: &str,
    prompt: &str,
    settings: &AdapterSettings,
    print: bool,
    resume_thread_id: Option<&str>,
    image_paths: &[String],
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
            // `--search` is a TOP-LEVEL codex flag (NOT a `codex exec` flag — `codex exec
            // --search` errors "unexpected argument"). It must precede the `exec` subcommand:
            // `codex --search exec ...`. Verified accepted on both new sessions and resume.
            let mut args: Vec<String> = vec![];
            if settings.web_search {
                args.push("--search".to_string());
            }
            args.push("exec".to_string());
            // Resume: `codex exec resume <thread_id> --json "prompt"`
            if let Some(tid) = resume_thread_id {
                args.push("resume".to_string());
                args.push(tid.to_string());
            }
            args.push("--json".to_string());
            args.push("--skip-git-repo-check".to_string());

            // Codex third-party provider (OpenAI Responses API) → inject as `-c model_providers.*`
            // overrides (shared with the app-server + side-question paths). Accepted on both
            // `codex exec` and `exec resume` (no resume gating). The API key is supplied separately
            // via the env var named by `env_key`, set on the child process at spawn time (see
            // run_agent extra_env). Here we additionally pin the provider-side `--model`.
            if let Some(p) = &settings.codex_provider {
                args.extend(codex_provider_config_args(p));
                if !p.model.is_empty() {
                    args.push("--model".to_string());
                    args.push(p.model.clone());
                }
            }

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
            // `codex exec resume` does NOT accept --profile (only --json,
            // --skip-git-repo-check, --ephemeral, --ignore-user-config,
            // --ignore-rules, --model, -c, --enable/--disable, --image,
            // --dangerously-bypass-approvals-and-sandbox, --last, --all).
            // Injecting --profile on a resume call would make Codex exit with
            // "error: unexpected argument '--profile' found". Only emit it for
            // new sessions; the profile applied when the session was first
            // created is persisted on disk and reused automatically.
            if resume_thread_id.is_none() {
                if let Some(p) = &settings.profile {
                    args.push("--profile".to_string());
                    args.push(p.clone());
                }
            } else if settings.profile.is_some() {
                log::debug!(
                    "[spawn] skipping --profile on codex resume (not supported by exec resume)"
                );
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
            // Skip entirely if a codex_provider already set --model (its provider-side model wins).
            let provider_set_model = settings
                .codex_provider
                .as_ref()
                .is_some_and(|p| !p.model.is_empty());
            if !provider_set_model {
                if let Some(ref m) = settings.model {
                    let lm = m.to_lowercase();
                    let is_claude_model = lm.is_empty()
                        || lm.contains("claude")
                        || lm.contains("opus")
                        || lm.contains("sonnet")
                        || lm.contains("haiku");
                    if !is_claude_model {
                        args.push("--model".to_string());
                        args.push(m.to_string());
                    }
                }
            }

            // Map permission_mode → Codex sandbox/approval flags.
            // `codex exec resume` does NOT accept --sandbox (verified against
            // codex v0.130 --help); the sandbox mode of the original session is
            // persisted on disk and reused. Only --dangerously-bypass-... is
            // accepted by both `exec` and `exec resume`.
            let is_resume = resume_thread_id.is_some();
            let is_read_only = matches!(settings.permission_mode.as_deref(), Some("plan"));
            if let Some(ref perm) = settings.permission_mode {
                match perm.as_str() {
                    "plan" => {
                        if !is_resume {
                            args.push("--sandbox".to_string());
                            args.push("read-only".to_string());
                        } else {
                            log::debug!(
                                "[spawn] skipping --sandbox read-only on codex resume (not supported by exec resume)"
                            );
                        }
                    }
                    "bypassPermissions" | "dontAsk" => {
                        // --dangerously-bypass-approvals-and-sandbox IS supported by exec resume.
                        args.push("--dangerously-bypass-approvals-and-sandbox".to_string());
                    }
                    // "default" / "acceptEdits" / "auto" → Codex default (workspace-write sandbox)
                    _ => {}
                }
            }

            // Inject --add-dir. Skip on resume because `codex exec resume` does
            // NOT accept --add-dir (verified against codex v0.130 --help).
            // Also skip in read-only/plan mode — Codex ignores writable dirs
            // when sandbox=read-only.
            if !is_resume && !is_read_only {
                for dir in &settings.add_dirs {
                    args.push("--add-dir".to_string());
                    args.push(dir.clone());
                }
            } else if !settings.add_dirs.is_empty() {
                log::debug!(
                    "[spawn] skipping --add-dir (resume={}, read_only={})",
                    is_resume,
                    is_read_only
                );
            }

            // Attach images via --image. Accepted by both `codex exec` and `exec resume`,
            // no resume gating. `--image` is VARIADIC (`<FILE>...`), so the two-arg form
            // `--image <path>` would swallow the trailing prompt as a second file. Use the
            // `--image=<path>` form (one value each) so the positional prompt survives.
            for path in image_paths {
                args.push(format!("--image={}", path));
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
            web_search: false,
            codex_provider: None,
        }
    }

    #[test]
    fn codex_resume_thread_id() {
        let s = make_settings();
        let (cmd, args) =
            build_agent_command("codex", "hello", &s, false, Some("tid_123"), &[]).unwrap();
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
        let (_, args) = build_agent_command("codex", "hi", &s, false, None, &[]).unwrap();
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
        let (_, args) = build_agent_command("codex", "q", &s, false, None, &[]).unwrap();
        assert!(args.contains(&"--sandbox".to_string()));
        assert!(args.contains(&"read-only".to_string()));
        assert!(!args.contains(&"--dangerously-bypass-approvals-and-sandbox".to_string()));
    }

    #[test]
    fn codex_plan_mode_skips_add_dirs() {
        let mut s = make_settings();
        s.permission_mode = Some("plan".into());
        s.add_dirs = vec!["/extra".into()];
        let (_, args) = build_agent_command("codex", "q", &s, false, None, &[]).unwrap();
        assert!(args.contains(&"--sandbox".to_string()));
        assert!(!args.contains(&"--add-dir".to_string()));
    }

    #[test]
    fn codex_bypass_permissions() {
        let mut s = make_settings();
        s.permission_mode = Some("bypassPermissions".into());
        let (_, args) = build_agent_command("codex", "", &s, false, None, &[]).unwrap();
        assert!(args.contains(&"--dangerously-bypass-approvals-and-sandbox".to_string()));
        assert!(!args.contains(&"--sandbox".to_string()));
    }

    #[test]
    fn codex_dont_ask_bypass() {
        let mut s = make_settings();
        s.permission_mode = Some("dontAsk".into());
        let (_, args) = build_agent_command("codex", "", &s, false, None, &[]).unwrap();
        assert!(args.contains(&"--dangerously-bypass-approvals-and-sandbox".to_string()));
    }

    #[test]
    fn codex_prompt_always_last() {
        let mut s = make_settings();
        s.permission_mode = Some("plan".into());
        s.add_dirs = vec!["/dir".into()];
        let (_, args) =
            build_agent_command("codex", "my prompt", &s, false, Some("t1"), &[]).unwrap();
        assert_eq!(args.last().unwrap(), "my prompt");
    }

    #[test]
    fn codex_default_mode_no_extra_flags() {
        let mut s = make_settings();
        s.permission_mode = Some("default".into());
        let (_, args) = build_agent_command("codex", "q", &s, false, None, &[]).unwrap();
        assert!(!args.contains(&"--sandbox".to_string()));
        assert!(!args.contains(&"--dangerously-bypass-approvals-and-sandbox".to_string()));
    }

    // ── Codex per-session flags ──

    #[test]
    fn codex_no_per_session_flags_by_default() {
        let s = make_settings();
        let (_, args) = build_agent_command("codex", "q", &s, false, None, &[]).unwrap();
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
        let (_, args) = build_agent_command("codex", "q", &s, false, None, &[]).unwrap();
        assert!(args.contains(&"--ephemeral".to_string()));
    }

    #[test]
    fn codex_ignore_user_config_flag() {
        let mut s = make_settings();
        s.ignore_user_config = true;
        let (_, args) = build_agent_command("codex", "q", &s, false, None, &[]).unwrap();
        assert!(args.contains(&"--ignore-user-config".to_string()));
    }

    #[test]
    fn codex_web_search_flag() {
        let mut s = make_settings();
        s.web_search = true;
        // --search is a TOP-LEVEL flag (before `exec`), accepted on both new and resume.
        let (_, args) = build_agent_command("codex", "q", &s, false, None, &[]).unwrap();
        assert_eq!(args.first().unwrap(), "--search");
        assert_eq!(args[1], "exec");
        let (_, resume_args) =
            build_agent_command("codex", "q", &s, false, Some("tid-1"), &[]).unwrap();
        assert_eq!(resume_args.first().unwrap(), "--search");
        assert!(resume_args.contains(&"resume".to_string()));
        // No web_search → no --search, exec is first.
        s.web_search = false;
        let (_, none_args) = build_agent_command("codex", "q", &s, false, None, &[]).unwrap();
        assert_eq!(none_args.first().unwrap(), "exec");
        assert!(!none_args.contains(&"--search".to_string()));
    }

    #[test]
    fn codex_filters_capitalized_claude_model() {
        // Case-insensitive: a capitalized Claude id must still be filtered out so it's
        // never passed to `codex --model` (Codex would reject it).
        let mut s = make_settings();
        s.model = Some("Claude-Opus-4".into());
        let (_, args) = build_agent_command("codex", "q", &s, false, None, &[]).unwrap();
        assert!(!args.contains(&"--model".to_string()));
        assert!(!args.contains(&"Claude-Opus-4".to_string()));
        // A real Codex model still passes through.
        s.model = Some("gpt-5.5".into());
        let (_, args2) = build_agent_command("codex", "q", &s, false, None, &[]).unwrap();
        assert!(args2.contains(&"--model".to_string()));
        assert!(args2.contains(&"gpt-5.5".to_string()));
    }

    #[test]
    fn codex_image_flags() {
        let s = make_settings();
        let imgs = vec!["/tmp/a.png".to_string()];
        // `--image=<path>` form (NOT two-arg) so the variadic flag doesn't eat the prompt.
        let (_, args) = build_agent_command("codex", "q", &s, false, None, &imgs).unwrap();
        assert!(args.contains(&"--image=/tmp/a.png".to_string()));
        assert!(!args.contains(&"--image".to_string())); // never the bare two-arg form
        assert_eq!(args.last().unwrap(), "q"); // prompt still last
                                               // Resume → still attached (codex exec resume accepts --image).
        let (_, resume_args) =
            build_agent_command("codex", "q", &s, false, Some("tid-1"), &imgs).unwrap();
        assert!(resume_args.contains(&"--image=/tmp/a.png".to_string()));
        // No images → no --image.
        let (_, none_args) = build_agent_command("codex", "q", &s, false, None, &[]).unwrap();
        assert!(!none_args.iter().any(|a| a.starts_with("--image")));
    }

    #[test]
    fn codex_provider_emits_overrides() {
        use crate::models::CodexProviderCredential;
        let mut s = make_settings();
        s.model = Some("opus".into()); // would be filtered; provider model must win instead
        s.codex_provider = Some(CodexProviderCredential {
            id: "vercel".into(),
            name: "Vercel AI Gateway".into(),
            base_url: "https://ai-gateway.vercel.sh/v1".into(),
            env_key: "AI_GATEWAY_API_KEY".into(),
            wire_api: "responses".into(),
            model: "openai/gpt-5.5".into(),
            api_key: Some("sk-secret".into()),
        });
        // New session
        let (_, args) = build_agent_command("codex", "q", &s, false, None, &[]).unwrap();
        let joined = args.join(" ");
        assert!(joined.contains("model_provider=\"vercel\""));
        assert!(
            joined.contains("model_providers.vercel.base_url=\"https://ai-gateway.vercel.sh/v1\"")
        );
        assert!(joined.contains("model_providers.vercel.env_key=\"AI_GATEWAY_API_KEY\""));
        assert!(joined.contains("model_providers.vercel.wire_api=\"responses\""));
        assert!(joined.contains("model_providers.vercel.requires_openai_auth=false"));
        // provider model wins over the (filtered) generic model; exactly one --model
        assert_eq!(args.iter().filter(|a| *a == "--model").count(), 1);
        assert!(args.contains(&"openai/gpt-5.5".to_string()));
        // never leaks the api key into args (it goes via env)
        assert!(!joined.contains("sk-secret"));
        // Accepted on resume too (re-applied since -c isn't persisted)
        let (_, ra) = build_agent_command("codex", "q", &s, false, Some("tid"), &[]).unwrap();
        assert!(ra.join(" ").contains("model_provider=\"vercel\""));
        // Absent → no provider overrides
        s.codex_provider = None;
        let (_, na) = build_agent_command("codex", "q", &s, false, None, &[]).unwrap();
        assert!(!na.join(" ").contains("model_provider="));
    }

    #[test]
    fn codex_provider_helper_shapes() {
        use crate::models::CodexProviderCredential;
        let p = CodexProviderCredential {
            id: "vercel".into(),
            name: "Vercel AI Gateway".into(),
            base_url: "https://ai-gateway.vercel.sh/v1".into(),
            env_key: "AI_GATEWAY_API_KEY".into(),
            wire_api: "responses".into(),
            model: "openai/gpt-5.5".into(),
            api_key: Some("sk-secret".into()),
        };
        // Config args: the -c overrides, NOT --model, and never the api key.
        let args = codex_provider_config_args(&p);
        let joined = args.join(" ");
        assert!(joined.contains("model_provider=\"vercel\""));
        assert!(
            joined.contains("model_providers.vercel.base_url=\"https://ai-gateway.vercel.sh/v1\"")
        );
        assert!(joined.contains("model_providers.vercel.env_key=\"AI_GATEWAY_API_KEY\""));
        assert!(joined.contains("model_providers.vercel.wire_api=\"responses\""));
        assert!(joined.contains("model_providers.vercel.requires_openai_auth=false"));
        assert!(!args.iter().any(|a| a == "--model"));
        assert!(!joined.contains("sk-secret"));
        // Env: env_key → api_key.
        assert_eq!(
            codex_provider_env(&p),
            Some(("AI_GATEWAY_API_KEY".into(), "sk-secret".into()))
        );
        // No api key → no env entry.
        let mut no_key = p.clone();
        no_key.api_key = None;
        assert_eq!(codex_provider_env(&no_key), None);
        // No env_key → no env entry (ChatGPT-login style provider).
        let mut no_env = p.clone();
        no_env.env_key = String::new();
        assert_eq!(codex_provider_env(&no_env), None);
        // env_key with empty config still omits the env_key override line.
        let args2 = codex_provider_config_args(&no_env);
        assert!(!args2.join(" ").contains(".env_key="));
    }

    #[test]
    fn codex_ignore_rules_flag() {
        let mut s = make_settings();
        s.ignore_rules = true;
        let (_, args) = build_agent_command("codex", "q", &s, false, None, &[]).unwrap();
        assert!(args.contains(&"--ignore-rules".to_string()));
    }

    #[test]
    fn codex_profile_flag() {
        let mut s = make_settings();
        s.profile = Some("dev".into());
        let (_, args) = build_agent_command("codex", "q", &s, false, None, &[]).unwrap();
        let idx = args
            .iter()
            .position(|a| a == "--profile")
            .expect("--profile");
        assert_eq!(args[idx + 1], "dev");
    }

    #[test]
    fn codex_profile_skipped_on_resume() {
        // `codex exec resume` rejects --profile; the profile from session
        // creation is persisted and reused automatically. Regression guard:
        // verify spawn does NOT emit --profile when resume_thread_id is set.
        let mut s = make_settings();
        s.profile = Some("dev".into());
        let (_, args) = build_agent_command("codex", "q", &s, false, Some("tid_42"), &[]).unwrap();
        assert!(args.contains(&"resume".to_string()));
        assert!(args.contains(&"tid_42".to_string()));
        assert!(!args.contains(&"--profile".to_string()));
    }

    #[test]
    fn codex_sandbox_skipped_on_resume() {
        // `codex exec resume` rejects --sandbox; the sandbox mode of the
        // original session is persisted on disk. Without this guard, resuming
        // a plan-mode Codex run would fail with "unexpected argument
        // '--sandbox' found".
        let mut s = make_settings();
        s.permission_mode = Some("plan".into());
        let (_, args) = build_agent_command("codex", "q", &s, false, Some("tid_x"), &[]).unwrap();
        assert!(args.contains(&"resume".to_string()));
        assert!(!args.contains(&"--sandbox".to_string()));
        assert!(!args.contains(&"read-only".to_string()));
    }

    #[test]
    fn codex_bypass_still_emitted_on_resume() {
        // --dangerously-bypass-approvals-and-sandbox IS supported by
        // `codex exec resume` — keep emitting it.
        let mut s = make_settings();
        s.permission_mode = Some("bypassPermissions".into());
        let (_, args) = build_agent_command("codex", "q", &s, false, Some("tid_x"), &[]).unwrap();
        assert!(args.contains(&"--dangerously-bypass-approvals-and-sandbox".to_string()));
    }

    #[test]
    fn codex_add_dir_skipped_on_resume() {
        // `codex exec resume` rejects --add-dir; the workspace of the original
        // session is persisted on disk. Without this guard, resuming a run
        // configured with add_dirs would fail with "unexpected argument
        // '--add-dir' found".
        let mut s = make_settings();
        s.add_dirs = vec!["/tmp/a".into(), "/tmp/b".into()];
        let (_, args) = build_agent_command("codex", "q", &s, false, Some("tid_x"), &[]).unwrap();
        assert!(args.contains(&"resume".to_string()));
        assert!(!args.contains(&"--add-dir".to_string()));
    }

    #[test]
    fn codex_profile_empty_string_skipped() {
        // build_adapter_settings filters empty strings to None, but spawn.rs only
        // checks Some(&p) without re-validating. Guard against future regressions
        // by asserting spawn doesn't emit --profile when the value is None.
        let mut s = make_settings();
        s.profile = None;
        let (_, args) = build_agent_command("codex", "q", &s, false, None, &[]).unwrap();
        assert!(!args.contains(&"--profile".to_string()));
    }

    #[test]
    fn codex_effort_emits_config_override() {
        for effort in ["none", "minimal", "low", "medium", "high", "xhigh"] {
            let mut s = make_settings();
            s.effort = Some(effort.into());
            let (_, args) = build_agent_command("codex", "q", &s, false, None, &[]).unwrap();
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
        let (_, args) = build_agent_command("codex", "q", &s, false, None, &[]).unwrap();
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
        let (_, args) = build_agent_command("codex", "q", &s, false, None, &[]).unwrap();
        assert!(args.contains(&"--ephemeral".to_string()));
        assert!(args.contains(&"--ignore-user-config".to_string()));
        assert!(args.contains(&"--ignore-rules".to_string()));
        assert!(args.contains(&"--profile".to_string()));
        assert!(args.contains(&"ci".to_string()));
        assert!(args.iter().any(|a| a == "model_reasoning_effort=\"high\""));
        assert_eq!(args.last().unwrap(), "q"); // prompt still last
    }
}
