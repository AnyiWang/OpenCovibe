use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
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
        agent: None,
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

// ── Codex plugin agent discovery ──

/// Collect agents from a .tmp plugin dir.
/// Inserts marketplace-free alias keys into `seen_tmp_aliases` to block cache fallback.
fn collect_tmp_codex_agents(
    dir: &Path,
    source: &str,
    plugin_name: &str,
    seen_tmp_aliases: &mut HashSet<String>,
    seen_canonical: &mut HashSet<PathBuf>,
    out: &mut Vec<AgentDefinitionSummary>,
) {
    for mut a in scan_agents_dir(dir, source, "plugin", true) {
        let md_path = dir.join(format!("{}.md", a.file_name));
        let canonical = std::fs::canonicalize(&md_path).unwrap_or(md_path);
        if seen_canonical.contains(&canonical) {
            continue;
        }
        let alias = format!("codex-plugin:{}:{}", plugin_name, a.file_name);
        if seen_tmp_aliases.contains(&alias) {
            continue;
        }
        seen_canonical.insert(canonical);
        seen_tmp_aliases.insert(alias);
        a.agent = Some("codex".into());
        out.push(a);
    }
}

/// Collect agents from a cache plugin dir.
/// Checks `seen_tmp_aliases` (marketplace-free) to block .tmp→cache duplicates.
/// Uses marketplace-aware key in `seen_cache` so same-name plugins from
/// different marketplaces coexist.
fn collect_cache_codex_agents(
    dir: &Path,
    source: &str,
    tmp_plugin_key: &str,
    seen_tmp_aliases: &HashSet<String>,
    cache_key_prefix: &str,
    seen_cache: &mut HashSet<String>,
    seen_canonical: &mut HashSet<PathBuf>,
    out: &mut Vec<AgentDefinitionSummary>,
) {
    for mut a in scan_agents_dir(dir, source, "plugin", true) {
        let md_path = dir.join(format!("{}.md", a.file_name));
        let canonical = std::fs::canonicalize(&md_path).unwrap_or(md_path);
        if seen_canonical.contains(&canonical) {
            continue;
        }
        // Blocked by .tmp alias?
        let tmp_alias = format!("codex-plugin:{}:{}", tmp_plugin_key, a.file_name);
        if seen_tmp_aliases.contains(&tmp_alias) {
            continue;
        }
        // Marketplace-aware cache dedup
        let cache_key = format!("codex-plugin:{}:{}", cache_key_prefix, a.file_name);
        if seen_cache.contains(&cache_key) {
            continue;
        }
        seen_canonical.insert(canonical);
        seen_cache.insert(cache_key);
        a.agent = Some("codex".into());
        out.push(a);
    }
}

/// Check if a Codex plugin is disabled.
///
/// - With marketplace: looks up `"plugin@marketplace"` in `plugins_config`.
/// - Without marketplace (.tmp path): uses `plugin_enabled_map` to find the
///   matching marketplace entry. Unambiguous single match → use its state.
///   Multiple marketplaces → conservative show. No match → show.
fn is_codex_plugin_disabled(
    plugin_name: &str,
    marketplace: Option<&str>,
    plugins_config: &serde_json::Map<String, serde_json::Value>,
    plugin_enabled_map: &HashMap<String, Vec<(String, bool)>>,
) -> bool {
    match marketplace {
        Some(mp) => {
            let id = format!("{}@{}", plugin_name, mp);
            plugins_config
                .get(&id)
                .and_then(|v| v.get("enabled"))
                .and_then(|v| v.as_bool())
                .map(|e| !e)
                .unwrap_or(false)
        }
        None => {
            // .tmp path — no marketplace, use mapping
            match plugin_enabled_map.get(plugin_name) {
                Some(entries) if entries.len() == 1 => !entries[0].1,
                Some(entries) if entries.len() > 1 => {
                    log::debug!(
                        "[agents] ambiguous plugin '{}' across {} marketplaces, showing",
                        plugin_name,
                        entries.len()
                    );
                    false // conservative: show
                }
                _ => false, // no match → show
            }
        }
    }
}

/// Discover agents from installed Codex plugins.
/// Scans two locations:
///   1. Primary: ~/.codex/.tmp/plugins/plugins/*/agents/
///   2. Fallback: ~/.codex/plugins/cache/{marketplace}/{plugin}/{version}/agents/
///              + skills/*/agents/
/// Respects enabled/disabled state from config.toml [plugins.*].
fn discover_codex_plugin_agents() -> Vec<AgentDefinitionSummary> {
    let codex_home = match crate::storage::cli_config::codex_home_dir() {
        Ok(d) => d,
        Err(e) => {
            log::warn!("[agents] codex_home_dir error: {}", e);
            return vec![];
        }
    };

    // Load config.toml once — extract [plugins.*] enabled states
    let (config, config_warning) = crate::storage::cli_config::load_codex_config();
    let plugins_config = config
        .get("plugins")
        .and_then(|v| v.as_object())
        .cloned()
        .unwrap_or_default();

    if config_warning.is_some() {
        log::warn!(
            "[agents] config.toml warning — all Codex plugin agents will default to enabled"
        );
    }

    // Build plugin_name → Vec<(marketplace, enabled)> mapping.
    // Two sources: (1) cache directory structure, (2) config.toml [plugins.*] entries.
    // Config fallback covers plugins in .tmp but not in cache (e.g. vercel, figma).
    let mut plugin_enabled_map: HashMap<String, Vec<(String, bool)>> = HashMap::new();
    let mut seen_config_ids: HashSet<String> = HashSet::new();

    // Source 1: cache directory — physical plugin dirs
    let cache_root_for_map = codex_home.join("plugins").join("cache");
    if cache_root_for_map.is_dir() {
        for mp in std::fs::read_dir(&cache_root_for_map)
            .into_iter()
            .flatten()
            .flatten()
        {
            if !mp.path().is_dir() {
                continue;
            }
            let mp_name = mp.file_name().to_string_lossy().to_string();
            for plugin in std::fs::read_dir(mp.path()).into_iter().flatten().flatten() {
                if !plugin.path().is_dir() {
                    continue;
                }
                let pname = plugin.file_name().to_string_lossy().to_string();
                let id = format!("{}@{}", pname, mp_name);
                let enabled = plugins_config
                    .get(&id)
                    .and_then(|v| v.get("enabled"))
                    .and_then(|v| v.as_bool())
                    .unwrap_or(true);
                seen_config_ids.insert(id);
                plugin_enabled_map
                    .entry(pname)
                    .or_default()
                    .push((mp_name.clone(), enabled));
            }
        }
    }

    // Source 2: config.toml [plugins.*] entries not already covered by cache.
    // Handles plugins that exist in .tmp but have no cache directory.
    // Config key format: "plugin_name@marketplace_name"
    for (id, val) in &plugins_config {
        if seen_config_ids.contains(id) {
            continue;
        }
        if let Some((pname, mp_name)) = id.split_once('@') {
            let enabled = val.get("enabled").and_then(|v| v.as_bool()).unwrap_or(true);
            plugin_enabled_map
                .entry(pname.to_string())
                .or_default()
                .push((mp_name.to_string(), enabled));
        }
    }

    let mut agents = Vec::new();
    // seen_tmp_aliases: marketplace-free keys from .tmp — used to block cache fallback
    let mut seen_tmp_aliases: HashSet<String> = HashSet::new();
    // seen_cache: marketplace-aware keys — cache-to-cache dedup (cross-version)
    let mut seen_cache: HashSet<String> = HashSet::new();
    let mut seen_canonical: HashSet<PathBuf> = HashSet::new();

    // Shorthand closure delegating to the extracted pure function
    let is_disabled = |plugin_name: &str, marketplace: Option<&str>| -> bool {
        is_codex_plugin_disabled(
            plugin_name,
            marketplace,
            &plugins_config,
            &plugin_enabled_map,
        )
    };

    // 1. Primary: ~/.codex/.tmp/plugins/plugins/*/agents/
    //    Inserts marketplace-free keys into seen_tmp_aliases so cache fallback is blocked.
    let tmp_root = codex_home.join(".tmp").join("plugins").join("plugins");
    if tmp_root.is_dir() {
        for entry in std::fs::read_dir(&tmp_root).into_iter().flatten().flatten() {
            if !entry.path().is_dir() {
                continue;
            }
            let plugin_name = entry.file_name().to_string_lossy().to_string();
            if is_disabled(&plugin_name, None) {
                continue;
            }

            let agents_dir = entry.path().join("agents");
            let source = format!("codex-plugin:{}", plugin_name);
            collect_tmp_codex_agents(
                &agents_dir,
                &source,
                &plugin_name,
                &mut seen_tmp_aliases,
                &mut seen_canonical,
                &mut agents,
            );
        }
    }

    // 2. Fallback: ~/.codex/plugins/cache/{marketplace}/{plugin}/{version}/
    //    Checks seen_tmp_aliases first (marketplace-free) — if .tmp already has
    //    this plugin's agent, skip it. Uses marketplace-aware seen_cache for
    //    cache-to-cache dedup so same-name plugins from different marketplaces coexist.
    let cache_root = codex_home.join("plugins").join("cache");
    if cache_root.is_dir() {
        for mp in std::fs::read_dir(&cache_root)
            .into_iter()
            .flatten()
            .flatten()
        {
            if !mp.path().is_dir() {
                continue;
            }
            let mp_name = mp.file_name().to_string_lossy().to_string();
            for plugin in std::fs::read_dir(mp.path()).into_iter().flatten().flatten() {
                if !plugin.path().is_dir() {
                    continue;
                }
                let plugin_name = plugin.file_name().to_string_lossy().to_string();
                if is_disabled(&plugin_name, Some(&mp_name)) {
                    continue;
                }

                // Select active version — matches Plugins page logic:
                // "local" wins if present, otherwise lexicographically last.
                let version_names: Vec<String> = std::fs::read_dir(plugin.path())
                    .into_iter()
                    .flatten()
                    .flatten()
                    .filter(|e| e.path().is_dir())
                    .map(|e| e.file_name().to_string_lossy().to_string())
                    .collect();
                if version_names.is_empty() {
                    continue;
                }
                let active = if version_names.iter().any(|v| v == "local") {
                    "local".to_string()
                } else {
                    let mut sorted = version_names;
                    sorted.sort();
                    sorted.last().unwrap().clone()
                };
                let vp = plugin.path().join(&active);

                let source = format!("codex-plugin:{}:{}", mp_name, plugin_name);
                let cache_prefix = format!("{}:{}", mp_name, plugin_name);
                collect_cache_codex_agents(
                    &vp.join("agents"),
                    &source,
                    &plugin_name,
                    &seen_tmp_aliases,
                    &cache_prefix,
                    &mut seen_cache,
                    &mut seen_canonical,
                    &mut agents,
                );

                // skills/*/agents/
                let skills_dir = vp.join("skills");
                if skills_dir.is_dir() {
                    for skill in std::fs::read_dir(&skills_dir)
                        .into_iter()
                        .flatten()
                        .flatten()
                    {
                        if !skill.path().is_dir() {
                            continue;
                        }
                        let skill_name = skill.file_name().to_string_lossy().to_string();
                        let s = format!("codex-plugin:{}:{}:{}", mp_name, plugin_name, skill_name);
                        let tmp_plugin = format!("{}:{}", plugin_name, skill_name);
                        let cache_plugin = format!("{}:{}:{}", mp_name, plugin_name, skill_name);
                        collect_cache_codex_agents(
                            &skill.path().join("agents"),
                            &s,
                            &tmp_plugin,
                            &seen_tmp_aliases,
                            &cache_plugin,
                            &mut seen_cache,
                            &mut seen_canonical,
                            &mut agents,
                        );
                    }
                }
            }
        }
    }

    log::debug!(
        "[agents] discover_codex_plugin_agents: {} total",
        agents.len()
    );
    agents
}

// ── Tauri commands ──

/// List Codex plugin agents (pure filesystem scan, no CLI call).
#[tauri::command]
pub fn list_codex_agents() -> Result<Vec<AgentDefinitionSummary>, String> {
    log::debug!("[agents] list_codex_agents");
    Ok(discover_codex_plugin_agents())
}

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

    // ── collect_codex_agents dedup tests ──

    fn write_agent_md(dir: &Path, name: &str) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(
            dir.join(format!("{}.md", name)),
            format!(
                "---\nname: {}\ndescription: test agent\n---\nYou are {}.",
                name, name
            ),
        )
        .unwrap();
    }

    /// .tmp has vercel/deployment-expert.md, cache has openai-curated/vercel/.../deployment-expert.md
    /// → only 1 agent returned (.tmp wins)
    #[test]
    fn codex_dedup_tmp_blocks_cache() {
        let root = tempfile::tempdir().unwrap();
        let tmp_agents = root.path().join("tmp").join("vercel").join("agents");
        let cache_agents = root
            .path()
            .join("cache")
            .join("openai-curated")
            .join("vercel")
            .join("abc123")
            .join("agents");

        write_agent_md(&tmp_agents, "deployment-expert");
        write_agent_md(&cache_agents, "deployment-expert");

        let mut agents = Vec::new();
        let mut seen_tmp: HashSet<String> = HashSet::new();
        let mut seen_cache: HashSet<String> = HashSet::new();
        let mut seen_canonical: HashSet<PathBuf> = HashSet::new();

        // .tmp phase
        collect_tmp_codex_agents(
            &tmp_agents,
            "codex-plugin:vercel",
            "vercel",
            &mut seen_tmp,
            &mut seen_canonical,
            &mut agents,
        );
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].agent.as_deref(), Some("codex"));

        // cache phase — should be blocked by tmp alias
        collect_cache_codex_agents(
            &cache_agents,
            "codex-plugin:openai-curated:vercel",
            "vercel",
            &seen_tmp,
            "openai-curated:vercel",
            &mut seen_cache,
            &mut seen_canonical,
            &mut agents,
        );
        // Still 1 — cache copy was blocked
        assert_eq!(agents.len(), 1);
    }

    /// cache has mp-a/foo/agent.md and mp-b/foo/agent.md, .tmp has nothing
    /// → 2 agents returned (different marketplaces, not merged)
    #[test]
    fn codex_dedup_different_marketplaces_coexist() {
        let root = tempfile::tempdir().unwrap();
        let mp_a_agents = root
            .path()
            .join("cache")
            .join("mp-a")
            .join("foo")
            .join("v1")
            .join("agents");
        let mp_b_agents = root
            .path()
            .join("cache")
            .join("mp-b")
            .join("foo")
            .join("v1")
            .join("agents");

        write_agent_md(&mp_a_agents, "helper");
        write_agent_md(&mp_b_agents, "helper");

        let mut agents = Vec::new();
        let seen_tmp: HashSet<String> = HashSet::new(); // empty — no .tmp
        let mut seen_cache: HashSet<String> = HashSet::new();
        let mut seen_canonical: HashSet<PathBuf> = HashSet::new();

        // mp-a
        collect_cache_codex_agents(
            &mp_a_agents,
            "codex-plugin:mp-a:foo",
            "foo",
            &seen_tmp,
            "mp-a:foo",
            &mut seen_cache,
            &mut seen_canonical,
            &mut agents,
        );
        assert_eq!(agents.len(), 1);

        // mp-b — different marketplace-aware key, should NOT be blocked
        collect_cache_codex_agents(
            &mp_b_agents,
            "codex-plugin:mp-b:foo",
            "foo",
            &seen_tmp,
            "mp-b:foo",
            &mut seen_cache,
            &mut seen_canonical,
            &mut agents,
        );
        assert_eq!(agents.len(), 2);
    }

    /// Same marketplace, different plugins, same agent filename → 2 agents
    #[test]
    fn codex_dedup_same_marketplace_different_plugins() {
        let root = tempfile::tempdir().unwrap();
        let foo_agents = root
            .path()
            .join("cache")
            .join("mp-a")
            .join("foo")
            .join("v1")
            .join("agents");
        let bar_agents = root
            .path()
            .join("cache")
            .join("mp-a")
            .join("bar")
            .join("v1")
            .join("agents");

        write_agent_md(&foo_agents, "helper");
        write_agent_md(&bar_agents, "helper");

        let mut agents = Vec::new();
        let seen_tmp: HashSet<String> = HashSet::new();
        let mut seen_cache: HashSet<String> = HashSet::new();
        let mut seen_canonical: HashSet<PathBuf> = HashSet::new();

        // foo
        collect_cache_codex_agents(
            &foo_agents,
            "codex-plugin:mp-a:foo",
            "foo",
            &seen_tmp,
            "mp-a:foo",
            &mut seen_cache,
            &mut seen_canonical,
            &mut agents,
        );
        assert_eq!(agents.len(), 1);

        // bar — same marketplace, different plugin → should NOT be blocked
        collect_cache_codex_agents(
            &bar_agents,
            "codex-plugin:mp-a:bar",
            "bar",
            &seen_tmp,
            "mp-a:bar",
            &mut seen_cache,
            &mut seen_canonical,
            &mut agents,
        );
        assert_eq!(agents.len(), 2);
    }

    /// Test is_codex_plugin_disabled with marketplace (direct config lookup)
    #[test]
    fn codex_disabled_with_marketplace() {
        let mut config = serde_json::Map::new();
        config.insert(
            "vercel@openai-curated".to_string(),
            serde_json::json!({"enabled": false}),
        );
        let map: HashMap<String, Vec<(String, bool)>> = HashMap::new();

        // Direct marketplace lookup → disabled
        assert!(is_codex_plugin_disabled(
            "vercel",
            Some("openai-curated"),
            &config,
            &map
        ));
        // Enabled plugin
        assert!(!is_codex_plugin_disabled(
            "figma",
            Some("openai-curated"),
            &config,
            &map
        ));
        // No config entry → default enabled
        assert!(!is_codex_plugin_disabled(
            "vercel",
            Some("other-mp"),
            &config,
            &map
        ));
    }

    /// Test is_codex_plugin_disabled without marketplace (.tmp path, uses map)
    #[test]
    fn codex_disabled_tmp_via_map() {
        let config = serde_json::Map::new();

        // Single marketplace entry, disabled → .tmp should be blocked
        let mut map: HashMap<String, Vec<(String, bool)>> = HashMap::new();
        map.entry("vercel".to_string())
            .or_default()
            .push(("openai-curated".to_string(), false));
        assert!(is_codex_plugin_disabled("vercel", None, &config, &map));

        // Single marketplace entry, enabled → .tmp should show
        let mut map2: HashMap<String, Vec<(String, bool)>> = HashMap::new();
        map2.entry("figma".to_string())
            .or_default()
            .push(("openai-curated".to_string(), true));
        assert!(!is_codex_plugin_disabled("figma", None, &config, &map2));

        // Ambiguous: multiple marketplaces → conservative show
        let mut map3: HashMap<String, Vec<(String, bool)>> = HashMap::new();
        map3.entry("foo".to_string())
            .or_default()
            .push(("mp-a".to_string(), false));
        map3.entry("foo".to_string())
            .or_default()
            .push(("mp-b".to_string(), true));
        assert!(!is_codex_plugin_disabled("foo", None, &config, &map3));

        // No match at all → default show
        assert!(!is_codex_plugin_disabled("unknown", None, &config, &map));
    }

    /// Config-only disabled (no cache dir): config.toml has vercel@openai-curated
    /// enabled=false, map is populated from config fallback → .tmp/vercel is hidden
    #[test]
    fn codex_disabled_plugin_from_config_only() {
        let mut config = serde_json::Map::new();
        config.insert(
            "vercel@openai-curated".to_string(),
            serde_json::json!({"enabled": false}),
        );

        // Simulate the config→map fallback path from discover_codex_plugin_agents:
        // config entry "vercel@openai-curated" with no cache dir → added to map
        let mut map: HashMap<String, Vec<(String, bool)>> = HashMap::new();
        for (id, val) in &config {
            if let Some((pname, mp_name)) = id.split_once('@') {
                let enabled = val.get("enabled").and_then(|v| v.as_bool()).unwrap_or(true);
                map.entry(pname.to_string())
                    .or_default()
                    .push((mp_name.to_string(), enabled));
            }
        }

        // .tmp path (no marketplace) → should be disabled via map
        assert!(is_codex_plugin_disabled("vercel", None, &config, &map));
        // Direct marketplace lookup also works
        assert!(is_codex_plugin_disabled(
            "vercel",
            Some("openai-curated"),
            &config,
            &map
        ));
        // Unknown plugin → not disabled
        assert!(!is_codex_plugin_disabled(
            "superpowers",
            None,
            &config,
            &map
        ));
    }
}
