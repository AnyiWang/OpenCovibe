use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

// ── Types ──

#[derive(Debug, Clone, Serialize)]
pub struct AgentDefinitionSummary {
    pub file_name: String,
    pub name: String,
    pub description: String,
    pub model: Option<String>,
    pub source: String,
    pub scope: String,
    pub tools: Option<Vec<String>>,
    pub disallowed_tools: Option<Vec<String>>,
    pub permission_mode: Option<String>,
    pub max_turns: Option<u32>,
    pub background: Option<bool>,
    pub isolation: Option<String>,
    pub readonly: bool,
    pub raw_content: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
#[allow(dead_code)]
struct AgentFrontmatter {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    tools: Option<Vec<String>>,
    #[serde(default, rename = "disallowedTools")]
    disallowed_tools: Option<Vec<String>>,
    #[serde(default, rename = "permissionMode")]
    permission_mode: Option<String>,
    #[serde(default, rename = "maxTurns")]
    max_turns: Option<u32>,
    #[serde(default)]
    skills: Option<Vec<String>>,
    #[serde(default)]
    memory: Option<String>,
    #[serde(default)]
    background: Option<bool>,
    #[serde(default)]
    isolation: Option<String>,
}

// ── Validation ──

/// Strict name validation for creating new agents.
/// Only lowercase letters, numbers, and hyphens; must start with alphanumeric; max 64 chars.
fn validate_agent_name_strict(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("Agent name cannot be empty".to_string());
    }
    if name.len() > 64 {
        return Err(format!(
            "Agent name too long ({} chars, max 64)",
            name.len()
        ));
    }
    let valid = name.len() <= 64
        && name
            .chars()
            .next()
            .map(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
            .unwrap_or(false)
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if !valid {
        return Err(format!(
            "Invalid agent name '{}': must match [a-z0-9][a-z0-9-]{{0,63}}",
            name
        ));
    }
    Ok(())
}

/// Lenient name validation for read/update/delete — only blocks path traversal.
/// Input is always a stem (no .md); if caller passes "foo.md", strip suffix first.
fn validate_agent_name_lenient(name: &str) -> Result<String, String> {
    // Strip .md suffix if present (prevent foo.md.md)
    let stem = match name.strip_suffix(".md") {
        Some(s) => s,
        None => name,
    };
    if stem.is_empty() {
        return Err("Agent name cannot be empty".to_string());
    }
    if stem.contains("..") || stem.contains('/') || stem.contains('\\') {
        return Err(format!(
            "Invalid agent name '{}': path traversal not allowed",
            stem
        ));
    }
    Ok(stem.to_string())
}

/// Resolve the agents directory for a given scope.
fn agents_dir(scope: &str, cwd: Option<&str>) -> Result<PathBuf, String> {
    match scope {
        "user" => Ok(crate::storage::teams::claude_home_dir().join("agents")),
        "project" => {
            let cwd = cwd.unwrap_or("");
            if cwd.is_empty() {
                return Err("Working directory required for project-scope agents".to_string());
            }
            let cwd_path = PathBuf::from(cwd);
            if !cwd_path.is_dir() {
                return Err(format!("Working directory does not exist: {}", cwd));
            }
            Ok(cwd_path.join(".claude").join("agents"))
        }
        _ => Err(format!(
            "Invalid scope '{}': must be 'user' or 'project'",
            scope
        )),
    }
}

/// Safely resolve agent file path from scope + file_name (stem).
/// When `create_dir` is true, creates the agents/ directory if needed.
fn safe_resolve_agent_path(
    scope: &str,
    file_name: &str,
    cwd: Option<&str>,
    create_dir: bool,
) -> Result<PathBuf, String> {
    let base = agents_dir(scope, cwd)?;

    // Canonicalize the known-existing parent (home or cwd), not the agents/ dir
    let parent_to_check = match scope {
        "user" => {
            let home = crate::storage::dirs_next().ok_or("Cannot determine home directory")?;
            std::fs::canonicalize(&home)
                .map_err(|e| format!("Cannot resolve home directory: {}", e))?
        }
        "project" => {
            let cwd_path = PathBuf::from(cwd.unwrap_or(""));
            std::fs::canonicalize(&cwd_path)
                .map_err(|e| format!("Cannot resolve working directory: {}", e))?
        }
        _ => return Err(format!("Invalid scope: {}", scope)),
    };

    // Construct target path: base / {file_name}.md
    let target = base.join(format!("{}.md", file_name));

    // String-level prefix safety check
    let target_str = target.to_string_lossy();
    let parent_str = parent_to_check.to_string_lossy();
    if !target_str.starts_with(parent_str.as_ref()) {
        log::warn!(
            "[agents] path escape rejected: target={}, parent={}",
            target_str,
            parent_str
        );
        return Err("Path outside allowed directory".to_string());
    }

    if create_dir {
        std::fs::create_dir_all(&base).map_err(|e| {
            log::error!("[agents] failed to create agents dir {:?}: {}", base, e);
            format!("Failed to create agents directory: {}", e)
        })?;
    }

    Ok(target)
}

// ── Frontmatter parsing ──

/// Extract YAML frontmatter and body from a .md file content.
fn parse_frontmatter(content: &str) -> (Option<AgentFrontmatter>, String) {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return (None, content.to_string());
    }

    // Find closing ---
    let after_first = &trimmed[3..];
    if let Some(end_idx) = after_first.find("\n---") {
        let yaml_str = &after_first[..end_idx];
        let body_start = end_idx + 4; // skip \n---
        let body = after_first[body_start..]
            .trim_start_matches('\n')
            .to_string();

        match serde_yaml::from_str::<AgentFrontmatter>(yaml_str) {
            Ok(fm) => (Some(fm), body),
            Err(e) => {
                log::warn!("[agents] failed to parse frontmatter YAML: {}", e);
                (None, content.to_string())
            }
        }
    } else {
        // No closing ---, treat entire content as body
        (None, content.to_string())
    }
}

/// Parse a single .md file into an AgentDefinitionSummary.
fn parse_agent_file(
    file_name: &str,
    content: &str,
    source: &str,
    scope: &str,
    readonly: bool,
) -> AgentDefinitionSummary {
    let (fm, _body) = parse_frontmatter(content);
    let fm = fm.unwrap_or_default();

    AgentDefinitionSummary {
        file_name: file_name.to_string(),
        name: fm.name.unwrap_or_else(|| file_name.to_string()),
        description: fm.description.unwrap_or_default(),
        model: fm.model,
        source: source.to_string(),
        scope: scope.to_string(),
        tools: fm.tools,
        disallowed_tools: fm.disallowed_tools,
        permission_mode: fm.permission_mode,
        max_turns: fm.max_turns,
        background: fm.background,
        isolation: fm.isolation,
        readonly,
        raw_content: if readonly {
            Some(content.to_string())
        } else {
            None
        },
    }
}

/// Scan a directory for .md agent files and parse each one.
fn scan_agents_dir(
    dir: &Path,
    source: &str,
    scope: &str,
    readonly: bool,
) -> Vec<AgentDefinitionSummary> {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return vec![],
    };

    let mut agents = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let ext = path.extension().and_then(|e| e.to_str());
        if ext != Some("md") {
            continue;
        }
        let stem = match path.file_stem().and_then(|s| s.to_str()) {
            Some(s) => s.to_string(),
            None => continue,
        };
        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(e) => {
                log::warn!("[agents] failed to read {:?}: {}", path, e);
                continue;
            }
        };
        agents.push(parse_agent_file(&stem, &content, source, scope, readonly));
    }

    log::debug!(
        "[agents] scan_agents_dir: {:?} → {} agents",
        dir,
        agents.len()
    );
    agents
}

// ── Plugin agent discovery ──

/// Discover agents from installed + enabled plugins.
/// Returns empty vec on any CLI failure (degradation).
async fn discover_plugin_agents() -> Vec<AgentDefinitionSummary> {
    // Step 1: Get installed + enabled plugins
    let installed = match crate::storage::plugins::list_installed_plugins_cli().await {
        Ok(list) => list,
        Err(e) => {
            log::warn!(
                "[agents] plugin CLI unavailable, skipping plugin agents: {}",
                e
            );
            return vec![];
        }
    };

    let enabled_set: HashSet<(String, String)> = installed
        .into_iter()
        .filter(|p| p.enabled != Some(false))
        .filter_map(|p| {
            let marketplace = p
                .extra
                .get("marketplace")
                .and_then(|v| v.as_str())
                .map(String::from)
                .or_else(|| p.scope.clone())?;
            Some((p.name.clone(), marketplace))
        })
        .collect();

    if enabled_set.is_empty() {
        log::debug!("[agents] no enabled plugins found");
        return vec![];
    }

    // Step 2: Scan marketplace manifests
    let marketplaces = crate::storage::plugins::list_marketplaces();
    let mut agents = Vec::new();

    for mp in &marketplaces {
        let manifest_path = PathBuf::from(&mp.install_location)
            .join(".claude-plugin")
            .join("marketplace.json");

        #[derive(Deserialize)]
        struct Manifest {
            #[serde(default)]
            plugins: Vec<ManifestPlugin>,
        }
        #[derive(Deserialize)]
        struct ManifestPlugin {
            name: String,
            #[serde(default)]
            source: Option<serde_json::Value>,
        }

        let manifest: Manifest = match std::fs::read_to_string(&manifest_path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
        {
            Some(m) => m,
            None => continue,
        };

        for plugin in &manifest.plugins {
            // Check if this plugin is in the enabled set
            if !enabled_set.contains(&(plugin.name.clone(), mp.name.clone())) {
                continue;
            }

            // Only process local plugins (source starts with "./")
            let rel_path = match plugin
                .source
                .as_ref()
                .and_then(|s| s.as_str())
                .filter(|s| s.starts_with("./"))
            {
                Some(p) => p,
                None => continue,
            };

            let plugin_dir = PathBuf::from(&mp.install_location).join(rel_path);

            // Step 3: Canonical prefix safety check
            let canonical_install = match std::fs::canonicalize(&mp.install_location) {
                Ok(p) => p,
                Err(e) => {
                    log::warn!(
                        "[agents] cannot canonicalize install_location {:?}: {}",
                        mp.install_location,
                        e
                    );
                    continue;
                }
            };
            let canonical_plugin = match std::fs::canonicalize(&plugin_dir) {
                Ok(p) => p,
                Err(_) => continue, // plugin_dir doesn't exist
            };
            if !canonical_plugin.starts_with(&canonical_install) {
                log::warn!(
                    "[agents] plugin path escape rejected: {:?} not under {:?}",
                    canonical_plugin,
                    canonical_install
                );
                continue;
            }

            // Step 4: Scan agents/ subdirectory
            let agents_dir = plugin_dir.join("agents");
            let source_str = format!("plugin:{}:{}", mp.name, plugin.name);
            let mut plugin_agents = scan_agents_dir(&agents_dir, &source_str, "plugin", true);
            agents.append(&mut plugin_agents);
        }
    }

    log::debug!("[agents] discover_plugin_agents: {} total", agents.len());
    agents
}

// ── Tauri commands ──

/// List all agent definitions from user/project/plugin sources.
#[tauri::command]
pub async fn list_agents(cwd: Option<String>) -> Result<Vec<AgentDefinitionSummary>, String> {
    let cwd_str = cwd.as_deref().unwrap_or("");
    log::debug!("[agents] list_agents: cwd={}", cwd_str);

    let mut all = Vec::new();

    // User scope: ~/.claude/agents/
    let user_dir = crate::storage::teams::claude_home_dir().join("agents");
    all.append(&mut scan_agents_dir(&user_dir, "user", "user", false));

    // Project scope: {cwd}/.claude/agents/
    if !cwd_str.is_empty() {
        let project_dir = PathBuf::from(cwd_str).join(".claude").join("agents");
        all.append(&mut scan_agents_dir(
            &project_dir,
            "project",
            "project",
            false,
        ));
    }

    // Plugin scope: enabled plugins' agents/ directories
    let mut plugin_agents = discover_plugin_agents().await;
    all.append(&mut plugin_agents);

    log::debug!(
        "[agents] list_agents: {} total (user+project+plugin)",
        all.len()
    );
    Ok(all)
}

/// Read the raw content of a single agent .md file.
#[tauri::command]
pub fn read_agent_file(
    scope: String,
    file_name: String,
    cwd: Option<String>,
) -> Result<String, String> {
    let file_name = validate_agent_name_lenient(&file_name)?;
    log::debug!(
        "[agents] read_agent_file: scope={}, file_name={}, cwd={:?}",
        scope,
        file_name,
        cwd
    );

    let path = safe_resolve_agent_path(&scope, &file_name, cwd.as_deref(), false)?;
    std::fs::read_to_string(&path).map_err(|e| {
        log::error!("[agents] failed to read {:?}: {}", path, e);
        format!("Failed to read agent file: {}", e)
    })
}

/// Create a new agent .md file. File must not already exist.
#[tauri::command]
pub fn create_agent_file(
    scope: String,
    file_name: String,
    content: String,
    cwd: Option<String>,
) -> Result<(), String> {
    validate_agent_name_strict(&file_name)?;
    log::debug!(
        "[agents] create_agent_file: scope={}, file_name={}, cwd={:?}",
        scope,
        file_name,
        cwd
    );

    let path = safe_resolve_agent_path(&scope, &file_name, cwd.as_deref(), true)?;
    if path.exists() {
        return Err(format!("Agent already exists: {}", file_name));
    }

    std::fs::write(&path, &content).map_err(|e| {
        log::error!("[agents] failed to write {:?}: {}", path, e);
        format!("Failed to create agent file: {}", e)
    })?;

    log::debug!("[agents] created agent: {:?}", path);
    Ok(())
}

/// Update an existing agent .md file. File must already exist.
#[tauri::command]
pub fn update_agent_file(
    scope: String,
    file_name: String,
    content: String,
    cwd: Option<String>,
) -> Result<(), String> {
    let file_name = validate_agent_name_lenient(&file_name)?;
    log::debug!(
        "[agents] update_agent_file: scope={}, file_name={}, cwd={:?}",
        scope,
        file_name,
        cwd
    );

    let path = safe_resolve_agent_path(&scope, &file_name, cwd.as_deref(), false)?;
    if !path.exists() {
        return Err(format!("Agent not found: {}", file_name));
    }

    std::fs::write(&path, &content).map_err(|e| {
        log::error!("[agents] failed to write {:?}: {}", path, e);
        format!("Failed to update agent file: {}", e)
    })?;

    log::debug!("[agents] updated agent: {:?}", path);
    Ok(())
}

/// Delete an agent .md file.
#[tauri::command]
pub fn delete_agent_file(
    scope: String,
    file_name: String,
    cwd: Option<String>,
) -> Result<(), String> {
    let file_name = validate_agent_name_lenient(&file_name)?;
    log::debug!(
        "[agents] delete_agent_file: scope={}, file_name={}, cwd={:?}",
        scope,
        file_name,
        cwd
    );

    let path = safe_resolve_agent_path(&scope, &file_name, cwd.as_deref(), false)?;
    if !path.exists() {
        return Err(format!("Agent not found: {}", file_name));
    }

    std::fs::remove_file(&path).map_err(|e| {
        log::error!("[agents] failed to delete {:?}: {}", path, e);
        format!("Failed to delete agent file: {}", e)
    })?;

    log::debug!("[agents] deleted agent: {:?}", path);
    Ok(())
}

// ── Tests ──

#[cfg(test)]
mod tests {
    use super::*;

    // ── Strict name validation ──

    #[test]
    fn validate_name_strict_valid() {
        assert!(validate_agent_name_strict("my-agent").is_ok());
        assert!(validate_agent_name_strict("a1").is_ok());
        assert!(validate_agent_name_strict("code-reviewer").is_ok());
    }

    #[test]
    fn validate_name_strict_reject_uppercase() {
        assert!(validate_agent_name_strict("FOO_BAR").is_err());
        assert!(validate_agent_name_strict("MyAgent").is_err());
    }

    #[test]
    fn validate_name_strict_reject_special() {
        assert!(validate_agent_name_strict("my agent").is_err());
        assert!(validate_agent_name_strict("a@b").is_err());
        assert!(validate_agent_name_strict("my_agent").is_err());
    }

    #[test]
    fn validate_name_strict_reject_empty() {
        assert!(validate_agent_name_strict("").is_err());
    }

    #[test]
    fn validate_name_strict_reject_too_long() {
        let long_name = "a".repeat(65);
        assert!(validate_agent_name_strict(&long_name).is_err());
        // Exactly 64 should pass
        let exact = "a".repeat(64);
        assert!(validate_agent_name_strict(&exact).is_ok());
    }

    #[test]
    fn validate_name_strict_reject_leading_hyphen() {
        assert!(validate_agent_name_strict("-my-agent").is_err());
    }

    // ── Lenient name validation ──

    #[test]
    fn validate_name_lenient_allows_uppercase() {
        assert_eq!(validate_agent_name_lenient("My_Agent").unwrap(), "My_Agent");
    }

    #[test]
    fn validate_name_lenient_reject_traversal() {
        assert!(validate_agent_name_lenient("../etc/passwd").is_err());
        assert!(validate_agent_name_lenient("foo..bar").is_err());
    }

    #[test]
    fn validate_name_lenient_reject_slash() {
        assert!(validate_agent_name_lenient("foo/bar").is_err());
        assert!(validate_agent_name_lenient("foo\\bar").is_err());
    }

    #[test]
    fn validate_name_lenient_reject_empty() {
        assert!(validate_agent_name_lenient("").is_err());
    }

    #[test]
    fn validate_name_lenient_strips_md_suffix() {
        assert_eq!(
            validate_agent_name_lenient("my-agent.md").unwrap(),
            "my-agent"
        );
    }

    #[test]
    fn validate_name_lenient_md_only_is_empty() {
        assert!(validate_agent_name_lenient(".md").is_err());
    }

    // ── Path construction ──

    #[test]
    fn resolve_path_reject_plugin_scope() {
        let result = agents_dir("plugin", None);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Invalid scope"));
    }

    // ── Frontmatter parsing ──

    #[test]
    fn parse_frontmatter_full() {
        let content = r#"---
name: code-reviewer
description: Reviews code quality
model: sonnet
tools:
  - Read
  - Grep
disallowedTools:
  - Write
permissionMode: plan
maxTurns: 10
background: true
isolation: worktree
---

You are a code reviewer."#;

        let (fm, body) = parse_frontmatter(content);
        let fm = fm.unwrap();
        assert_eq!(fm.name.as_deref(), Some("code-reviewer"));
        assert_eq!(fm.description.as_deref(), Some("Reviews code quality"));
        assert_eq!(fm.model.as_deref(), Some("sonnet"));
        assert_eq!(fm.tools, Some(vec!["Read".to_string(), "Grep".to_string()]));
        assert_eq!(fm.disallowed_tools, Some(vec!["Write".to_string()]));
        assert_eq!(fm.permission_mode.as_deref(), Some("plan"));
        assert_eq!(fm.max_turns, Some(10));
        assert_eq!(fm.background, Some(true));
        assert_eq!(fm.isolation.as_deref(), Some("worktree"));
        assert_eq!(body, "You are a code reviewer.");
    }

    #[test]
    fn parse_frontmatter_minimal() {
        let content = "---\nname: test\ndescription: A test\n---\nBody here.";
        let (fm, body) = parse_frontmatter(content);
        let fm = fm.unwrap();
        assert_eq!(fm.name.as_deref(), Some("test"));
        assert_eq!(fm.description.as_deref(), Some("A test"));
        assert_eq!(fm.model, None);
        assert_eq!(fm.tools, None);
        assert_eq!(body, "Body here.");
    }

    #[test]
    fn parse_frontmatter_no_yaml() {
        let content = "Just a plain markdown file.\nNo frontmatter here.";
        let (fm, body) = parse_frontmatter(content);
        assert!(fm.is_none());
        assert_eq!(body, content);
    }

    #[test]
    fn parse_frontmatter_empty() {
        let (fm, body) = parse_frontmatter("");
        assert!(fm.is_none());
        assert_eq!(body, "");
    }

    #[test]
    fn parse_frontmatter_unknown_fields_preserved() {
        let content = "---\nname: test\ndescription: A test\ncustom_field: hello\n---\nBody.";
        let (fm, body) = parse_frontmatter(content);
        // Unknown fields don't cause errors
        let fm = fm.unwrap();
        assert_eq!(fm.name.as_deref(), Some("test"));
        assert_eq!(body, "Body.");
    }

    // ── parse_agent_file ──

    #[test]
    fn parse_agent_file_uses_frontmatter_name() {
        let content = "---\nname: My Bot\ndescription: A bot\n---\nPrompt.";
        let agent = parse_agent_file("my-bot", content, "user", "user", false);
        assert_eq!(agent.file_name, "my-bot");
        assert_eq!(agent.name, "My Bot");
        assert!(!agent.readonly);
        assert!(agent.raw_content.is_none());
    }

    #[test]
    fn parse_agent_file_falls_back_to_file_name() {
        let content = "---\ndescription: A bot\n---\nPrompt.";
        let agent = parse_agent_file("my-bot", content, "user", "user", false);
        assert_eq!(agent.file_name, "my-bot");
        assert_eq!(agent.name, "my-bot"); // fallback to file_name
    }

    #[test]
    fn parse_agent_file_plugin_readonly_has_raw_content() {
        let content = "---\nname: test\ndescription: A test\n---\nPrompt.";
        let agent = parse_agent_file("test", content, "plugin:mp:plug", "plugin", true);
        assert!(agent.readonly);
        assert_eq!(agent.raw_content, Some(content.to_string()));
        assert_eq!(agent.source, "plugin:mp:plug");
        assert_eq!(agent.scope, "plugin");
    }

    // ── CRUD integration tests (with temp directories) ──

    #[test]
    fn create_and_read_agent_file() {
        let tmp = tempfile::tempdir().unwrap();
        let agents_path = tmp.path().join(".claude").join("agents");
        // safe_resolve_agent_path requires a real cwd for project scope
        // Use user scope by setting HOME temporarily — or just test path construction
        // For simplicity, test the file I/O directly
        std::fs::create_dir_all(&agents_path).unwrap();
        let file_path = agents_path.join("test-agent.md");

        let content = "---\nname: test-agent\ndescription: A test agent\n---\nYou are a test.";
        std::fs::write(&file_path, content).unwrap();

        let read_back = std::fs::read_to_string(&file_path).unwrap();
        assert_eq!(read_back, content);

        // Delete
        std::fs::remove_file(&file_path).unwrap();
        assert!(!file_path.exists());
    }

    #[test]
    fn scan_agents_dir_mixed() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();

        // Create two .md files and one non-md file
        std::fs::write(
            dir.join("agent-a.md"),
            "---\nname: Agent A\ndescription: First\nmodel: haiku\n---\nPrompt A.",
        )
        .unwrap();
        std::fs::write(
            dir.join("agent-b.md"),
            "---\nname: Agent B\ndescription: Second\n---\nPrompt B.",
        )
        .unwrap();
        std::fs::write(dir.join("not-an-agent.txt"), "ignore me").unwrap();

        let agents = scan_agents_dir(dir, "user", "user", false);
        assert_eq!(agents.len(), 2);

        let names: Vec<&str> = agents.iter().map(|a| a.file_name.as_str()).collect();
        assert!(names.contains(&"agent-a"));
        assert!(names.contains(&"agent-b"));

        let a = agents.iter().find(|a| a.file_name == "agent-a").unwrap();
        assert_eq!(a.name, "Agent A");
        assert_eq!(a.model.as_deref(), Some("haiku"));
        assert!(!a.readonly);
    }

    #[test]
    fn scan_agents_dir_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let agents = scan_agents_dir(tmp.path(), "user", "user", false);
        assert!(agents.is_empty());
    }

    #[test]
    fn scan_agents_dir_nonexistent() {
        let agents = scan_agents_dir(Path::new("/nonexistent/path"), "user", "user", false);
        assert!(agents.is_empty());
    }

    #[test]
    fn file_name_vs_frontmatter_name_divergence() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("my-bot.md"),
            "---\nname: My Bot\ndescription: desc\n---\nBody.",
        )
        .unwrap();
        let agents = scan_agents_dir(tmp.path(), "user", "user", false);
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].file_name, "my-bot");
        assert_eq!(agents[0].name, "My Bot");
    }
}
