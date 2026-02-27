use crate::models::{MarketplaceInfo, MarketplacePlugin, PluginComponents, StandaloneSkill};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// ~/.claude/plugins/
fn plugins_dir() -> PathBuf {
    crate::storage::teams::claude_home_dir().join("plugins")
}

/// ~/.claude/skills/
fn skills_dir() -> PathBuf {
    crate::storage::teams::claude_home_dir().join("skills")
}

// ── Internal deserialization types ──

#[derive(Deserialize)]
struct KnownMarketplaceEntry {
    pub source: serde_json::Value,
    #[serde(rename = "installLocation")]
    pub install_location: String,
    #[serde(rename = "lastUpdated")]
    pub last_updated: Option<String>,
}

#[derive(Deserialize)]
struct MarketplaceManifest {
    #[serde(default)]
    pub plugins: Vec<MarketplacePlugin>,
}

#[derive(Deserialize)]
struct InstallCountsCache {
    #[serde(default)]
    pub counts: Vec<InstallCountEntry>,
}

#[derive(Deserialize)]
struct InstallCountEntry {
    pub plugin: String,
    pub unique_installs: u64,
}

/// Generic JSON file reader — returns None on read or parse errors.
fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Option<T> {
    match std::fs::read_to_string(path) {
        Ok(s) => match serde_json::from_str(&s) {
            Ok(v) => Some(v),
            Err(e) => {
                log::warn!("[plugins] parse error {}: {}", path.display(), e);
                None
            }
        },
        Err(e) => {
            log::debug!("[plugins] read error {}: {}", path.display(), e);
            None
        }
    }
}

/// List all registered marketplaces from known_marketplaces.json.
pub fn list_marketplaces() -> Vec<MarketplaceInfo> {
    let known_path = plugins_dir().join("known_marketplaces.json");
    let entries: HashMap<String, KnownMarketplaceEntry> = match read_json(&known_path) {
        Some(v) => v,
        None => return vec![],
    };

    let mut result = Vec::new();
    for (name, entry) in &entries {
        // Read marketplace.json to get plugin count
        let manifest_path = PathBuf::from(&entry.install_location)
            .join(".claude-plugin")
            .join("marketplace.json");
        let plugin_count = read_json::<MarketplaceManifest>(&manifest_path)
            .map(|m| m.plugins.len())
            .unwrap_or(0);

        result.push(MarketplaceInfo {
            name: name.clone(),
            source: entry.source.clone(),
            install_location: entry.install_location.clone(),
            last_updated: entry.last_updated.clone(),
            plugin_count,
        });
    }

    log::debug!(
        "[plugins] list_marketplaces: found {} marketplaces",
        result.len()
    );
    result
}

/// List all plugins across all marketplaces, enriched with install counts and components.
pub fn list_marketplace_plugins() -> Vec<MarketplacePlugin> {
    let marketplaces = list_marketplaces();

    // Load install counts
    let counts_path = plugins_dir().join("install-counts-cache.json");
    let counts_map: HashMap<String, u64> = read_json::<InstallCountsCache>(&counts_path)
        .map(|cache| {
            cache
                .counts
                .into_iter()
                .map(|e| (e.plugin, e.unique_installs))
                .collect()
        })
        .unwrap_or_default();

    let mut all_plugins = Vec::new();

    for mp in &marketplaces {
        let manifest_path = PathBuf::from(&mp.install_location)
            .join(".claude-plugin")
            .join("marketplace.json");
        let manifest: MarketplaceManifest = match read_json(&manifest_path) {
            Some(m) => m,
            None => continue,
        };

        for mut plugin in manifest.plugins {
            plugin.marketplace_name = Some(mp.name.clone());

            // Enrich with install count
            let count_key = format!("{}@{}", plugin.name, mp.name);
            plugin.install_count = counts_map.get(&count_key).copied();

            // Discover components for local plugins (source is a string starting with "./")
            let is_local = plugin
                .source
                .as_ref()
                .and_then(|s| s.as_str())
                .map(|s| s.starts_with("./"))
                .unwrap_or(false);

            if is_local {
                if let Some(rel_path) = plugin.source.as_ref().and_then(|s| s.as_str()) {
                    let plugin_dir = PathBuf::from(&mp.install_location).join(rel_path);
                    plugin.components =
                        discover_plugin_components(&plugin_dir, &plugin.lsp_servers);
                }
            }
            // else: external plugin, keep default PluginComponents

            all_plugins.push(plugin);
        }
    }

    // Sort by install_count descending (plugins without counts go to end)
    all_plugins.sort_by(|a, b| {
        let a_count = a.install_count.unwrap_or(0);
        let b_count = b.install_count.unwrap_or(0);
        b_count.cmp(&a_count)
    });

    log::debug!(
        "[plugins] list_marketplace_plugins: {} plugins across {} marketplaces",
        all_plugins.len(),
        marketplaces.len()
    );
    all_plugins
}

/// Scan a plugin directory for its components (skills, commands, agents, hooks, mcp, lsp).
fn discover_plugin_components(
    plugin_dir: &Path,
    lsp_servers_json: &Option<serde_json::Value>,
) -> PluginComponents {
    let skills = list_subdir_names(&plugin_dir.join("skills"));
    let commands = list_md_stems(&plugin_dir.join("commands"));
    let agents = list_md_stems(&plugin_dir.join("agents"));
    let hooks = plugin_dir.join("hooks").is_dir() || plugin_dir.join("hooks.json").is_file();

    let mcp_servers = if let Some(mcp) =
        read_json::<serde_json::Map<String, serde_json::Value>>(&plugin_dir.join(".mcp.json"))
    {
        mcp.keys().cloned().collect()
    } else {
        vec![]
    };

    let lsp_servers = match lsp_servers_json {
        Some(serde_json::Value::Object(map)) => map.keys().cloned().collect(),
        _ => vec![],
    };

    log::trace!(
        "[plugins] discover_components: {:?} → skills={}, cmds={}, agents={}",
        plugin_dir,
        skills.len(),
        commands.len(),
        agents.len()
    );

    PluginComponents {
        skills,
        commands,
        agents,
        hooks,
        mcp_servers,
        lsp_servers,
    }
}

/// List subdirectory names within a directory (for skills — each subdir has a SKILL.md).
fn list_subdir_names(dir: &Path) -> Vec<String> {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return vec![],
    };
    entries
        .flatten()
        .filter(|e| e.path().is_dir())
        .filter_map(|e| e.file_name().to_str().map(String::from))
        .collect()
}

/// List .md file stems within a directory (for commands, agents).
fn list_md_stems(dir: &Path) -> Vec<String> {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return vec![],
    };
    entries
        .flatten()
        .filter_map(|e| {
            let path = e.path();
            if path.is_file() && path.extension().map(|ext| ext == "md").unwrap_or(false) {
                path.file_stem().and_then(|s| s.to_str()).map(String::from)
            } else {
                None
            }
        })
        .collect()
}

/// List standalone skills from ~/.claude/skills/*/SKILL.md
/// and optionally from {cwd}/.claude/skills/*/SKILL.md.
pub fn list_standalone_skills(cwd: &str) -> Vec<StandaloneSkill> {
    let mut skills = Vec::new();

    // User-scope skills (~/.claude/skills/)
    scan_skills_dir(&skills_dir(), "user", &mut skills);

    // Project-scope skills ({cwd}/.claude/skills/)
    if !cwd.is_empty() {
        let project_dir = PathBuf::from(cwd).join(".claude").join("skills");
        scan_skills_dir(&project_dir, "project", &mut skills);
    }

    log::debug!(
        "[plugins] list_standalone_skills: found {} skills",
        skills.len()
    );
    skills
}

/// Scan a directory for skills and append to the result vector.
fn scan_skills_dir(dir: &Path, scope: &str, skills: &mut Vec<StandaloneSkill>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) => {
            log::debug!("[plugins] cannot read skills dir {}: {}", dir.display(), e);
            return;
        }
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let skill_md = path.join("SKILL.md");
        if !skill_md.is_file() {
            continue;
        }

        let (name, description) = parse_skill_frontmatter(&skill_md);
        let name = if name.is_empty() {
            entry.file_name().to_str().unwrap_or("unknown").to_string()
        } else {
            name
        };

        skills.push(StandaloneSkill {
            name,
            description,
            path: skill_md.to_string_lossy().to_string(),
            scope: scope.to_string(),
        });
    }
}

/// Parse YAML frontmatter from a SKILL.md file.
/// Extracts `name` and `description` from between `---` delimiters.
fn parse_skill_frontmatter(path: &Path) -> (String, String) {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return (String::new(), String::new()),
    };

    // Only look at first 1KB (find safe UTF-8 char boundary to avoid panic)
    let head: &str = if content.len() > 1024 {
        let mut end = 1024;
        while !content.is_char_boundary(end) {
            end -= 1;
        }
        &content[..end]
    } else {
        &content
    };

    // Find frontmatter between --- delimiters
    if !head.starts_with("---") {
        return (String::new(), String::new());
    }

    let after_first = &head[3..];
    let end_pos = match after_first.find("---") {
        Some(p) => p,
        None => return (String::new(), String::new()),
    };

    let frontmatter = &after_first[..end_pos];
    let mut name = String::new();
    let mut description = String::new();

    for line in frontmatter.lines() {
        let line = line.trim();
        if let Some(val) = line.strip_prefix("name:") {
            name = val.trim().trim_matches('"').to_string();
        } else if let Some(val) = line.strip_prefix("description:") {
            description = val.trim().trim_matches('"').to_string();
        }
    }

    (name, description)
}

/// Read skill content with path validation (security: prevent arbitrary file reads).
/// Validates against ~/.claude/skills/, ~/.claude/plugins/, and optionally {cwd}/.claude/skills/.
pub fn read_skill_content(path: &str, cwd: &str) -> Result<String, String> {
    log::debug!("[plugins] read_skill_content: path={}, cwd={}", path, cwd);

    let canonical = validate_skill_path(path, cwd)?;

    std::fs::read_to_string(&canonical).map_err(|e| format!("Failed to read file: {}", e))
}

// ── CLI plugin command execution ──

use crate::agent::claude_stream::{augmented_path, resolve_claude_path};
use tokio::process::Command;
use tokio::time::{timeout, Duration};

const PLUGIN_CMD_TIMEOUT: Duration = Duration::from_secs(30);

/// Result of a CLI plugin command execution.
#[derive(Debug)]
pub struct PluginCommandResult {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
}

/// Run a `claude plugin ...` CLI command and capture output.
///
/// `args` is the argument list after `plugin` — e.g., `["install", "frontend-design", "--scope", "user"]`.
///
/// Returns `PluginCommandResult` with stdout, stderr, exit_code, and success flag.
/// Returns `Err(String)` only for spawn failures or timeouts (not CLI errors — those are in stderr).
pub async fn run_plugin_command(args: &[&str]) -> Result<PluginCommandResult, String> {
    let claude_bin = resolve_claude_path();
    let path_env = augmented_path();

    log::debug!(
        "[plugins] run_plugin_command: {} plugin {}",
        claude_bin,
        args.join(" ")
    );

    let mut cmd = Command::new(&claude_bin);
    cmd.arg("plugin");
    for arg in args {
        cmd.arg(arg);
    }
    cmd.env("PATH", &path_env)
        .env_remove("CLAUDECODE") // Allow running inside a Claude Code session
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    let child = cmd.spawn().map_err(|e| {
        log::error!("[plugins] failed to spawn claude: {}", e);
        format!("Failed to spawn claude: {}", e)
    })?;

    let result = timeout(PLUGIN_CMD_TIMEOUT, child.wait_with_output()).await;

    match result {
        Ok(Ok(output)) => {
            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            let exit_code = output.status.code();
            let success = output.status.success();

            log::debug!(
                "[plugins] command completed: success={}, exit_code={:?}, stdout_len={}, stderr_len={}",
                success, exit_code, stdout.len(), stderr.len()
            );
            if !success {
                log::debug!("[plugins] stderr: {}", &stderr[..stderr.len().min(500)]);
            }

            Ok(PluginCommandResult {
                success,
                stdout,
                stderr,
                exit_code,
            })
        }
        Ok(Err(e)) => {
            log::error!("[plugins] process error: {}", e);
            Err(format!("Process error: {}", e))
        }
        Err(_) => {
            log::error!(
                "[plugins] command timed out after {}s",
                PLUGIN_CMD_TIMEOUT.as_secs()
            );
            Err(format!(
                "Command timed out after {}s",
                PLUGIN_CMD_TIMEOUT.as_secs()
            ))
        }
    }
}

/// Validate a plugin name (alphanumeric, hyphens, optional @marketplace suffix).
/// Examples: "frontend-design", "github@claude-plugins-official"
pub fn validate_plugin_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("Plugin name cannot be empty".to_string());
    }
    if name.len() > 256 {
        return Err("Plugin name too long".to_string());
    }
    // Allow: alphanumeric, hyphens, underscores, dots, @, /
    // Disallow: spaces, semicolons, backticks, pipes, etc.
    let valid = name
        .chars()
        .all(|c| c.is_alphanumeric() || c == '-' || c == '_' || c == '.' || c == '@' || c == '/');
    if !valid {
        return Err(format!("Invalid characters in plugin name: {}", name));
    }
    Ok(())
}

/// Validate a standalone skill name.
/// Only alphanumeric characters, hyphens, and underscores allowed.
/// No dots, slashes, @, or spaces — the name becomes a directory name.
pub fn validate_skill_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("Skill name cannot be empty".to_string());
    }
    if name.len() > 128 {
        return Err("Skill name too long (max 128 characters)".to_string());
    }
    let valid = name
        .chars()
        .all(|c| c.is_alphanumeric() || c == '-' || c == '_');
    if !valid {
        return Err(format!(
            "Invalid skill name '{}': only letters, numbers, hyphens, and underscores allowed",
            name
        ));
    }
    // Defense-in-depth: prevent traversal patterns (already blocked by char validation)
    if name == "." || name == ".." || name.contains("..") {
        return Err("Invalid skill name: directory traversal not allowed".to_string());
    }
    Ok(())
}

/// Resolve the skills base directory for a given scope.
/// - "user" -> ~/.claude/skills/
/// - "project" -> {cwd}/.claude/skills/
fn resolve_skill_dir(scope: &str, cwd: &str) -> Result<PathBuf, String> {
    match scope {
        "user" => Ok(skills_dir()),
        "project" => {
            if cwd.is_empty() {
                return Err("Working directory required for project-scope skills".to_string());
            }
            let cwd_path = PathBuf::from(cwd);
            if !cwd_path.is_dir() {
                return Err(format!("Working directory does not exist: {}", cwd));
            }
            Ok(cwd_path.join(".claude").join("skills"))
        }
        _ => Err(format!(
            "Invalid scope '{}': must be 'user' or 'project'",
            scope
        )),
    }
}

/// Validate that a skill path is within allowed directories.
/// Allowed: ~/.claude/skills/, ~/.claude/plugins/, or {cwd}/.claude/skills/.
/// Returns the canonicalized path.
fn validate_skill_path(path: &str, cwd: &str) -> Result<PathBuf, String> {
    let requested = PathBuf::from(path);
    let canonical =
        std::fs::canonicalize(&requested).map_err(|e| format!("Cannot resolve path: {}", e))?;

    let home = crate::storage::teams::claude_home_dir();
    let allowed_skills = match std::fs::canonicalize(home.join("skills")) {
        Ok(p) => p,
        Err(_) => home.join("skills"),
    };
    let allowed_plugins = match std::fs::canonicalize(home.join("plugins")) {
        Ok(p) => p,
        Err(_) => home.join("plugins"),
    };

    // Check user-scope dirs
    if canonical.starts_with(&allowed_skills) || canonical.starts_with(&allowed_plugins) {
        return Ok(canonical);
    }

    // Check project-scope dir
    if !cwd.is_empty() {
        let project_skills = PathBuf::from(cwd).join(".claude").join("skills");
        if let Ok(project_canonical) = std::fs::canonicalize(&project_skills) {
            if canonical.starts_with(&project_canonical) {
                return Ok(canonical);
            }
        }
    }

    Err("Access denied: path is outside allowed skill directories".to_string())
}

/// Create a new standalone skill.
/// Creates {scope_dir}/skills/{name}/SKILL.md with YAML frontmatter.
pub fn create_skill(
    name: &str,
    description: &str,
    content: &str,
    scope: &str,
    cwd: &str,
) -> Result<StandaloneSkill, String> {
    validate_skill_name(name)?;

    let base_dir = resolve_skill_dir(scope, cwd)?;
    let skill_dir = base_dir.join(name);

    if skill_dir.exists() {
        return Err(format!(
            "Skill '{}' already exists in {} scope",
            name, scope
        ));
    }

    // Build SKILL.md content with frontmatter (quote description for YAML safety)
    let full_content = format!(
        "---\nname: \"{}\"\ndescription: \"{}\"\n---\n\n{}",
        name, description, content
    );

    // Create directory and write file
    std::fs::create_dir_all(&skill_dir)
        .map_err(|e| format!("Failed to create skill directory: {}", e))?;

    let skill_md = skill_dir.join("SKILL.md");
    std::fs::write(&skill_md, &full_content)
        .map_err(|e| format!("Failed to write SKILL.md: {}", e))?;

    log::debug!(
        "[plugins] create_skill: name={}, scope={}, path={}",
        name,
        scope,
        skill_md.display()
    );

    Ok(StandaloneSkill {
        name: name.to_string(),
        description: description.to_string(),
        path: skill_md.to_string_lossy().to_string(),
        scope: scope.to_string(),
    })
}

/// Update the content of an existing skill's SKILL.md file.
/// Path must be within allowed skill directories.
pub fn update_skill_content(path: &str, content: &str, cwd: &str) -> Result<(), String> {
    let canonical = validate_skill_path(path, cwd)?;

    // Verify it's a SKILL.md file
    if canonical.file_name().and_then(|n| n.to_str()) != Some("SKILL.md") {
        return Err("Can only update SKILL.md files".to_string());
    }

    std::fs::write(&canonical, content).map_err(|e| format!("Failed to write skill: {}", e))?;

    log::debug!(
        "[plugins] update_skill_content: path={}, content_len={}",
        path,
        content.len()
    );

    Ok(())
}

/// Delete a standalone skill by removing its entire directory.
/// `path` should point to the SKILL.md file; the parent directory is removed.
pub fn delete_skill(path: &str, cwd: &str) -> Result<(), String> {
    let canonical = validate_skill_path(path, cwd)?;

    // Verify it's a SKILL.md file
    if canonical.file_name().and_then(|n| n.to_str()) != Some("SKILL.md") {
        return Err("Can only delete SKILL.md skills".to_string());
    }

    // Remove the parent directory (e.g., ~/.claude/skills/my-skill/)
    let skill_dir = canonical
        .parent()
        .ok_or_else(|| "Cannot determine skill directory".to_string())?;

    std::fs::remove_dir_all(skill_dir).map_err(|e| format!("Failed to delete skill: {}", e))?;

    log::debug!(
        "[plugins] delete_skill: path={}, dir={}",
        path,
        skill_dir.display()
    );

    Ok(())
}

/// Validate a marketplace source (URL, path, or GitHub owner/repo).
/// Examples: "https://github.com/user/repo.git", "owner/repo", "/path/to/marketplace"
pub fn validate_marketplace_source(source: &str) -> Result<(), String> {
    if source.is_empty() {
        return Err("Marketplace source cannot be empty".to_string());
    }
    if source.len() > 1024 {
        return Err("Marketplace source too long".to_string());
    }
    // Disallow shell metacharacters
    let dangerous = [
        ';', '|', '&', '`', '$', '(', ')', '{', '}', '<', '>', '\n', '\r',
    ];
    for c in &dangerous {
        if source.contains(*c) {
            return Err(format!("Invalid character '{}' in marketplace source", c));
        }
    }
    Ok(())
}

/// Validate scope parameter.
pub fn validate_scope(scope: &str) -> Result<(), String> {
    match scope {
        "user" | "project" | "local" | "managed" => Ok(()),
        _ => Err(format!(
            "Invalid scope '{}': must be user, project, local, or managed",
            scope
        )),
    }
}

/// List installed plugins via CLI.
pub async fn list_installed_plugins_cli() -> Result<Vec<crate::models::InstalledPlugin>, String> {
    let result = run_plugin_command(&["list", "--json"]).await?;
    if !result.success {
        return Err(format!("CLI error: {}", result.stderr.trim()));
    }

    let plugins: Vec<crate::models::InstalledPlugin> = serde_json::from_str(result.stdout.trim())
        .map_err(|e| {
        log::warn!("[plugins] failed to parse installed plugins JSON: {}", e);
        format!("Failed to parse plugin list: {}", e)
    })?;

    log::debug!(
        "[plugins] list_installed_plugins_cli: {} plugins",
        plugins.len()
    );
    Ok(plugins)
}
