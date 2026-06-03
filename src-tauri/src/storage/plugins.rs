use crate::models::{
    InstalledPlugin, MarketplaceInfo, MarketplacePlugin, PluginComponents, SkillDisabledBy,
    SkillSourceKind, StandaloneSkill,
};
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
    let mut commands = Vec::new();
    visit_md_stems(&plugin_dir.join("commands"), "", 0, &mut |name, _| {
        commands.push(name)
    });
    let mut agents = Vec::new();
    visit_md_stems(&plugin_dir.join("agents"), "", 0, &mut |name, _| {
        agents.push(name)
    });
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
        // Use file_type to avoid following symlinks into cycles (matches
        // visit_md_stems).
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .filter_map(|e| e.file_name().to_str().map(String::from))
        .collect()
}

/// List .md command/agent entries within a directory, recursively scanning
/// subdirectories. Returns `(command_name, file_path)` pairs where
/// `command_name` uses colons to separate nested subdirectories
/// (e.g. `.claude/commands/opsx/apply.md` → `("opsx:apply", <path>)`).
fn list_md_stems(dir: &Path) -> Vec<(String, PathBuf)> {
    let mut stems = Vec::new();
    visit_md_stems(dir, "", 0, &mut |name, path| stems.push((name, path)));
    stems
}

/// Maximum directory nesting depth for command discovery. Guards against
/// pathological structures or symlink cycles.
const MAX_COMMAND_DEPTH: usize = 8;

/// Recursively walk `.md` files and invoke `visit(command_name, file_path)` for each.
/// Lets callers that don't need the path skip the per-entry PathBuf allocation.
fn visit_md_stems(dir: &Path, prefix: &str, depth: usize, visit: &mut dyn FnMut(String, PathBuf)) {
    if depth > MAX_COMMAND_DEPTH {
        return;
    }
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        // Resolve metadata via the path (which follows symlinks) rather than
        // entry.file_type() or entry.metadata() (both report symlink-itself).
        // Users commonly symlink shared .md files or whole subdirectories from
        // a dotfiles repo into ~/.claude/commands/ — those must surface in the
        // slash menu. Cycles are bounded by MAX_COMMAND_DEPTH.
        let metadata = match std::fs::metadata(&path) {
            Ok(m) => m,
            Err(_) => continue, // broken symlink or unreadable — skip silently
        };
        if metadata.is_dir() {
            if let Some(dir_name) = path.file_name().and_then(|n| n.to_str()) {
                let new_prefix = if prefix.is_empty() {
                    dir_name.to_string()
                } else {
                    format!("{}:{}", prefix, dir_name)
                };
                visit_md_stems(&path, &new_prefix, depth + 1, visit);
            }
        } else if metadata.is_file() && path.extension().map(|ext| ext == "md").unwrap_or(false) {
            if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                let command_name = if prefix.is_empty() {
                    stem.to_string()
                } else {
                    format!("{}:{}", prefix, stem)
                };
                visit(command_name, path);
            }
        }
    }
}

/// List project-level commands from ~/.claude/commands/ and {cwd}/.claude/commands/.
/// Scans flat .md files, parses frontmatter for name/description, falls back to file stem.
pub fn list_project_commands(cwd: &str) -> Vec<crate::models::CliCommand> {
    let mut commands = Vec::new();
    let mut seen = std::collections::HashSet::new();

    // Project-scope commands first (higher priority)
    if !cwd.is_empty() {
        let project_dir = PathBuf::from(cwd).join(".claude").join("commands");
        scan_commands_dir(&project_dir, &mut commands, &mut seen);
    }

    // User-scope commands
    let user_dir = crate::storage::teams::claude_home_dir().join("commands");
    scan_commands_dir(&user_dir, &mut commands, &mut seen);

    log::debug!(
        "[plugins] list_project_commands: found {} commands",
        commands.len()
    );
    commands
}

/// Scan a directory for .md command files and append to the result vector.
fn scan_commands_dir(
    dir: &Path,
    commands: &mut Vec<crate::models::CliCommand>,
    seen: &mut std::collections::HashSet<String>,
) {
    let stems = list_md_stems(dir);
    for (stem, md_path) in stems {
        if seen.contains(&stem) {
            continue; // project-scope already added this name
        }
        // Invocation name is path-derived (e.g. `opsx:apply` for opsx/apply.md).
        // Frontmatter `name` is only a display label in Claude Code and is
        // restricted to [a-z0-9-], so it cannot encode the nested invocation
        // form CLI actually expects. Pull only `description` for the menu hint.
        let (_display_name, description) = parse_skill_frontmatter(&md_path);
        commands.push(crate::models::CliCommand {
            name: stem.clone(),
            description,
            aliases: vec![],
            extra: std::collections::HashMap::new(),
        });
        seen.insert(stem);
    }
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
            ..Default::default()
        });
    }
}

/// Parse YAML frontmatter from a SKILL.md file.
/// Extracts `name` and `description` from between `---` delimiters.
fn parse_skill_frontmatter(path: &Path) -> (String, String) {
    use std::io::Read;
    // Read at most 1KB — command/agent .md bodies can be tens of KB and we
    // never look past the frontmatter delimiter.
    let mut buf = Vec::with_capacity(1024);
    let read_ok = std::fs::File::open(path)
        .and_then(|f| f.take(1024).read_to_end(&mut buf))
        .is_ok();
    if !read_ok {
        return (String::new(), String::new());
    }
    // Truncate at the last UTF-8 char boundary so str::from_utf8 cannot panic.
    let mut end = buf.len();
    while end > 0 && std::str::from_utf8(&buf[..end]).is_err() {
        end -= 1;
    }
    let head = match std::str::from_utf8(&buf[..end]) {
        Ok(s) => s,
        Err(_) => return (String::new(), String::new()),
    };

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
use crate::process_ext::HideConsole;
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
/// `cwd` sets the working directory for the command (required for `--scope project`/`local`).
///
/// Returns `PluginCommandResult` with stdout, stderr, exit_code, and success flag.
/// Returns `Err(String)` only for spawn failures or timeouts (not CLI errors — those are in stderr).
pub async fn run_plugin_command(
    args: &[&str],
    cwd: Option<&str>,
) -> Result<PluginCommandResult, String> {
    let claude_bin = resolve_claude_path();
    let path_env = augmented_path();

    log::debug!(
        "[plugins] run_plugin_command: {} plugin {} (cwd={:?})",
        claude_bin,
        args.join(" "),
        cwd
    );

    let mut cmd = Command::new(&claude_bin);
    cmd.arg("plugin");
    for arg in args {
        cmd.arg(arg);
    }
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    cmd.env("PATH", &path_env)
        .env_remove("CLAUDECODE") // Allow running inside a Claude Code session
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    cmd.hide_console().kill_on_drop(true);
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
        ..Default::default()
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
    let result = run_plugin_command(&["list", "--json"], None).await?;
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

// ══════════════════════════════════════════════════════════════════════
// Codex Skills support
// ══════════════════════════════════════════════════════════════════════

const MAX_SCAN_DEPTH: usize = 6;
const MAX_DIRS_PER_ROOT: usize = 2000;

/// $HOME/.agents/skills/
fn codex_user_skills_dir() -> PathBuf {
    crate::storage::home_dir()
        .map(|h| PathBuf::from(h).join(".agents").join("skills"))
        .unwrap_or_default()
}

/// $CODEX_HOME/skills/
fn codex_legacy_skills_dir() -> Result<PathBuf, String> {
    crate::storage::cli_config::codex_home_dir().map(|d| d.join("skills"))
}

/// $CODEX_HOME/skills/.system/
fn codex_bundled_skills_dir() -> Result<PathBuf, String> {
    crate::storage::cli_config::codex_home_dir().map(|d| d.join("skills").join(".system"))
}

/// Find git root from `cwd` upward, then return all ancestor dirs from root to cwd (inclusive).
fn project_layers(cwd: &str) -> Vec<PathBuf> {
    let cwd_path = match std::fs::canonicalize(cwd) {
        Ok(p) => p,
        Err(_) => return vec![],
    };

    // Walk up to find .git
    let mut probe = cwd_path.clone();
    let git_root = loop {
        if probe.join(".git").exists() {
            break Some(probe.clone());
        }
        if !probe.pop() {
            break None;
        }
    };

    let root = match git_root {
        Some(r) => r,
        None => return vec![cwd_path],
    };

    // Build layers from root down to cwd
    let mut layers = Vec::new();
    layers.push(root.clone());

    if let Ok(suffix) = cwd_path.strip_prefix(&root) {
        let mut current = root;
        for component in suffix.components() {
            current = current.join(component);
            layers.push(current.clone());
        }
    }
    // Dedup in case root == cwd
    layers.dedup();
    layers
}

/// Recursive directory walker collecting SKILL.md files.
#[allow(clippy::too_many_arguments)]
fn walk_codex_skills(
    dir: &Path,
    depth: usize,
    max_depth: usize,
    follow_symlinks: bool,
    visited: &mut std::collections::HashSet<PathBuf>,
    dir_count: &mut usize,
    max_dirs: usize,
    found: &mut Vec<(PathBuf, String, String)>,
) {
    if depth > max_depth || *dir_count >= max_dirs {
        return;
    }

    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let file_name = entry.file_name();
        let name_str = file_name.to_string_lossy();

        // Skip dot-entries
        if name_str.starts_with('.') {
            continue;
        }

        let path = entry.path();

        // Check if it's a symlink
        let is_symlink = entry.file_type().map(|ft| ft.is_symlink()).unwrap_or(false);

        if is_symlink && !follow_symlinks {
            continue;
        }

        // Check if it's a SKILL.md file
        if name_str == "SKILL.md" && path.is_file() {
            let (name, description) = parse_skill_frontmatter(&path);
            found.push((path, name, description));
            continue;
        }

        // Recurse into directories
        if path.is_dir() {
            // Prevent symlink loops via canonical path
            if is_symlink || follow_symlinks {
                if let Ok(canonical) = std::fs::canonicalize(&path) {
                    if !visited.insert(canonical) {
                        continue; // already visited
                    }
                }
            }

            *dir_count += 1;
            walk_codex_skills(
                &path,
                depth + 1,
                max_depth,
                follow_symlinks,
                visited,
                dir_count,
                max_dirs,
                found,
            );
        }
    }
}

/// Scan a single root directory for Codex SKILL.md files.
#[cfg_attr(not(test), allow(dead_code))]
fn scan_codex_skills_dir(
    dir: &Path,
    scope: &str,
    source_kind: SkillSourceKind,
    follow_symlinks: bool,
    skills: &mut Vec<StandaloneSkill>,
) {
    scan_codex_skills_dir_with_roots(dir, scope, source_kind, follow_symlinks, skills, &[], None);
}

/// Inner scanner with explicit allowed_roots and bundled_dir for testability.
fn scan_codex_skills_dir_with_roots(
    dir: &Path,
    scope: &str,
    source_kind: SkillSourceKind,
    follow_symlinks: bool,
    skills: &mut Vec<StandaloneSkill>,
    allowed_roots: &[PathBuf],
    bundled_dir: Option<&Path>,
) {
    if !dir.is_dir() {
        return;
    }

    let mut visited = std::collections::HashSet::new();
    let mut dir_count = 0usize;
    let mut found = Vec::new();

    walk_codex_skills(
        dir,
        0,
        MAX_SCAN_DEPTH,
        follow_symlinks,
        &mut visited,
        &mut dir_count,
        MAX_DIRS_PER_ROOT,
        &mut found,
    );

    let skills_root_canonical = std::fs::canonicalize(dir).ok();

    for (skill_md_path, name, description) in found {
        let display_name = if name.is_empty() {
            // Use parent directory name as fallback
            skill_md_path
                .parent()
                .and_then(|p| p.file_name())
                .and_then(|n| n.to_str())
                .unwrap_or("unknown")
                .to_string()
        } else {
            name
        };

        let is_bundled = source_kind == SkillSourceKind::Bundled;

        // can_delete logic
        let can_delete = if is_bundled {
            false
        } else if let Some(ref root_canon) = skills_root_canonical {
            // Root-level SKILL.md (parent == skills root) → false
            let parent = skill_md_path.parent();
            let parent_canonical = parent.and_then(|p| std::fs::canonicalize(p).ok());
            if parent_canonical.as_ref() == Some(root_canon) {
                false
            } else {
                // Canonical path outside allowed roots → false
                let canonical = std::fs::canonicalize(&skill_md_path).ok();
                if let Some(ref can_path) = canonical {
                    if !allowed_roots.is_empty() {
                        allowed_roots.iter().any(|r| can_path.starts_with(r))
                    } else {
                        can_path.starts_with(root_canon)
                    }
                } else {
                    false
                }
            }
        } else {
            false
        };

        // can_edit = false for v1 (no update_codex_skill)
        let can_edit = false;

        // can_toggle: path exists + non-system + belongs to known root + symlink in-root
        let can_toggle = if is_bundled {
            false
        } else {
            skill_md_path.exists()
                && skills_root_canonical.as_ref().is_some_and(|root| {
                    std::fs::canonicalize(&skill_md_path)
                        .ok()
                        .is_some_and(|cp| {
                            // Check it's not inside bundled dir
                            if let Some(bd) = bundled_dir {
                                if let Ok(bd_canon) = std::fs::canonicalize(bd) {
                                    if cp.starts_with(&bd_canon) {
                                        return false;
                                    }
                                }
                            }
                            cp.starts_with(root) || allowed_roots.iter().any(|r| cp.starts_with(r))
                        })
                })
        };

        skills.push(StandaloneSkill {
            name: display_name,
            description,
            path: skill_md_path.to_string_lossy().to_string(),
            scope: scope.to_string(),
            agent: "codex".to_string(),
            source_kind: Some(source_kind),
            enabled: true,
            disabled_by: None,
            can_edit,
            can_delete,
            can_toggle,
        });
    }
}

/// List all Codex skills following the upstream scan order.
pub fn list_codex_skills(cwd: Option<&str>) -> Vec<StandaloneSkill> {
    list_codex_skills_with_overrides(cwd, None, None, None, None)
}

/// Inner implementation with injectable paths for testing.
fn list_codex_skills_with_overrides(
    cwd: Option<&str>,
    user_dir_override: Option<&Path>,
    legacy_dir_override: Option<&Path>,
    bundled_dir_override: Option<&Path>,
    codex_config_override: Option<&Path>,
) -> Vec<StandaloneSkill> {
    let mut skills = Vec::new();
    let mut seen_canonical = std::collections::HashSet::new();
    let mut allowed_roots = Vec::new();

    let user_dir = user_dir_override
        .map(PathBuf::from)
        .unwrap_or_else(codex_user_skills_dir);
    let legacy_dir = legacy_dir_override
        .map(PathBuf::from)
        .unwrap_or_else(|| codex_legacy_skills_dir().unwrap_or_default());
    let bundled_dir = bundled_dir_override
        .map(PathBuf::from)
        .unwrap_or_else(|| codex_bundled_skills_dir().unwrap_or_default());

    // Collect allowed roots for can_delete checks
    if user_dir.is_dir() {
        if let Ok(c) = std::fs::canonicalize(&user_dir) {
            allowed_roots.push(c);
        }
    }
    if legacy_dir.is_dir() {
        if let Ok(c) = std::fs::canonicalize(&legacy_dir) {
            allowed_roots.push(c);
        }
    }

    // 1-2. Project roots (only if cwd is provided)
    if let Some(cwd_str) = cwd {
        if !cwd_str.is_empty() {
            let layers = project_layers(cwd_str);
            for layer in &layers {
                // .codex/skills/ → ProjectCodex
                let codex_skills = layer.join(".codex").join("skills");
                if codex_skills.is_dir() {
                    if let Ok(c) = std::fs::canonicalize(&codex_skills) {
                        allowed_roots.push(c);
                    }
                }
                scan_codex_skills_dir_with_roots(
                    &codex_skills,
                    "project",
                    SkillSourceKind::ProjectCodex,
                    true,
                    &mut skills,
                    &allowed_roots,
                    Some(&bundled_dir),
                );

                // .agents/skills/ → ProjectAgents
                let agents_skills = layer.join(".agents").join("skills");
                if agents_skills.is_dir() {
                    if let Ok(c) = std::fs::canonicalize(&agents_skills) {
                        allowed_roots.push(c);
                    }
                }
                scan_codex_skills_dir_with_roots(
                    &agents_skills,
                    "project",
                    SkillSourceKind::ProjectAgents,
                    true,
                    &mut skills,
                    &allowed_roots,
                    Some(&bundled_dir),
                );
            }
        }
    }

    // 3. $HOME/.agents/skills/ → user, User
    scan_codex_skills_dir_with_roots(
        &user_dir,
        "user",
        SkillSourceKind::User,
        true,
        &mut skills,
        &allowed_roots,
        Some(&bundled_dir),
    );

    // 4. $CODEX_HOME/skills/ (excluding .system/) → user, Legacy
    if legacy_dir.is_dir() {
        // We need to scan legacy but exclude .system which is already handled by
        // walk_codex_skills skipping dot-entries. So scanning legacy_dir is fine.
        scan_codex_skills_dir_with_roots(
            &legacy_dir,
            "user",
            SkillSourceKind::Legacy,
            false,
            &mut skills,
            &allowed_roots,
            Some(&bundled_dir),
        );
    }

    // 5. $CODEX_HOME/skills/.system/ → system, Bundled
    scan_codex_skills_dir_with_roots(
        &bundled_dir,
        "system",
        SkillSourceKind::Bundled,
        false,
        &mut skills,
        &allowed_roots,
        Some(&bundled_dir),
    );

    // 6. Dedup by canonical SKILL.md path (keep first occurrence)
    skills.retain(|s| {
        let p = PathBuf::from(&s.path);
        let key = std::fs::canonicalize(&p).unwrap_or(p);
        seen_canonical.insert(key)
    });

    // 7. Apply config.toml [skills] rules
    let config_path = codex_config_override
        .map(PathBuf::from)
        .unwrap_or_else(|| crate::storage::cli_config::codex_config_path().unwrap_or_default());
    apply_codex_skill_config(&config_path, &mut skills);

    log::debug!(
        "[plugins] list_codex_skills: found {} skills (cwd={:?})",
        skills.len(),
        cwd
    );
    skills
}

/// Configuration rule from [[skills.config]] in config.toml.
#[derive(Debug)]
struct SkillConfigRule {
    path: Option<String>,
    name: Option<String>,
    enabled: bool,
}

/// Parse and apply [skills] config rules from config.toml.
fn apply_codex_skill_config(config_path: &Path, skills: &mut [StandaloneSkill]) {
    let content = match std::fs::read_to_string(config_path) {
        Ok(c) => c,
        Err(_) => return, // No config file — all skills keep defaults
    };

    let table: toml::Value = match toml::from_str(&content) {
        Ok(v) => v,
        Err(e) => {
            log::warn!(
                "[plugins] failed to parse codex config {}: {}",
                config_path.display(),
                e
            );
            return;
        }
    };

    let skills_section = match table.get("skills") {
        Some(v) => v,
        None => return,
    };

    // Parse [[skills.config]] rules
    let mut rules = Vec::new();
    if let Some(configs) = skills_section.get("config").and_then(|v| v.as_array()) {
        for entry in configs {
            let path = entry.get("path").and_then(|v| v.as_str()).map(String::from);
            let name = entry.get("name").and_then(|v| v.as_str()).map(String::from);
            let enabled = entry
                .get("enabled")
                .and_then(|v| v.as_bool())
                .unwrap_or(true);
            if path.is_some() || name.is_some() {
                rules.push(SkillConfigRule {
                    path,
                    name,
                    enabled,
                });
            }
        }
    }

    // Parse [skills.bundled] enabled
    let bundled_enabled = skills_section
        .get("bundled")
        .and_then(|v| v.get("enabled"))
        .and_then(|v| v.as_bool())
        .unwrap_or(true);

    // Apply bundled disabled (terminal — cannot be overridden)
    if !bundled_enabled {
        for skill in skills.iter_mut() {
            if skill.source_kind == Some(SkillSourceKind::Bundled) {
                skill.enabled = false;
                skill.disabled_by = Some(SkillDisabledBy::Bundled);
            }
        }
    }

    // Apply [[skills.config]] rules in order (later overrides earlier)
    for skill in skills.iter_mut() {
        // Bundled disabled is terminal
        if skill.source_kind == Some(SkillSourceKind::Bundled) && !bundled_enabled {
            continue;
        }

        let mut result_enabled = true;
        let mut result_disabled_by = None;

        for rule in &rules {
            let matches_path = rule.path.as_ref().is_some_and(|rp| skill.path == *rp);
            let matches_name = rule.name.as_ref().is_some_and(|rn| skill.name == *rn);

            if matches_path || matches_name {
                result_enabled = rule.enabled;
                if !rule.enabled {
                    result_disabled_by = Some(if matches_path {
                        SkillDisabledBy::Path
                    } else {
                        SkillDisabledBy::Name
                    });
                } else {
                    result_disabled_by = None;
                }
            }
        }

        skill.enabled = result_enabled;
        skill.disabled_by = result_disabled_by;
    }
}

/// Create a new Codex skill.
pub fn create_codex_skill(
    name: &str,
    description: &str,
    content: &str,
    scope: &str,
    cwd: Option<&str>,
) -> Result<StandaloneSkill, String> {
    create_codex_skill_with_overrides(name, description, content, scope, cwd, None)
}

/// Inner implementation with injectable user_dir for testing.
fn create_codex_skill_with_overrides(
    name: &str,
    description: &str,
    content: &str,
    scope: &str,
    cwd: Option<&str>,
    user_dir_override: Option<&Path>,
) -> Result<StandaloneSkill, String> {
    // Validate name
    validate_codex_skill_name(name)?;

    let (base_dir, source_kind) = match scope {
        "user" => {
            let dir = user_dir_override
                .map(PathBuf::from)
                .unwrap_or_else(codex_user_skills_dir);
            (dir, SkillSourceKind::User)
        }
        "project" => {
            let cwd_str = cwd.ok_or("Working directory required for project-scope skills")?;
            if cwd_str.is_empty() {
                return Err("Working directory required for project-scope skills".to_string());
            }
            let dir = PathBuf::from(cwd_str).join(".agents").join("skills");
            (dir, SkillSourceKind::ProjectAgents)
        }
        _ => {
            return Err(format!(
                "Invalid scope '{}': must be 'user' or 'project'",
                scope
            ))
        }
    };

    let skill_dir = base_dir.join(name);
    if skill_dir.exists() {
        return Err(format!(
            "Skill '{}' already exists in {} scope",
            name, scope
        ));
    }

    // Escape frontmatter values: replace \ with \\, " with \"
    let escaped_name = name.replace('\\', "\\\\").replace('"', "\\\"");
    let escaped_desc = description.replace('\\', "\\\\").replace('"', "\\\"");

    let full_content = format!(
        "---\nname: \"{}\"\ndescription: \"{}\"\n---\n\n{}",
        escaped_name, escaped_desc, content
    );

    std::fs::create_dir_all(&skill_dir)
        .map_err(|e| format!("Failed to create skill directory: {}", e))?;

    let skill_md = skill_dir.join("SKILL.md");
    std::fs::write(&skill_md, &full_content)
        .map_err(|e| format!("Failed to write SKILL.md: {}", e))?;

    log::debug!(
        "[plugins] create_codex_skill: name={}, scope={}, path={}",
        name,
        scope,
        skill_md.display()
    );

    Ok(StandaloneSkill {
        name: name.to_string(),
        description: description.to_string(),
        path: skill_md.to_string_lossy().to_string(),
        scope: scope.to_string(),
        agent: "codex".to_string(),
        source_kind: Some(source_kind),
        enabled: true,
        disabled_by: None,
        can_edit: false,
        can_delete: true,
        can_toggle: true,
    })
}

/// Validate a Codex skill name.
fn validate_codex_skill_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("Skill name cannot be empty".to_string());
    }
    if name.starts_with('.') {
        return Err("Skill name cannot start with a dot".to_string());
    }
    if name.contains("..") {
        return Err("Skill name cannot contain '..'".to_string());
    }
    if name.contains('/') || name.contains('\\') {
        return Err("Skill name cannot contain path separators".to_string());
    }
    if name.contains('\0') {
        return Err("Skill name cannot contain null bytes".to_string());
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
    Ok(())
}

/// Delete a Codex skill by removing its entire directory.
pub fn delete_codex_skill(path: &str, cwd: Option<&str>) -> Result<(), String> {
    delete_codex_skill_with_overrides(path, cwd, None, None, None)
}

/// Inner implementation with injectable paths for testing.
fn delete_codex_skill_with_overrides(
    path: &str,
    cwd: Option<&str>,
    user_dir_override: Option<&Path>,
    legacy_dir_override: Option<&Path>,
    bundled_dir_override: Option<&Path>,
) -> Result<(), String> {
    let path_buf = PathBuf::from(path);
    let canonical =
        std::fs::canonicalize(&path_buf).map_err(|e| format!("Cannot resolve path: {}", e))?;

    // Verify it's a SKILL.md
    if canonical.file_name().and_then(|n| n.to_str()) != Some("SKILL.md") {
        return Err("Can only delete SKILL.md files".to_string());
    }

    let user_dir = user_dir_override
        .map(PathBuf::from)
        .unwrap_or_else(codex_user_skills_dir);
    let legacy_dir = legacy_dir_override
        .map(PathBuf::from)
        .unwrap_or_else(|| codex_legacy_skills_dir().unwrap_or_default());
    let bundled_dir = bundled_dir_override
        .map(PathBuf::from)
        .unwrap_or_else(|| codex_bundled_skills_dir().unwrap_or_default());

    // 1. System (.system/) → always reject (check BEFORE legacy!)
    if let Ok(bc) = std::fs::canonicalize(&bundled_dir) {
        if canonical.starts_with(&bc) {
            return Err("Cannot delete bundled (system) skills".to_string());
        }
    }

    let mut allowed = false;

    // 2. $HOME/.agents/skills/ → allow
    if let Ok(uc) = std::fs::canonicalize(&user_dir) {
        if canonical.starts_with(&uc) {
            allowed = true;
            // Root-level safety
            if let Some(parent) = canonical.parent() {
                if parent == uc {
                    return Err("Cannot delete root-level skill file".to_string());
                }
            }
        }
    }

    // 3. $CODEX_HOME/skills/ (excluding .system/) → allow (legacy)
    if !allowed {
        if let Ok(lc) = std::fs::canonicalize(&legacy_dir) {
            if canonical.starts_with(&lc) {
                allowed = true;
                if let Some(parent) = canonical.parent() {
                    if parent == lc {
                        return Err("Cannot delete root-level skill file".to_string());
                    }
                }
            }
        }
    }

    // 4-5. Project paths (needs cwd)
    if !allowed {
        if let Some(cwd_str) = cwd {
            let layers = project_layers(cwd_str);
            for layer in &layers {
                for sub in &[".agents/skills", ".codex/skills"] {
                    let proj_dir = layer.join(sub);
                    if let Ok(pc) = std::fs::canonicalize(&proj_dir) {
                        if canonical.starts_with(&pc) {
                            allowed = true;
                            if let Some(parent) = canonical.parent() {
                                if parent == pc {
                                    return Err("Cannot delete root-level skill file".to_string());
                                }
                            }
                            break;
                        }
                    }
                }
                if allowed {
                    break;
                }
            }
        }
    }

    if !allowed {
        return Err("Access denied: path is outside allowed Codex skill directories".to_string());
    }

    // Delete the entire skill directory (parent of SKILL.md)
    let skill_dir = canonical
        .parent()
        .ok_or_else(|| "Cannot determine skill directory".to_string())?;

    std::fs::remove_dir_all(skill_dir).map_err(|e| format!("Failed to delete skill: {}", e))?;

    log::debug!(
        "[plugins] delete_codex_skill: path={}, dir={}",
        path,
        skill_dir.display()
    );

    Ok(())
}

/// Toggle a Codex skill's enabled state via config.toml.
pub fn toggle_codex_skill(
    skill_path: &str,
    enabled: bool,
    cwd: Option<&str>,
) -> Result<(), String> {
    let config_path = crate::storage::cli_config::codex_config_path()?;
    toggle_codex_skill_with_overrides(skill_path, enabled, cwd, &config_path, None, None)
}

/// Inner implementation with injectable paths for testing.
fn toggle_codex_skill_with_overrides(
    skill_path: &str,
    enabled: bool,
    cwd: Option<&str>,
    config_path: &Path,
    bundled_dir_override: Option<&Path>,
    extra_roots: Option<&[PathBuf]>,
) -> Result<(), String> {
    let path_buf = PathBuf::from(skill_path);

    // Pre-validation: path must exist
    if !path_buf.exists() {
        return Err(format!("Skill path does not exist: {}", skill_path));
    }

    let canonical =
        std::fs::canonicalize(&path_buf).map_err(|e| format!("Cannot resolve path: {}", e))?;

    // .system/ path → Err
    let bundled_dir = bundled_dir_override
        .map(PathBuf::from)
        .unwrap_or_else(|| codex_bundled_skills_dir().unwrap_or_default());
    if let Ok(bc) = std::fs::canonicalize(&bundled_dir) {
        if canonical.starts_with(&bc) {
            return Err("Cannot toggle bundled (system) skills".to_string());
        }
    }

    // Check belongs to known Codex skill roots
    let mut belongs = false;
    let mut known_roots = Vec::new();

    let user_dir = codex_user_skills_dir();
    if let Ok(c) = std::fs::canonicalize(&user_dir) {
        known_roots.push(c);
    }
    if let Ok(legacy) = codex_legacy_skills_dir() {
        if let Ok(c) = std::fs::canonicalize(&legacy) {
            known_roots.push(c);
        }
    }
    if let Some(cwd_str) = cwd {
        for layer in project_layers(cwd_str) {
            for sub in &[".agents/skills", ".codex/skills"] {
                let d = layer.join(sub);
                if let Ok(c) = std::fs::canonicalize(&d) {
                    known_roots.push(c);
                }
            }
        }
    }
    if let Some(extras) = extra_roots {
        for r in extras {
            if let Ok(c) = std::fs::canonicalize(r) {
                known_roots.push(c);
            }
        }
    }

    for root in &known_roots {
        if canonical.starts_with(root) {
            belongs = true;
            break;
        }
    }

    if !belongs {
        return Err("Skill path does not belong to any known Codex skill directory".to_string());
    }

    // Read or create config.toml
    let content = std::fs::read_to_string(config_path).unwrap_or_default();
    let mut doc: toml_edit::DocumentMut = content
        .parse::<toml_edit::DocumentMut>()
        .map_err(|e| format!("Failed to parse config.toml: {}", e))?;

    // Ensure [skills] and [[skills.config]] exist
    if doc.get("skills").is_none() {
        doc["skills"] = toml_edit::Item::Table(toml_edit::Table::new());
    }

    if enabled {
        // Enable: parse name from SKILL.md frontmatter
        let (name, _) = parse_skill_frontmatter(&PathBuf::from(skill_path));
        if name.is_empty() {
            return Err(
                "Cannot parse skill name from SKILL.md frontmatter (required for enable)"
                    .to_string(),
            );
        }

        // Remove existing entries for this path
        remove_config_entries_for_path(&mut doc, skill_path);

        // Check if still disabled by a name rule
        let still_disabled_by_name = is_disabled_by_name_rule(&doc, &name);

        if still_disabled_by_name {
            // Append path=... enabled=true to override
            append_skill_config_entry(&mut doc, skill_path, true);
        }
        // else: removing the old entry restored default (enabled)
    } else {
        // Disable: remove existing entries for this path, then add disabled
        remove_config_entries_for_path(&mut doc, skill_path);
        append_skill_config_entry(&mut doc, skill_path, false);
    }

    // Write back
    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create config directory: {}", e))?;
    }
    std::fs::write(config_path, doc.to_string())
        .map_err(|e| format!("Failed to write config.toml: {}", e))?;

    // Match cli_config.rs: set 0600 permissions (config may contain sensitive skill rules)
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(config_path, std::fs::Permissions::from_mode(0o600));
    }

    log::debug!(
        "[plugins] toggle_codex_skill: path={}, enabled={}",
        skill_path,
        enabled
    );

    Ok(())
}

/// Remove all [[skills.config]] entries that match the given path.
fn remove_config_entries_for_path(doc: &mut toml_edit::DocumentMut, path: &str) {
    if let Some(skills) = doc.get_mut("skills").and_then(|v| v.as_table_mut()) {
        if let Some(config_item) = skills.get_mut("config") {
            if let Some(arr) = config_item.as_array_of_tables_mut() {
                // Collect indices to remove (reverse order)
                let mut to_remove = Vec::new();
                for (i, entry) in arr.iter().enumerate() {
                    if let Some(p) = entry.get("path").and_then(|v| v.as_str()) {
                        if p == path {
                            to_remove.push(i);
                        }
                    }
                }
                for i in to_remove.into_iter().rev() {
                    arr.remove(i);
                }
            }
        }
    }
}

/// Check if a skill name is disabled by any name rule in [[skills.config]].
fn is_disabled_by_name_rule(doc: &toml_edit::DocumentMut, name: &str) -> bool {
    if let Some(skills) = doc.get("skills").and_then(|v| v.as_table()) {
        if let Some(config_item) = skills.get("config") {
            if let Some(arr) = config_item.as_array_of_tables() {
                let mut disabled = false;
                for entry in arr.iter() {
                    if let Some(n) = entry.get("name").and_then(|v| v.as_str()) {
                        if n == name {
                            let e = entry
                                .get("enabled")
                                .and_then(|v| v.as_bool())
                                .unwrap_or(true);
                            disabled = !e;
                        }
                    }
                }
                return disabled;
            }
        }
    }
    false
}

/// Append a [[skills.config]] entry with path and enabled.
fn append_skill_config_entry(doc: &mut toml_edit::DocumentMut, path: &str, enabled: bool) {
    // Ensure [skills] exists
    if doc.get("skills").is_none() {
        doc["skills"] = toml_edit::Item::Table(toml_edit::Table::new());
    }

    let skills = doc["skills"].as_table_mut().unwrap();

    // Ensure [[skills.config]] array exists
    if skills.get("config").is_none() {
        skills.insert(
            "config",
            toml_edit::Item::ArrayOfTables(toml_edit::ArrayOfTables::new()),
        );
    }

    if let Some(arr) = skills
        .get_mut("config")
        .and_then(|v| v.as_array_of_tables_mut())
    {
        let mut entry = toml_edit::Table::new();
        entry.insert("path", toml_edit::value(path));
        entry.insert("enabled", toml_edit::value(enabled));
        arr.push(entry);
    }
}

// ── Codex installed plugins ──

/// Deserialization helper for `.codex-plugin/plugin.json` manifests.
#[derive(Deserialize)]
struct CodexPluginManifest {
    #[serde(default)]
    name: String,
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    interface: Option<CodexPluginInterface>,
}

#[derive(Deserialize)]
struct CodexPluginInterface {
    #[serde(default, rename = "displayName")]
    display_name: Option<String>,
    #[serde(default, rename = "shortDescription")]
    short_description: Option<String>,
}

/// List installed Codex plugins from `$CODEX_HOME/plugins/cache/`.
///
/// Scans the two-level directory structure: `{marketplace}/{plugin_name}/{version}/`.
/// For each plugin selects the active version ("local" if present, otherwise the
/// lexicographically last entry).  Reads `.codex-plugin/plugin.json` for metadata.
/// Enable/disable state comes from `$CODEX_HOME/config.toml` `[plugins."id@marketplace"]`.
pub fn list_codex_installed_plugins() -> Vec<InstalledPlugin> {
    let codex_home = match crate::storage::cli_config::codex_home_dir() {
        Ok(d) => d,
        Err(e) => {
            log::warn!("[plugins] codex_home_dir error: {}", e);
            return Vec::new();
        }
    };

    let cache_root = codex_home.join("plugins").join("cache");
    if !cache_root.is_dir() {
        log::debug!(
            "[plugins] codex plugin cache not found: {}",
            cache_root.display()
        );
        return Vec::new();
    }

    // Load config.toml once to extract [plugins.*] enabled state
    let (config, config_warning) = crate::storage::cli_config::load_codex_config();
    let plugins_config = config
        .get("plugins")
        .and_then(|v| v.as_object())
        .cloned()
        .unwrap_or_default();

    let mut result = Vec::new();

    // Iterate marketplace directories
    let mp_entries = match std::fs::read_dir(&cache_root) {
        Ok(e) => e,
        Err(e) => {
            log::warn!("[plugins] cannot read cache dir: {}", e);
            return Vec::new();
        }
    };

    for mp_entry in mp_entries.flatten() {
        if !mp_entry.path().is_dir() {
            continue;
        }
        let marketplace = mp_entry.file_name().to_string_lossy().to_string();

        // Iterate plugin directories within this marketplace
        let plugin_entries = match std::fs::read_dir(mp_entry.path()) {
            Ok(e) => e,
            Err(_) => continue,
        };

        for plugin_entry in plugin_entries.flatten() {
            if !plugin_entry.path().is_dir() {
                continue;
            }
            let plugin_dir_name = plugin_entry.file_name().to_string_lossy().to_string();

            // Select active version
            let version_entries: Vec<String> = match std::fs::read_dir(plugin_entry.path()) {
                Ok(e) => e
                    .flatten()
                    .filter(|de| de.path().is_dir())
                    .map(|de| de.file_name().to_string_lossy().to_string())
                    .collect(),
                Err(_) => continue,
            };

            if version_entries.is_empty() {
                continue;
            }

            let active_version = if version_entries.iter().any(|v| v == "local") {
                "local".to_string()
            } else {
                let mut sorted = version_entries;
                sorted.sort();
                sorted.last().unwrap().clone()
            };

            log::debug!(
                "[plugins] codex plugin {}/{}: active version={}",
                marketplace,
                plugin_dir_name,
                active_version
            );

            let version_path = plugin_entry.path().join(&active_version);
            let manifest_path = version_path.join(".codex-plugin").join("plugin.json");

            // Parse manifest
            let manifest: Option<CodexPluginManifest> =
                match std::fs::read_to_string(&manifest_path) {
                    Ok(s) => match serde_json::from_str(&s) {
                        Ok(m) => Some(m),
                        Err(e) => {
                            log::warn!("[plugins] bad manifest {}: {}", manifest_path.display(), e);
                            None
                        }
                    },
                    Err(e) => {
                        log::warn!(
                            "[plugins] cannot read manifest {}: {}",
                            manifest_path.display(),
                            e
                        );
                        None
                    }
                };

            // Skip plugins with unreadable manifests
            let manifest = match manifest {
                Some(m) => m,
                None => continue,
            };

            // Construct plugin_id: "{plugin_dir_name}@{marketplace}"
            let plugin_id = format!("{}@{}", plugin_dir_name, marketplace);

            // Determine display name / description from manifest
            let display_name = manifest
                .interface
                .as_ref()
                .and_then(|i| i.display_name.as_deref())
                .unwrap_or_else(|| {
                    if manifest.name.is_empty() {
                        &plugin_dir_name
                    } else {
                        &manifest.name
                    }
                })
                .to_string();

            let description = manifest
                .interface
                .as_ref()
                .and_then(|i| i.short_description.clone())
                .or(manifest.description)
                .unwrap_or_default();

            let version = manifest.version.unwrap_or_else(|| active_version.clone());

            // Check enabled state from config.toml [plugins."plugin_id"]
            let enabled = plugins_config
                .get(&plugin_id)
                .and_then(|v| v.get("enabled"))
                .and_then(|v| v.as_bool())
                .unwrap_or(true); // default to enabled (matches Codex behavior)

            let mut extra = serde_json::Map::new();
            if let Some(ref warning) = config_warning {
                log::warn!(
                    "[plugins] config.toml warning — plugin {} defaulting to enabled=true",
                    plugin_id
                );
                extra.insert(
                    "configWarning".to_string(),
                    serde_json::Value::String(warning.clone()),
                );
            }

            result.push(InstalledPlugin {
                name: display_name,
                description,
                version: Some(version),
                scope: Some("user".to_string()),
                enabled: Some(enabled),
                marketplace: Some(marketplace.clone()),
                plugin_id: Some(plugin_id),
                agent: Some("codex".to_string()),
                project_path: None,
                extra,
            });
        }
    }

    log::debug!(
        "[plugins] list_codex_installed_plugins: found {} plugins",
        result.len()
    );
    result
}

/// Toggle a Codex plugin's enabled state in `$CODEX_HOME/config.toml`.
///
/// Sets `[plugins."plugin_id"] enabled = true/false` using `toml_edit` to
/// preserve existing formatting and other fields.
pub fn toggle_codex_plugin(plugin_id: &str, enabled: bool) -> Result<(), String> {
    // Validate plugin_id format: must contain @, both sides non-empty,
    // only ASCII alphanumeric + - _
    if !plugin_id.contains('@') {
        return Err("Invalid plugin_id: must contain '@'".to_string());
    }
    let parts: Vec<&str> = plugin_id.splitn(2, '@').collect();
    if parts.len() != 2 || parts[0].is_empty() || parts[1].is_empty() {
        return Err("Invalid plugin_id: name and marketplace must be non-empty".to_string());
    }
    for segment in &parts {
        if segment
            .chars()
            .any(|c| !c.is_ascii_alphanumeric() && c != '-' && c != '_')
        {
            return Err(format!(
                "Invalid plugin_id segment '{}': only ASCII alphanumeric, '-', '_' allowed",
                segment
            ));
        }
    }

    let config_path = crate::storage::cli_config::codex_config_path()?;

    // Read or create the TOML document
    let mut doc: toml_edit::DocumentMut = match std::fs::read_to_string(&config_path) {
        Ok(s) => s
            .parse::<toml_edit::DocumentMut>()
            .map_err(|e| format!("config.toml parse error: {}", e))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => toml_edit::DocumentMut::new(),
        Err(e) => return Err(format!("Failed to read config.toml: {}", e)),
    };

    // Ensure [plugins] table exists
    if doc.get("plugins").is_none() || !doc["plugins"].is_table() {
        doc["plugins"] = toml_edit::Item::Table(toml_edit::Table::new());
    }

    let plugins_table = doc["plugins"]
        .as_table_mut()
        .ok_or_else(|| "plugins is not a table".to_string())?;

    // Ensure [plugins."plugin_id"] sub-table exists
    if plugins_table.get(plugin_id).is_none() || !plugins_table[plugin_id].is_table() {
        plugins_table[plugin_id] = toml_edit::Item::Table(toml_edit::Table::new());
    }

    // Set enabled value
    plugins_table[plugin_id]["enabled"] = toml_edit::value(enabled);

    // Write back
    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create config directory: {}", e))?;
    }
    std::fs::write(&config_path, doc.to_string())
        .map_err(|e| format!("Failed to write config.toml: {}", e))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&config_path, std::fs::Permissions::from_mode(0o600));
    }

    log::debug!("[plugins] toggle_codex_plugin: {}={}", plugin_id, enabled);

    Ok(())
}

#[cfg(test)]
mod codex_skill_tests {
    use super::*;
    use tempfile::TempDir;

    fn skill_md_content(name: &str, desc: &str) -> String {
        format!(
            "---\nname: \"{}\"\ndescription: \"{}\"\n---\n\nBody of {}.",
            name, desc, name
        )
    }

    fn write_skill(dir: &Path, rel: &str, name: &str, desc: &str) {
        let full = dir.join(rel);
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        std::fs::write(&full, skill_md_content(name, desc)).unwrap();
    }

    // ── Scanner tests ──

    #[test]
    fn test_codex_skill_recursive_scan() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("skills");
        // 6 levels deep: skills/a/b/c/d/e/f/SKILL.md
        write_skill(&root, "a/b/c/d/e/f/SKILL.md", "deep-skill", "6 levels");
        // Also a shallow one
        write_skill(&root, "top/SKILL.md", "top-skill", "top level");

        let mut skills = Vec::new();
        scan_codex_skills_dir(&root, "user", SkillSourceKind::User, false, &mut skills);

        let names: Vec<&str> = skills.iter().map(|s| s.name.as_str()).collect();
        assert!(
            names.contains(&"deep-skill"),
            "expected deep-skill in {:?}",
            names
        );
        assert!(
            names.contains(&"top-skill"),
            "expected top-skill in {:?}",
            names
        );
        assert_eq!(skills.len(), 2);
    }

    #[test]
    fn test_codex_skill_dedup_by_path() {
        let tmp = TempDir::new().unwrap();
        let dir_a = tmp.path().join("a");
        let dir_b = tmp.path().join("b");
        write_skill(&dir_a, "my-skill/SKILL.md", "my-skill", "from a");
        write_skill(&dir_b, "my-skill/SKILL.md", "my-skill", "from b");

        let skills = list_codex_skills_with_overrides(
            None,
            Some(&dir_a),
            Some(&dir_b),
            Some(tmp.path().join("bundled").as_path()),
            Some(tmp.path().join("config.toml").as_path()),
        );

        // Same name but different paths → both kept (dedup is by canonical path)
        let matching: Vec<_> = skills.iter().filter(|s| s.name == "my-skill").collect();
        assert_eq!(
            matching.len(),
            2,
            "same name different path should both be kept"
        );
    }

    #[test]
    fn test_codex_skill_enabled_path_rule() {
        let tmp = TempDir::new().unwrap();
        let user_dir = tmp.path().join("user_skills");
        write_skill(&user_dir, "foo/SKILL.md", "foo", "a skill");

        let skill_path = user_dir.join("foo/SKILL.md").to_string_lossy().to_string();
        let config_path = tmp.path().join("config.toml");
        std::fs::write(
            &config_path,
            format!(
                "[[skills.config]]\npath = \"{}\"\nenabled = false\n",
                skill_path
            ),
        )
        .unwrap();

        let skills = list_codex_skills_with_overrides(
            None,
            Some(&user_dir),
            Some(tmp.path().join("empty_legacy").as_path()),
            Some(tmp.path().join("empty_bundled").as_path()),
            Some(&config_path),
        );

        let foo = skills.iter().find(|s| s.name == "foo").unwrap();
        assert!(!foo.enabled);
        assert_eq!(foo.disabled_by, Some(SkillDisabledBy::Path));
    }

    #[test]
    fn test_codex_skill_enabled_name_rule() {
        let tmp = TempDir::new().unwrap();
        let user_dir = tmp.path().join("user_skills");
        write_skill(&user_dir, "bar/SKILL.md", "bar", "a skill");

        let config_path = tmp.path().join("config.toml");
        std::fs::write(
            &config_path,
            "[[skills.config]]\nname = \"bar\"\nenabled = false\n",
        )
        .unwrap();

        let skills = list_codex_skills_with_overrides(
            None,
            Some(&user_dir),
            Some(tmp.path().join("el").as_path()),
            Some(tmp.path().join("eb").as_path()),
            Some(&config_path),
        );

        let bar = skills.iter().find(|s| s.name == "bar").unwrap();
        assert!(!bar.enabled);
        assert_eq!(bar.disabled_by, Some(SkillDisabledBy::Name));
    }

    #[test]
    fn test_codex_skill_name_override_by_path() {
        let tmp = TempDir::new().unwrap();
        let user_dir = tmp.path().join("user_skills");
        write_skill(&user_dir, "baz/SKILL.md", "baz", "a skill");

        let skill_path = user_dir.join("baz/SKILL.md").to_string_lossy().to_string();
        let config_path = tmp.path().join("config.toml");
        // Name disables, then path re-enables
        std::fs::write(
            &config_path,
            format!(
                "[[skills.config]]\nname = \"baz\"\nenabled = false\n\n[[skills.config]]\npath = \"{}\"\nenabled = true\n",
                skill_path
            ),
        ).unwrap();

        let skills = list_codex_skills_with_overrides(
            None,
            Some(&user_dir),
            Some(tmp.path().join("el").as_path()),
            Some(tmp.path().join("eb").as_path()),
            Some(&config_path),
        );

        let baz = skills.iter().find(|s| s.name == "baz").unwrap();
        assert!(baz.enabled, "path rule should override name rule");
        assert_eq!(baz.disabled_by, None);
    }

    #[test]
    fn test_codex_skill_bundled_disabled() {
        let tmp = TempDir::new().unwrap();
        let bundled_dir = tmp.path().join("bundled");
        write_skill(&bundled_dir, "sys/SKILL.md", "sys-skill", "built-in");

        let config_path = tmp.path().join("config.toml");
        std::fs::write(&config_path, "[skills.bundled]\nenabled = false\n").unwrap();

        let skills = list_codex_skills_with_overrides(
            None,
            Some(tmp.path().join("eu").as_path()),
            Some(tmp.path().join("el").as_path()),
            Some(&bundled_dir),
            Some(&config_path),
        );

        let sys = skills.iter().find(|s| s.name == "sys-skill").unwrap();
        assert!(!sys.enabled);
        assert_eq!(sys.disabled_by, Some(SkillDisabledBy::Bundled));
    }

    #[test]
    fn test_codex_skill_bundled_not_overridable_by_path_rule() {
        let tmp = TempDir::new().unwrap();
        let bundled_dir = tmp.path().join("bundled");
        write_skill(&bundled_dir, "sys/SKILL.md", "sys-skill", "built-in");

        let skill_path = bundled_dir
            .join("sys/SKILL.md")
            .to_string_lossy()
            .to_string();
        let config_path = tmp.path().join("config.toml");
        std::fs::write(
            &config_path,
            format!(
                "[skills.bundled]\nenabled = false\n\n[[skills.config]]\npath = \"{}\"\nenabled = true\n",
                skill_path
            ),
        ).unwrap();

        let skills = list_codex_skills_with_overrides(
            None,
            Some(tmp.path().join("eu").as_path()),
            Some(tmp.path().join("el").as_path()),
            Some(&bundled_dir),
            Some(&config_path),
        );

        let sys = skills.iter().find(|s| s.name == "sys-skill").unwrap();
        assert!(
            !sys.enabled,
            "bundled disabled should not be overridden by path rule"
        );
        assert_eq!(sys.disabled_by, Some(SkillDisabledBy::Bundled));
    }

    // ── Toggle tests ──

    #[test]
    fn test_codex_skill_toggle_disable() {
        let tmp = TempDir::new().unwrap();
        let skills_dir = tmp.path().join("skills");
        write_skill(&skills_dir, "my-skill/SKILL.md", "my-skill", "desc");

        let config_path = tmp.path().join("config.toml");
        let skill_path = skills_dir.join("my-skill/SKILL.md");
        let skill_path_str = skill_path.to_string_lossy().to_string();

        toggle_codex_skill_with_overrides(
            &skill_path_str,
            false,
            None,
            &config_path,
            Some(tmp.path().join("no-bundled").as_path()),
            Some(&[skills_dir.clone()]),
        )
        .unwrap();

        let content = std::fs::read_to_string(&config_path).unwrap();
        assert!(
            content.contains("enabled = false"),
            "config should contain disabled entry: {}",
            content
        );
        assert!(
            content.contains(&skill_path_str),
            "config should reference skill path"
        );
    }

    #[test]
    fn test_codex_skill_toggle_enable_path() {
        let tmp = TempDir::new().unwrap();
        let skills_dir = tmp.path().join("skills");
        write_skill(&skills_dir, "my-skill/SKILL.md", "my-skill", "desc");

        let config_path = tmp.path().join("config.toml");
        let skill_path = skills_dir.join("my-skill/SKILL.md");
        let skill_path_str = skill_path.to_string_lossy().to_string();

        // First disable
        std::fs::write(
            &config_path,
            format!(
                "[[skills.config]]\npath = \"{}\"\nenabled = false\n",
                skill_path_str
            ),
        )
        .unwrap();

        // Then enable (should remove the entry since no name rule blocks it)
        toggle_codex_skill_with_overrides(
            &skill_path_str,
            true,
            None,
            &config_path,
            Some(tmp.path().join("no-bundled").as_path()),
            Some(&[skills_dir.clone()]),
        )
        .unwrap();

        let content = std::fs::read_to_string(&config_path).unwrap();
        // Should NOT contain a path entry since removing the disable is enough
        assert!(
            !content.contains(&skill_path_str),
            "enabling should remove path entry when no name rule: {}",
            content
        );
    }

    #[test]
    fn test_codex_skill_toggle_enable_name() {
        let tmp = TempDir::new().unwrap();
        let skills_dir = tmp.path().join("skills");
        write_skill(&skills_dir, "my-skill/SKILL.md", "my-skill", "desc");

        let config_path = tmp.path().join("config.toml");
        let skill_path = skills_dir.join("my-skill/SKILL.md");
        let skill_path_str = skill_path.to_string_lossy().to_string();

        // Name rule disables it + a path entry also disables it
        std::fs::write(
            &config_path,
            format!(
                "[[skills.config]]\nname = \"my-skill\"\nenabled = false\n\n[[skills.config]]\npath = \"{}\"\nenabled = false\n",
                skill_path_str
            ),
        ).unwrap();

        // Enable via path → should append override
        toggle_codex_skill_with_overrides(
            &skill_path_str,
            true,
            None,
            &config_path,
            Some(tmp.path().join("no-bundled").as_path()),
            Some(&[skills_dir.clone()]),
        )
        .unwrap();

        let content = std::fs::read_to_string(&config_path).unwrap();
        // Should have the name disable rule still + a path enable override
        assert!(
            content.contains("name = \"my-skill\""),
            "name rule should remain"
        );
        assert!(
            content.contains("enabled = true"),
            "should have path-enabled override: {}",
            content
        );
    }

    // ── Delete tests ──

    #[test]
    fn test_codex_skill_delete_system_rejected() {
        let tmp = TempDir::new().unwrap();
        let bundled = tmp.path().join("bundled");
        write_skill(&bundled, "sys/SKILL.md", "sys", "system");

        let skill_path = bundled.join("sys/SKILL.md").to_string_lossy().to_string();
        let result = delete_codex_skill_with_overrides(
            &skill_path,
            None,
            Some(tmp.path().join("eu").as_path()),
            Some(tmp.path().join("el").as_path()),
            Some(&bundled),
        );
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("bundled"));
    }

    #[test]
    fn test_codex_skill_delete_system_before_legacy() {
        // Even if bundled is under legacy path, system check should come first
        let tmp = TempDir::new().unwrap();
        let legacy = tmp.path().join("skills");
        let bundled = legacy.join(".system");
        // Note: walk_codex_skills skips dot-entries so .system won't be scanned in legacy,
        // but delete validation needs to check .system before legacy.
        write_skill(&bundled, "sys/SKILL.md", "sys", "system");

        let skill_path = bundled.join("sys/SKILL.md").to_string_lossy().to_string();
        let result = delete_codex_skill_with_overrides(
            &skill_path,
            None,
            Some(tmp.path().join("eu").as_path()),
            Some(&legacy),
            Some(&bundled),
        );
        assert!(
            result.is_err(),
            "system check should reject before legacy allows"
        );
        assert!(result.unwrap_err().contains("bundled"));
    }

    #[test]
    fn test_codex_skill_delete_root_level_rejected() {
        let tmp = TempDir::new().unwrap();
        let user_dir = tmp.path().join("user_skills");
        // SKILL.md directly in root (no subdirectory)
        write_skill(&user_dir, "SKILL.md", "root", "root level");

        let skill_path = user_dir.join("SKILL.md").to_string_lossy().to_string();
        let result = delete_codex_skill_with_overrides(
            &skill_path,
            None,
            Some(&user_dir),
            Some(tmp.path().join("el").as_path()),
            Some(tmp.path().join("eb").as_path()),
        );
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("root-level"));
    }

    // ── Create tests ──

    #[test]
    fn test_codex_skill_create_user_dir() {
        let tmp = TempDir::new().unwrap();
        let user_dir = tmp.path().join("user_skills");

        let result = create_codex_skill_with_overrides(
            "my-skill",
            "desc",
            "body",
            "user",
            None,
            Some(&user_dir),
        );
        assert!(result.is_ok(), "create user skill: {:?}", result);
        let skill = result.unwrap();
        assert_eq!(skill.name, "my-skill");
        assert_eq!(skill.scope, "user");
        assert_eq!(skill.agent, "codex");
        assert!(skill.path.contains("user_skills"));
        assert!(PathBuf::from(&skill.path).exists());
    }

    #[test]
    fn test_codex_skill_create_project_dir() {
        let tmp = TempDir::new().unwrap();
        let cwd = tmp.path().to_str().unwrap();

        let result = create_codex_skill_with_overrides(
            "proj-skill",
            "project desc",
            "body",
            "project",
            Some(cwd),
            None,
        );
        assert!(result.is_ok(), "create project skill: {:?}", result);
        let skill = result.unwrap();
        assert_eq!(skill.scope, "project");
        let expected_dir = tmp.path().join(".agents").join("skills").join("proj-skill");
        assert!(expected_dir.join("SKILL.md").exists());
    }

    #[test]
    fn test_codex_skill_create_name_validation() {
        let tmp = TempDir::new().unwrap();
        let user_dir = tmp.path().join("skills");

        // Empty
        assert!(
            create_codex_skill_with_overrides("", "d", "b", "user", None, Some(&user_dir)).is_err()
        );
        // ".."
        assert!(
            create_codex_skill_with_overrides("..", "d", "b", "user", None, Some(&user_dir))
                .is_err()
        );
        // Contains /
        assert!(
            create_codex_skill_with_overrides("a/b", "d", "b", "user", None, Some(&user_dir))
                .is_err()
        );
        // Dot prefix
        assert!(create_codex_skill_with_overrides(
            ".hidden",
            "d",
            "b",
            "user",
            None,
            Some(&user_dir)
        )
        .is_err());
        // Valid
        assert!(create_codex_skill_with_overrides(
            "my-skill_1",
            "d",
            "b",
            "user",
            None,
            Some(&user_dir)
        )
        .is_ok());
    }

    #[test]
    fn test_codex_skill_create_frontmatter_special_chars() {
        let tmp = TempDir::new().unwrap();
        let user_dir = tmp.path().join("skills");

        let result = create_codex_skill_with_overrides(
            "special",
            "has \"quotes\" and \\backslash",
            "body",
            "user",
            None,
            Some(&user_dir),
        );
        assert!(result.is_ok());
        let skill = result.unwrap();
        let content = std::fs::read_to_string(&skill.path).unwrap();
        assert!(
            content.contains("\\\"quotes\\\""),
            "quotes should be escaped: {}",
            content
        );
        assert!(
            content.contains("\\\\backslash"),
            "backslash should be escaped: {}",
            content
        );
    }

    #[test]
    fn test_codex_skill_create_returns_complete_flags() {
        let tmp = TempDir::new().unwrap();
        let user_dir = tmp.path().join("skills");

        let skill = create_codex_skill_with_overrides(
            "flagtest",
            "desc",
            "body",
            "user",
            None,
            Some(&user_dir),
        )
        .unwrap();

        assert_eq!(skill.agent, "codex");
        assert_eq!(skill.source_kind, Some(SkillSourceKind::User));
        assert!(skill.enabled);
        assert_eq!(skill.disabled_by, None);
        assert!(!skill.can_edit, "v1: can_edit should be false");
        assert!(skill.can_delete);
        assert!(skill.can_toggle);
    }

    // ── can_* flags tests ──

    #[test]
    fn test_codex_skill_can_delete_bundled_false() {
        let tmp = TempDir::new().unwrap();
        let bundled = tmp.path().join("bundled");
        write_skill(&bundled, "sys/SKILL.md", "sys", "system");

        let skills = list_codex_skills_with_overrides(
            None,
            Some(tmp.path().join("eu").as_path()),
            Some(tmp.path().join("el").as_path()),
            Some(&bundled),
            Some(tmp.path().join("config.toml").as_path()),
        );

        let sys = skills.iter().find(|s| s.name == "sys").unwrap();
        assert!(!sys.can_delete, "bundled skills should not be deletable");
    }

    #[test]
    fn test_codex_skill_can_edit_all_codex_false_v1() {
        let tmp = TempDir::new().unwrap();
        let user_dir = tmp.path().join("user");
        let legacy_dir = tmp.path().join("legacy");
        let bundled_dir = tmp.path().join("bundled");
        write_skill(&user_dir, "u/SKILL.md", "u-skill", "user");
        write_skill(&legacy_dir, "l/SKILL.md", "l-skill", "legacy");
        write_skill(&bundled_dir, "b/SKILL.md", "b-skill", "bundled");

        let skills = list_codex_skills_with_overrides(
            None,
            Some(&user_dir),
            Some(&legacy_dir),
            Some(&bundled_dir),
            Some(tmp.path().join("config.toml").as_path()),
        );

        for skill in &skills {
            assert!(
                !skill.can_edit,
                "v1: all codex skills should have can_edit=false, got true for {}",
                skill.name
            );
        }
    }

    #[test]
    fn test_codex_skill_can_toggle_bundled_false() {
        let tmp = TempDir::new().unwrap();
        let bundled = tmp.path().join("bundled");
        write_skill(&bundled, "sys/SKILL.md", "sys", "system");

        let skills = list_codex_skills_with_overrides(
            None,
            Some(tmp.path().join("eu").as_path()),
            Some(tmp.path().join("el").as_path()),
            Some(&bundled),
            Some(tmp.path().join("config.toml").as_path()),
        );

        let sys = skills.iter().find(|s| s.name == "sys").unwrap();
        assert!(!sys.can_toggle, "bundled skills should not be toggleable");
    }

    #[test]
    fn test_codex_skill_scanner_skips_dot_entries() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("skills");
        write_skill(&root, ".hidden/SKILL.md", "hidden", "should be skipped");
        write_skill(&root, "visible/SKILL.md", "visible", "should appear");

        let mut skills = Vec::new();
        scan_codex_skills_dir(&root, "user", SkillSourceKind::User, false, &mut skills);

        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, "visible");
    }

    // ── Toggle rejection tests ──

    #[test]
    fn test_codex_skill_toggle_rejects_system_path() {
        let tmp = TempDir::new().unwrap();
        let bundled = tmp.path().join("bundled");
        write_skill(&bundled, "sys/SKILL.md", "sys", "system");

        let skill_path = bundled.join("sys/SKILL.md").to_string_lossy().to_string();
        let config_path = tmp.path().join("config.toml");

        let result = toggle_codex_skill_with_overrides(
            &skill_path,
            false,
            None,
            &config_path,
            Some(&bundled),
            None,
        );
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("bundled"));
    }

    #[test]
    fn test_codex_skill_toggle_rejects_unknown_path() {
        let config_path = PathBuf::from("/tmp/nonexistent-config.toml");

        let result = toggle_codex_skill_with_overrides(
            "/tmp/does-not-exist/SKILL.md",
            false,
            None,
            &config_path,
            None,
            None,
        );
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("does not exist"));
    }

    #[test]
    fn test_codex_skill_list_cwd_none_skips_project() {
        let tmp = TempDir::new().unwrap();
        let user_dir = tmp.path().join("user_skills");
        write_skill(&user_dir, "u/SKILL.md", "user-skill", "user");

        // Even if cwd has skills, passing None should skip project scanning
        let skills = list_codex_skills_with_overrides(
            None,
            Some(&user_dir),
            Some(tmp.path().join("el").as_path()),
            Some(tmp.path().join("eb").as_path()),
            Some(tmp.path().join("config.toml").as_path()),
        );

        for skill in &skills {
            assert_ne!(
                skill.scope, "project",
                "no project skills should appear with cwd=None"
            );
        }
    }

    #[test]
    fn test_codex_skill_toggle_enable_bad_frontmatter_err() {
        let tmp = TempDir::new().unwrap();
        let skills_dir = tmp.path().join("skills");
        // Write skill with NO frontmatter
        let skill_dir = skills_dir.join("bad");
        std::fs::create_dir_all(&skill_dir).unwrap();
        let skill_md = skill_dir.join("SKILL.md");
        std::fs::write(&skill_md, "No frontmatter here").unwrap();

        let config_path = tmp.path().join("config.toml");
        let skill_path_str = skill_md.to_string_lossy().to_string();

        let result = toggle_codex_skill_with_overrides(
            &skill_path_str,
            true, // enable requires parsing name
            None,
            &config_path,
            Some(tmp.path().join("no-bundled").as_path()),
            Some(&[skills_dir]),
        );
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("frontmatter"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn names(stems: &[(String, PathBuf)]) -> Vec<&str> {
        stems.iter().map(|(n, _)| n.as_str()).collect()
    }

    #[test]
    fn list_md_stems_flat() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("apply.md"), "---\nname: apply\n---").unwrap();
        fs::write(dir.path().join("review.md"), "---\nname: review\n---").unwrap();

        let stems = list_md_stems(dir.path());
        let names = names(&stems);
        assert_eq!(stems.len(), 2);
        assert!(names.contains(&"apply"));
        assert!(names.contains(&"review"));
    }

    #[test]
    fn list_md_stems_nested_uses_colon_prefix() {
        let dir = tempdir().unwrap();
        fs::create_dir_all(dir.path().join("opsx")).unwrap();
        fs::write(
            dir.path().join("opsx").join("apply.md"),
            "---\nname: apply\n---",
        )
        .unwrap();
        fs::write(
            dir.path().join("opsx").join("continue.md"),
            "---\nname: continue\n---",
        )
        .unwrap();
        fs::write(dir.path().join("test.md"), "---\nname: test\n---").unwrap();

        let stems = list_md_stems(dir.path());
        let names = names(&stems);
        assert_eq!(stems.len(), 3);
        assert!(names.contains(&"opsx:apply"));
        assert!(names.contains(&"opsx:continue"));
        assert!(names.contains(&"test"));
    }

    #[test]
    fn list_md_stems_returns_real_paths_for_nested() {
        // Regression: scan_commands_dir needs the actual file path to read
        // frontmatter; colon-encoded names must not be re-joined as `opsx:apply.md`.
        let dir = tempdir().unwrap();
        fs::create_dir_all(dir.path().join("opsx")).unwrap();
        let expected = dir.path().join("opsx").join("apply.md");
        fs::write(&expected, "---\nname: apply\n---").unwrap();

        let stems = list_md_stems(dir.path());
        let (name, path) = stems.iter().find(|(n, _)| n == "opsx:apply").unwrap();
        assert_eq!(name, "opsx:apply");
        assert_eq!(path, &expected);
        assert!(path.is_file(), "returned path must exist on disk");
    }

    #[test]
    fn list_md_stems_deeply_nested() {
        let dir = tempdir().unwrap();
        fs::create_dir_all(dir.path().join("a").join("b")).unwrap();
        fs::write(dir.path().join("a").join("b").join("cmd.md"), "x").unwrap();

        let stems = list_md_stems(dir.path());
        let names = names(&stems);
        assert_eq!(stems.len(), 1);
        assert!(names.contains(&"a:b:cmd"));
    }

    #[test]
    fn scan_commands_dir_uses_path_name_and_reads_description() {
        // Invocation name must be path-derived ("opsx:apply"), not the
        // frontmatter `name` (which is just a display label and can't encode
        // nested invocation). Description is pulled from frontmatter.
        let dir = tempdir().unwrap();
        fs::create_dir_all(dir.path().join("opsx")).unwrap();
        fs::write(
            dir.path().join("opsx").join("apply.md"),
            "---\nname: opsx-apply\ndescription: Apply an opsx change\n---\nbody",
        )
        .unwrap();

        let mut commands = Vec::new();
        let mut seen = std::collections::HashSet::new();
        scan_commands_dir(dir.path(), &mut commands, &mut seen);

        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0].name, "opsx:apply");
        assert_eq!(commands[0].description, "Apply an opsx change");
        assert!(seen.contains("opsx:apply"));
    }

    #[cfg(unix)]
    #[test]
    fn list_md_stems_follows_symlinked_file() {
        use std::os::unix::fs::symlink;
        // A common pattern: shared command files in a dotfiles repo, symlinked
        // into ~/.claude/commands/ so they show up in the slash menu.
        let real_dir = tempdir().unwrap();
        let link_dir = tempdir().unwrap();
        let target = real_dir.path().join("review.md");
        fs::write(&target, "---\nname: review\n---").unwrap();
        symlink(&target, link_dir.path().join("review.md")).unwrap();

        let stems = list_md_stems(link_dir.path());
        let names = names(&stems);
        assert_eq!(stems.len(), 1, "symlinked .md must be discovered");
        assert!(names.contains(&"review"));
    }

    #[cfg(unix)]
    #[test]
    fn list_md_stems_follows_symlinked_dir() {
        use std::os::unix::fs::symlink;
        // Whole-directory symlink (e.g. `commands/team -> ~/dotfiles/team-commands/`).
        // Nested entries must surface with the symlink name as the prefix.
        let real_dir = tempdir().unwrap();
        let link_dir = tempdir().unwrap();
        fs::create_dir_all(real_dir.path().join("nested")).unwrap();
        fs::write(real_dir.path().join("nested").join("apply.md"), "x").unwrap();
        fs::write(real_dir.path().join("plain.md"), "x").unwrap();

        symlink(real_dir.path(), link_dir.path().join("team")).unwrap();

        let stems = list_md_stems(link_dir.path());
        let names = names(&stems);
        assert!(
            names.contains(&"team:plain"),
            "flat .md under symlinked dir missing; got: {:?}",
            names
        );
        assert!(
            names.contains(&"team:nested:apply"),
            "nested .md under symlinked dir missing; got: {:?}",
            names
        );
    }

    #[cfg(unix)]
    #[test]
    fn list_md_stems_terminates_on_symlink_cycle() {
        use std::os::unix::fs::symlink;
        // a/loop -> .. (creates a cycle a/loop/loop/loop/...). MAX_COMMAND_DEPTH
        // must bound the recursion regardless of follow-symlinks behaviour.
        let dir = tempdir().unwrap();
        fs::create_dir_all(dir.path().join("a")).unwrap();
        fs::write(dir.path().join("a").join("real.md"), "x").unwrap();
        symlink(dir.path().join("a"), dir.path().join("a").join("loop")).unwrap();

        // Should return without hanging. We don't assert an exact count — the
        // depth cap may duplicate the real.md entry along each loop level — but
        // we DO require at least one occurrence of "a:real" or "real".
        let stems = list_md_stems(dir.path());
        let names = names(&stems);
        assert!(
            names.iter().any(|n| n.ends_with("real") || *n == "real"),
            "expected real.md to appear despite the cycle; got: {:?}",
            names
        );
    }

    #[test]
    fn scan_commands_dir_ignores_frontmatter_name_for_flat_command() {
        // Regression: even for flat commands, frontmatter `name` must not
        // override the file stem. CLI invocation is `/foo` for `foo.md`.
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join("foo.md"),
            "---\nname: bar\ndescription: Foo helper\n---\nbody",
        )
        .unwrap();

        let mut commands = Vec::new();
        let mut seen = std::collections::HashSet::new();
        scan_commands_dir(dir.path(), &mut commands, &mut seen);

        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0].name, "foo");
        assert_eq!(commands[0].description, "Foo helper");
    }
}
