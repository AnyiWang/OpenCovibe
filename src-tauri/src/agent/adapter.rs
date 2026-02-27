use crate::agent::session_actor::SessionActorHandle;
use crate::models::{AgentSettings, SessionMode, UserSettings};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

// ── Session map (actor-based) ──

pub type ActorSessionMap = Arc<Mutex<HashMap<String, SessionActorHandle>>>;

pub fn new_actor_session_map() -> ActorSessionMap {
    Arc::new(Mutex::new(HashMap::new()))
}

// ── Adapter settings ──

pub struct AdapterSettings {
    pub model: Option<String>,
    pub allowed_tools: Vec<String>,
    pub disallowed_tools: Vec<String>,
    pub permission_mode: Option<String>,
    pub append_system_prompt: Option<String>,
    pub max_budget_usd: Option<f64>,
    pub fallback_model: Option<String>,
    pub system_prompt: Option<String>,
    pub tool_set: Option<String>,
    pub add_dirs: Vec<String>,
    pub json_schema: Option<String>,
    pub include_partial_messages: bool,
    pub cli_debug: Option<String>,
    pub no_session_persistence: bool,
    pub max_turns: Option<u32>,
    pub effort: Option<String>,
    pub betas: Vec<String>,
    pub agents_json: Option<String>,
}

/// Map OpenCovibe permission mode names to Claude CLI `--permission-mode` values.
fn map_permission_mode(mode: &str) -> String {
    match mode {
        "ask" => "default".to_string(),
        "auto_read" => "acceptEdits".to_string(),
        "auto_all" => "bypassPermissions".to_string(),
        "delegate" => "delegate".to_string(),
        "dont_ask" => "dontAsk".to_string(),
        other => {
            log::warn!(
                "[adapter] unknown permission_mode '{}', passing through to CLI",
                other
            );
            other.to_string()
        }
    }
}

/// Build a unified `AdapterSettings` from agent + user settings.
/// Agent-level settings take priority over user-level.
/// `model_override` (from UI per-message) takes highest priority.
pub fn build_adapter_settings(
    agent: &AgentSettings,
    user: &UserSettings,
    model_override: Option<String>,
) -> AdapterSettings {
    let model = model_override
        .filter(|m| !m.is_empty())
        .or_else(|| agent.model.clone())
        .or_else(|| user.default_model.clone());

    let allowed_tools = if agent.allowed_tools.is_empty() {
        user.allowed_tools.clone()
    } else {
        agent.allowed_tools.clone()
    };

    let disallowed_tools = agent.disallowed_tools.clone().unwrap_or_default();

    // Permission mode: plan_mode=true is a UI shortcut for "plan"
    let permission_mode = if agent.plan_mode.unwrap_or(false) {
        Some("plan".to_string())
    } else {
        let raw = &user.permission_mode;
        if raw.is_empty() {
            None
        } else {
            Some(map_permission_mode(raw))
        }
    };

    let append_system_prompt = agent.append_system_prompt.clone();

    // Budget: agent-level overrides user-level
    let max_budget_usd = agent.max_budget_usd.or(user.max_budget_usd);

    // Fallback model: agent-level overrides user-level
    let fallback_model = agent
        .fallback_model
        .clone()
        .or_else(|| user.fallback_model.clone());

    let system_prompt = agent.system_prompt.clone();
    let tool_set = agent.tool_set.clone();
    let add_dirs = agent.add_dirs.clone().unwrap_or_default();
    let json_schema = agent
        .json_schema
        .as_ref()
        .and_then(|v| serde_json::to_string(v).ok());
    // Default to true: stream sessions need partial messages for real-time streaming.
    // Users can explicitly set false in agent config to disable.
    let include_partial_messages = agent.include_partial_messages.unwrap_or(true);
    let cli_debug = agent.cli_debug.clone();
    let no_session_persistence = agent.no_session_persistence.unwrap_or(false);
    let max_turns = agent.max_turns;
    let effort = agent.effort.clone();
    let betas = agent.betas.clone().unwrap_or_default();
    let agents_json = agent.agents_json.clone();

    // Mutual exclusion: system_prompt takes priority over append_system_prompt
    if system_prompt.is_some() && append_system_prompt.is_some() {
        log::warn!("[adapter] both system_prompt and append_system_prompt are set; system_prompt takes priority");
    }

    log::debug!(
        "[adapter] build_adapter_settings: model={:?}, perm={:?}, allowed={}, disallowed={}, budget={:?}, fallback={:?}, sys_prompt={}chars, append_sys={}chars, tool_set={:?}, add_dirs={}, json_schema={}, partial={}, debug={:?}, no_persist={}, max_turns={:?}, effort={:?}, betas={}, agents_json={}",
        model,
        permission_mode,
        allowed_tools.len(),
        disallowed_tools.len(),
        max_budget_usd,
        fallback_model,
        system_prompt.as_ref().map_or(0, |s| s.len()),
        append_system_prompt.as_ref().map_or(0, |s| s.len()),
        tool_set,
        add_dirs.len(),
        json_schema.is_some(),
        include_partial_messages,
        cli_debug,
        no_session_persistence,
        max_turns,
        effort,
        betas.len(),
        agents_json.is_some(),
    );

    AdapterSettings {
        model,
        allowed_tools,
        disallowed_tools,
        permission_mode,
        append_system_prompt,
        max_budget_usd,
        fallback_model,
        system_prompt,
        tool_set,
        add_dirs,
        json_schema,
        include_partial_messages,
        cli_debug,
        no_session_persistence,
        max_turns,
        effort,
        betas,
        agents_json,
    }
}

/// Build CLI args for settings flags (shared between stream and pipe/pty modes).
/// Returns Vec of args to append. `print_mode` controls print-only flags.
pub fn build_settings_args(settings: &AdapterSettings, print_mode: bool) -> Vec<String> {
    let mut args = Vec::new();

    // Model
    if let Some(ref m) = settings.model {
        if !m.is_empty() {
            args.push("--model".into());
            args.push(m.clone());
        }
    }

    // Allowed tools
    if !settings.allowed_tools.is_empty() {
        args.push("--allowedTools".into());
        args.push(settings.allowed_tools.join(","));
    }

    // Disallowed tools
    if !settings.disallowed_tools.is_empty() {
        args.push("--disallowed-tools".into());
        args.push(settings.disallowed_tools.join(","));
    }

    // Permission mode
    if let Some(ref perm) = settings.permission_mode {
        args.push("--permission-mode".into());
        args.push(perm.clone());
    }

    // System prompt takes priority over append_system_prompt
    if let Some(ref sp) = settings.system_prompt {
        args.push("--system-prompt".into());
        args.push(sp.clone());
    } else if let Some(ref asp) = settings.append_system_prompt {
        args.push("--append-system-prompt".into());
        args.push(asp.clone());
    }

    // Tool set (--tools)
    if let Some(ref ts) = settings.tool_set {
        args.push("--tools".into());
        args.push(ts.clone());
    }

    // Additional directories
    for dir in &settings.add_dirs {
        args.push("--add-dir".into());
        args.push(dir.clone());
    }

    // CLI debug
    if let Some(ref dbg) = settings.cli_debug {
        if dbg.is_empty() {
            args.push("--debug".into());
        } else {
            args.push("--debug".into());
            args.push(dbg.clone());
        }
    }

    // No session persistence
    if settings.no_session_persistence {
        args.push("--no-session-persistence".into());
    }

    // Budget and fallback apply in both stream and print modes for stream sessions,
    // but only in print mode for pipe/pty (single-shot)
    if let Some(budget) = settings.max_budget_usd {
        args.push("--max-budget-usd".into());
        args.push(budget.to_string());
    }
    if let Some(ref fb) = settings.fallback_model {
        args.push("--fallback-model".into());
        args.push(fb.clone());
    }

    // Max turns
    if let Some(turns) = settings.max_turns {
        args.push("--max-turns".into());
        args.push(turns.to_string());
    }

    // Effort level
    if let Some(ref eff) = settings.effort {
        if !eff.is_empty() {
            args.push("--effort".into());
            args.push(eff.clone());
        }
    }

    // Betas
    if !settings.betas.is_empty() {
        args.push("--betas".into());
        args.push(settings.betas.join(","));
    }

    // Custom agent definitions
    if let Some(ref agents) = settings.agents_json {
        if !agents.is_empty() {
            args.push("--agents".into());
            args.push(agents.clone());
        }
    }

    // Print-only flags
    if print_mode {
        if let Some(ref schema) = settings.json_schema {
            args.push("--json-schema".into());
            args.push(schema.clone());
        }
        // NOTE: --include-partial-messages is NOT added here because it requires
        // --output-format=stream-json (CLI error otherwise). Only spawn_cli_process
        // (session.rs) adds it, since it's the only caller in stream-json mode.
    }

    args
}

/// Validate settings + mode combination before spawning.
pub fn validate_session_params(
    settings: &AdapterSettings,
    mode: &SessionMode,
) -> Result<(), String> {
    if settings.no_session_persistence && !matches!(mode, SessionMode::New) {
        return Err("Cannot resume/continue with no_session_persistence enabled".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
        }
    }

    #[test]
    fn test_build_args_defaults_empty() {
        let s = make_settings();
        let args = build_settings_args(&s, false);
        assert!(args.is_empty(), "all-None settings should produce no args");
    }

    #[test]
    fn test_build_args_system_prompt_overrides_append() {
        let mut s = make_settings();
        s.system_prompt = Some("replace".into());
        s.append_system_prompt = Some("append".into());
        let args = build_settings_args(&s, false);
        assert!(args.contains(&"--system-prompt".to_string()));
        assert!(!args.contains(&"--append-system-prompt".to_string()));
    }

    #[test]
    fn test_build_args_append_when_no_system_prompt() {
        let mut s = make_settings();
        s.append_system_prompt = Some("append text".into());
        let args = build_settings_args(&s, false);
        assert!(args.contains(&"--append-system-prompt".to_string()));
        assert!(!args.contains(&"--system-prompt".to_string()));
    }

    #[test]
    fn test_build_args_json_schema_print_only() {
        let mut s = make_settings();
        s.json_schema = Some(r#"{"type":"object"}"#.into());
        assert!(
            !build_settings_args(&s, false).contains(&"--json-schema".to_string()),
            "json_schema should not appear in non-print mode"
        );
        assert!(
            build_settings_args(&s, true).contains(&"--json-schema".to_string()),
            "json_schema should appear in print mode"
        );
    }

    #[test]
    fn test_build_args_include_partial_never_emitted() {
        // --include-partial-messages requires --output-format=stream-json which only
        // spawn_cli_process (session.rs) uses. build_settings_args must never emit it.
        let mut s = make_settings();
        s.include_partial_messages = true;
        assert!(
            !build_settings_args(&s, false).contains(&"--include-partial-messages".to_string()),
            "include_partial_messages should not appear in non-print mode"
        );
        assert!(
            !build_settings_args(&s, true).contains(&"--include-partial-messages".to_string()),
            "include_partial_messages should not appear in print mode either (only session.rs adds it)"
        );
    }

    #[test]
    fn test_validate_no_persistence_resume_conflict() {
        let mut s = make_settings();
        s.no_session_persistence = true;
        assert!(validate_session_params(&s, &SessionMode::New).is_ok());
        assert!(validate_session_params(&s, &SessionMode::Resume).is_err());
        assert!(validate_session_params(&s, &SessionMode::Continue).is_err());
        assert!(validate_session_params(&s, &SessionMode::Fork).is_err());
    }

    #[test]
    fn test_validate_normal_modes_ok() {
        let s = make_settings();
        assert!(validate_session_params(&s, &SessionMode::New).is_ok());
        assert!(validate_session_params(&s, &SessionMode::Resume).is_ok());
        assert!(validate_session_params(&s, &SessionMode::Continue).is_ok());
        assert!(validate_session_params(&s, &SessionMode::Fork).is_ok());
    }

    #[test]
    fn test_build_args_add_dirs() {
        let mut s = make_settings();
        s.add_dirs = vec!["/path/a".into(), "/path/b".into()];
        let args = build_settings_args(&s, false);
        let add_dir_count = args.iter().filter(|a| *a == "--add-dir").count();
        assert_eq!(add_dir_count, 2);
        assert!(args.contains(&"/path/a".to_string()));
        assert!(args.contains(&"/path/b".to_string()));
    }

    #[test]
    fn test_build_args_cli_debug_empty_filter() {
        let mut s = make_settings();
        s.cli_debug = Some("".into());
        let args = build_settings_args(&s, false);
        assert!(args.contains(&"--debug".to_string()));
        // Empty filter → just "--debug" with no arg after it
        assert_eq!(args.iter().filter(|a| *a == "--debug").count(), 1);
        assert_eq!(args.len(), 1);
    }

    #[test]
    fn test_build_args_cli_debug_with_filter() {
        let mut s = make_settings();
        s.cli_debug = Some("api".into());
        let args = build_settings_args(&s, false);
        assert!(args.contains(&"--debug".to_string()));
        assert!(args.contains(&"api".to_string()));
    }

    #[test]
    fn test_build_args_all_flags() {
        let mut s = make_settings();
        s.model = Some("opus".into());
        s.allowed_tools = vec!["Read".into(), "Write".into()];
        s.disallowed_tools = vec!["Bash".into()];
        s.permission_mode = Some("plan".into());
        s.system_prompt = Some("Be helpful".into());
        s.tool_set = Some("extended".into());
        s.add_dirs = vec!["/extra".into()];
        s.cli_debug = Some("verbose".into());
        s.no_session_persistence = true;
        s.max_budget_usd = Some(5.0);
        s.fallback_model = Some("haiku".into());
        s.json_schema = Some(r#"{"type":"object"}"#.into());
        s.include_partial_messages = true;
        s.max_turns = Some(20);
        s.effort = Some("high".into());
        s.betas = vec!["context-1m-2025-08-07".into()];
        s.agents_json = Some(r#"[{"description":"test"}]"#.into());

        let args = build_settings_args(&s, true);

        assert!(args.contains(&"--model".to_string()));
        assert!(args.contains(&"opus".to_string()));
        assert!(args.contains(&"--allowedTools".to_string()));
        assert!(args.contains(&"--disallowed-tools".to_string()));
        assert!(args.contains(&"--permission-mode".to_string()));
        assert!(args.contains(&"plan".to_string()));
        assert!(args.contains(&"--system-prompt".to_string()));
        assert!(args.contains(&"Be helpful".to_string()));
        assert!(!args.contains(&"--append-system-prompt".to_string()));
        assert!(args.contains(&"--tools".to_string()));
        assert!(args.contains(&"extended".to_string()));
        assert!(args.contains(&"--add-dir".to_string()));
        assert!(args.contains(&"/extra".to_string()));
        assert!(args.contains(&"--debug".to_string()));
        assert!(args.contains(&"verbose".to_string()));
        assert!(args.contains(&"--no-session-persistence".to_string()));
        assert!(args.contains(&"--max-budget-usd".to_string()));
        assert!(args.contains(&"5".to_string()));
        assert!(args.contains(&"--fallback-model".to_string()));
        assert!(args.contains(&"haiku".to_string()));
        assert!(args.contains(&"--json-schema".to_string()));
        assert!(args.contains(&"--max-turns".to_string()));
        assert!(args.contains(&"20".to_string()));
        assert!(args.contains(&"--effort".to_string()));
        assert!(args.contains(&"high".to_string()));
        assert!(args.contains(&"--betas".to_string()));
        assert!(args.contains(&"context-1m-2025-08-07".to_string()));
        assert!(args.contains(&"--agents".to_string()));
        assert!(args.contains(&r#"[{"description":"test"}]"#.to_string()));
        // --include-partial-messages is NOT emitted by build_settings_args
        // (only spawn_cli_process adds it, since it requires --output-format=stream-json)
        assert!(!args.contains(&"--include-partial-messages".to_string()));
    }

    #[test]
    fn test_build_args_effort() {
        let mut s = make_settings();
        s.effort = Some("low".into());
        let args = build_settings_args(&s, false);
        assert!(args.contains(&"--effort".to_string()));
        assert!(args.contains(&"low".to_string()));
    }

    #[test]
    fn test_build_args_effort_empty_skipped() {
        let mut s = make_settings();
        s.effort = Some("".into());
        let args = build_settings_args(&s, false);
        assert!(!args.contains(&"--effort".to_string()));
    }

    #[test]
    fn test_build_args_betas() {
        let mut s = make_settings();
        s.betas = vec!["context-1m-2025-08-07".into()];
        let args = build_settings_args(&s, false);
        assert!(args.contains(&"--betas".to_string()));
        assert!(args.contains(&"context-1m-2025-08-07".to_string()));
    }

    #[test]
    fn test_build_args_agents_json() {
        let mut s = make_settings();
        s.agents_json = Some(r#"[{"description":"reviewer"}]"#.into());
        let args = build_settings_args(&s, false);
        assert!(args.contains(&"--agents".to_string()));
        assert!(args.contains(&r#"[{"description":"reviewer"}]"#.to_string()));
    }

    #[test]
    fn test_build_args_agents_json_empty_skipped() {
        let mut s = make_settings();
        s.agents_json = Some("".into());
        let args = build_settings_args(&s, false);
        assert!(!args.contains(&"--agents".to_string()));
    }
}
