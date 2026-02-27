use crate::models::{
    CommunitySkillDetail, CommunitySkillResult, PluginOperationResult, ProviderHealth,
};
use reqwest::Client;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::LazyLock;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

// ── Constants ──

const ALLOWED_HOSTS: &[&str] = &["skills.sh", "api.skyll.app", "raw.githubusercontent.com"];

const CACHE_TTL: Duration = Duration::from_secs(120);
const MAX_CACHE_ENTRIES: usize = 200;
const HEALTH_TTL: Duration = Duration::from_secs(300);
const MAX_CONTENT_SIZE: u64 = 1_048_576; // 1MB

// ── HTTP client (reuse across requests) ──

static CLIENT: LazyLock<Client> = LazyLock::new(|| {
    Client::builder()
        .timeout(Duration::from_secs(15))
        .connect_timeout(Duration::from_secs(10))
        .user_agent("OpenCovibe/0.1")
        .pool_idle_timeout(Duration::from_secs(90))
        .pool_max_idle_per_host(2)
        // Use system proxy if configured (skills.sh may require proxy in some networks)
        .build()
        .unwrap_or_default()
});

const MAX_RETRIES: u32 = 3;
const RETRY_BASE_MS: u64 = 500;

/// Retry a GET request with exponential backoff.
async fn get_with_retry(
    url: &str,
    query: Option<&[(&str, &str)]>,
) -> Result<reqwest::Response, String> {
    let mut last_err = String::new();
    for attempt in 0..MAX_RETRIES {
        if attempt > 0 {
            let delay = RETRY_BASE_MS * (1 << (attempt - 1)); // 500ms, 1000ms
            log::debug!(
                "[community] retry {}/{} after {}ms",
                attempt + 1,
                MAX_RETRIES,
                delay
            );
            tokio::time::sleep(Duration::from_millis(delay)).await;
        }

        let mut req = CLIENT.get(url);
        if let Some(q) = query {
            req = req.query(q);
        }

        match req.send().await {
            Ok(resp) => return Ok(resp),
            Err(e) => {
                last_err = format!("{e}");
                log::debug!(
                    "[community] request failed (attempt {}/{}): {}",
                    attempt + 1,
                    MAX_RETRIES,
                    e
                );
            }
        }
    }
    Err(format!(
        "Request failed after {MAX_RETRIES} attempts: {last_err}"
    ))
}

// ── Search cache: query → (timestamp, results) ──

type SearchCache = HashMap<String, (Instant, Vec<CommunitySkillResult>)>;
static SEARCH_CACHE: LazyLock<Mutex<SearchCache>> = LazyLock::new(|| Mutex::new(HashMap::new()));

// ── Health cache ──

static HEALTH_CACHE: LazyLock<Mutex<Option<(Instant, ProviderHealth)>>> =
    LazyLock::new(|| Mutex::new(None));

// ── Install mutex (serialize install/remove) ──

static INSTALL_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

// ── Intermediate deserialization structs ──

#[derive(Deserialize)]
struct SkillsShResponse {
    skills: Vec<SkillsShItem>,
    #[serde(default)]
    #[allow(dead_code)]
    count: u32,
}

#[derive(Deserialize)]
struct SkillsShItem {
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default, rename = "skillId")]
    skill_id: String,
    #[serde(default)]
    installs: u64,
    #[serde(default)]
    source: String,
}

#[derive(Deserialize)]
struct SkyllResponse {
    id: String,
    #[serde(default)]
    #[allow(dead_code)]
    title: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    source: String,
    #[serde(default)]
    install_count: u64,
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    refs: Option<SkyllRefs>,
}

#[derive(Deserialize, Default)]
struct SkyllRefs {
    #[serde(default)]
    raw: Option<String>,
    #[serde(default)]
    skills_sh: Option<String>,
    #[serde(default)]
    github: Option<String>,
}

// ── Validators ──

pub fn validate_query(q: &str) -> Result<(), String> {
    let trimmed = q.trim();
    if trimmed.len() < 2 {
        return Err("Query must be at least 2 characters".into());
    }
    if trimmed.len() > 200 {
        return Err("Query too long (max 200 characters)".into());
    }
    if trimmed.chars().any(|c| c.is_control()) {
        return Err("Query contains invalid characters".into());
    }
    Ok(())
}

pub fn validate_skill_id(id: &str) -> Result<(), String> {
    if id.is_empty() {
        return Err("Skill ID cannot be empty".into());
    }
    if id.len() > 256 {
        return Err("Skill ID too long (max 256 characters)".into());
    }
    if !id
        .chars()
        .all(|c| c.is_alphanumeric() || "-_:./".contains(c))
    {
        return Err("Skill ID contains invalid characters".into());
    }
    Ok(())
}

fn validate_url(url_str: &str) -> Result<(), String> {
    let parsed = url::Url::parse(url_str).map_err(|e| format!("Invalid URL: {e}"))?;
    if parsed.scheme() != "https" {
        return Err("Only HTTPS URLs allowed".into());
    }
    let host = parsed.host_str().unwrap_or("");
    if !ALLOWED_HOSTS.contains(&host) {
        return Err(format!("Host not allowed: {host}"));
    }
    Ok(())
}

fn validate_scope(scope: &str) -> Result<(), String> {
    match scope {
        "user" | "project" => Ok(()),
        _ => Err(format!(
            "Invalid scope: {scope}. Must be \"user\" or \"project\""
        )),
    }
}

/// Convert remote skill_id to safe local directory name.
/// Strips path components, keeps only alphanumeric + hyphen + underscore.
fn to_local_slug(skill_id: &str) -> Result<String, String> {
    let base = skill_id.rsplit('/').next().unwrap_or(skill_id);
    let slug: String = base
        .chars()
        .map(|c| if c == ':' { '-' } else { c })
        .filter(|c| c.is_alphanumeric() || *c == '-' || *c == '_')
        .collect();
    if slug.is_empty() {
        return Err("Invalid skill id".into());
    }
    Ok(slug)
}

// ── Public API ──

pub async fn health_check() -> ProviderHealth {
    // Check cache first
    {
        let cache = HEALTH_CACHE.lock().await;
        if let Some((ts, ref health)) = *cache {
            if ts.elapsed() < HEALTH_TTL {
                log::debug!(
                    "[community] health_check: cached result={}",
                    health.available
                );
                return health.clone();
            }
        }
    }

    log::debug!("[community] health_check: fetching from skills.sh");
    let result = get_with_retry("https://skills.sh/api/search?q=test&limit=1", None).await;

    let health = match result {
        Ok(resp) if resp.status().is_success() => ProviderHealth {
            available: true,
            reason: None,
        },
        Ok(resp) => ProviderHealth {
            available: false,
            reason: Some(format!("HTTP {}", resp.status())),
        },
        Err(e) => ProviderHealth {
            available: false,
            reason: Some(e),
        },
    };

    log::debug!(
        "[community] health_check: available={}, reason={:?}",
        health.available,
        health.reason
    );

    // Update cache
    let mut cache = HEALTH_CACHE.lock().await;
    *cache = Some((Instant::now(), health.clone()));

    health
}

pub async fn search(query: &str, limit: u32) -> Result<Vec<CommunitySkillResult>, String> {
    let cache_key = format!("{}:{}", query.to_lowercase(), limit);

    // Check cache
    {
        let cache = SEARCH_CACHE.lock().await;
        if let Some((ts, ref results)) = cache.get(&cache_key) {
            if ts.elapsed() < CACHE_TTL {
                log::debug!(
                    "[community] search: cache hit for '{}', {} results",
                    query,
                    results.len()
                );
                return Ok(results.clone());
            }
        }
    }

    log::debug!("[community] search: query='{}', limit={}", query, limit);

    let limit_str = limit.to_string();
    let resp = get_with_retry(
        "https://skills.sh/api/search",
        Some(&[("q", query), ("limit", limit_str.as_str())]),
    )
    .await
    .map_err(|e| format!("Search request failed: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("Search API returned HTTP {}", resp.status()));
    }

    let body: SkillsShResponse = resp
        .json()
        .await
        .map_err(|e| format!("Failed to parse search results: {e}"))?;

    let results: Vec<CommunitySkillResult> = body
        .skills
        .into_iter()
        .map(|item| {
            let display_name = if item.name.is_empty() {
                item.skill_id
            } else {
                item.name
            };
            CommunitySkillResult {
                id: item.id,
                name: display_name,
                installs: item.installs,
                source: item.source,
            }
        })
        .collect();

    log::debug!(
        "[community] search: '{}' returned {} results",
        query,
        results.len()
    );

    // Store in cache (with eviction)
    {
        let mut cache = SEARCH_CACHE.lock().await;
        if cache.len() >= MAX_CACHE_ENTRIES {
            let now = Instant::now();
            cache.retain(|_, (ts, _)| now.duration_since(*ts) < CACHE_TTL);
            if cache.len() >= MAX_CACHE_ENTRIES {
                cache.clear();
            }
        }
        cache.insert(cache_key, (Instant::now(), results.clone()));
    }

    Ok(results)
}

pub async fn get_detail(source: &str, skill_id: &str) -> Result<CommunitySkillDetail, String> {
    let skyll_url = format!("https://api.skyll.app/skills/{}/{}", source, skill_id);
    validate_url(&skyll_url)?;

    log::debug!(
        "[community] get_detail: source={}, skill_id={}, url={}",
        source,
        skill_id,
        skyll_url
    );

    match get_with_retry(&skyll_url, None).await {
        Ok(resp) if resp.status().is_success() => {
            let body: SkyllResponse = resp
                .json()
                .await
                .map_err(|e| format!("Failed to parse detail: {e}"))?;

            let refs = body.refs.unwrap_or_default();

            // Only use raw_url if it passes URL validation
            let raw_url = refs.raw.filter(|u| validate_url(u).is_ok());

            log::debug!(
                "[community] get_detail: skyll success, content_len={}, raw_url={:?}",
                body.content.as_ref().map(|c| c.len()).unwrap_or(0),
                raw_url
            );

            Ok(CommunitySkillDetail {
                id: body.id,
                name: skill_id.to_string(),
                description: body.description,
                installs: body.install_count,
                source: body.source,
                content: body.content,
                raw_url,
                skills_sh_url: refs.skills_sh,
                github_url: refs.github,
            })
        }
        Ok(resp) => {
            log::debug!(
                "[community] get_detail: skyll returned HTTP {}, falling back to raw GitHub",
                resp.status()
            );
            fallback_detail(source, skill_id).await
        }
        Err(e) => {
            log::debug!(
                "[community] get_detail: skyll request failed: {e}, falling back to raw GitHub"
            );
            fallback_detail(source, skill_id).await
        }
    }
}

/// Fallback: construct raw.githubusercontent.com URL and download SKILL.md directly.
async fn fallback_detail(source: &str, skill_id: &str) -> Result<CommunitySkillDetail, String> {
    let raw_url = format!(
        "https://raw.githubusercontent.com/{}/main/skills/{}/SKILL.md",
        source, skill_id
    );
    validate_url(&raw_url)?;

    log::debug!("[community] fallback_detail: downloading {}", raw_url);

    let resp = get_with_retry(&raw_url, None)
        .await
        .map_err(|e| format!("Failed to download SKILL.md: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!(
            "Could not fetch skill detail (HTTP {})",
            resp.status()
        ));
    }

    // Check content length
    if let Some(len) = resp.content_length() {
        if len > MAX_CONTENT_SIZE {
            return Err(format!("SKILL.md too large ({len} bytes, max 1MB)"));
        }
    }

    let content = resp
        .text()
        .await
        .map_err(|e| format!("Failed to read SKILL.md: {e}"))?;

    if content.len() as u64 > MAX_CONTENT_SIZE {
        return Err("SKILL.md too large (max 1MB)".into());
    }

    log::debug!(
        "[community] fallback_detail: downloaded {} bytes",
        content.len()
    );

    Ok(CommunitySkillDetail {
        id: format!("{}/{}", source, skill_id),
        name: skill_id.to_string(),
        description: String::new(),
        installs: 0,
        source: source.to_string(),
        content: Some(content),
        raw_url: Some(raw_url),
        skills_sh_url: None,
        github_url: Some(format!("https://github.com/{}", source)),
    })
}

pub async fn install_skill(
    source: &str,
    skill_id: &str,
    scope: &str,
    cwd: Option<&str>,
) -> Result<PluginOperationResult, String> {
    validate_scope(scope)?;
    let slug = to_local_slug(skill_id)?;

    log::debug!(
        "[community] install_skill: source={}, skill_id={}, scope={}, slug={}",
        source,
        skill_id,
        scope,
        slug
    );

    // Serialize installs
    let _lock = INSTALL_LOCK.lock().await;

    // Get detail (which includes content)
    let detail = get_detail(source, skill_id).await?;

    let mut content = detail.content.unwrap_or_default();

    // If content is empty, try downloading from raw_url
    if content.is_empty() {
        if let Some(ref raw_url) = detail.raw_url {
            validate_url(raw_url)?;
            log::debug!("[community] install_skill: content empty, downloading from raw_url");

            let resp = get_with_retry(raw_url, None)
                .await
                .map_err(|e| format!("Failed to download SKILL.md: {e}"))?;

            if !resp.status().is_success() {
                return Err(format!(
                    "Failed to download SKILL.md (HTTP {})",
                    resp.status()
                ));
            }

            if let Some(len) = resp.content_length() {
                if len > MAX_CONTENT_SIZE {
                    return Err(format!("SKILL.md too large ({len} bytes, max 1MB)"));
                }
            }

            content = resp
                .text()
                .await
                .map_err(|e| format!("Failed to read SKILL.md: {e}"))?;

            if content.len() as u64 > MAX_CONTENT_SIZE {
                return Err("SKILL.md too large (max 1MB)".into());
            }
        }
    }

    if content.is_empty() {
        return Err("No SKILL.md content available for this skill".into());
    }

    // Ensure YAML frontmatter exists (Skills tab parses name/description from it).
    // Community SKILL.md files often lack frontmatter — prepend it using API metadata.
    if !content.starts_with("---") {
        let desc_escaped = detail.description.replace('"', r#"\""#);
        let slug_escaped = slug.replace('"', r#"\""#);
        content = format!(
            "---\nname: \"{}\"\ndescription: \"{}\"\n---\n\n{}",
            slug_escaped, desc_escaped, content
        );
        log::debug!(
            "[community] install_skill: prepended frontmatter for '{}'",
            skill_id
        );
    }

    // Determine target path
    let target_dir = match scope {
        "user" => {
            let home = crate::storage::dirs_next()
                .ok_or_else(|| "Could not determine home directory".to_string())?;
            home.join(".claude").join("skills").join(&slug)
        }
        "project" => {
            let cwd_str = cwd.ok_or("Project scope requires a working directory (cwd)")?;
            if cwd_str.is_empty() {
                return Err("Project scope requires a non-empty working directory".into());
            }
            std::path::PathBuf::from(cwd_str)
                .join(".claude")
                .join("skills")
                .join(&slug)
        }
        _ => return Err(format!("Invalid scope: {scope}")),
    };

    let target_file = target_dir.join("SKILL.md");

    // Check if already installed
    if target_file.exists() {
        return Ok(PluginOperationResult {
            success: false,
            message: format!("Skill already installed at {}", target_file.display()),
        });
    }

    log::debug!(
        "[community] install_skill: writing {} bytes to {}",
        content.len(),
        target_file.display()
    );

    std::fs::create_dir_all(&target_dir).map_err(|e| format!("Failed to create directory: {e}"))?;
    std::fs::write(&target_file, &content).map_err(|e| format!("Failed to write SKILL.md: {e}"))?;

    log::debug!(
        "[community] install_skill: installed {} to {}",
        skill_id,
        target_file.display()
    );

    Ok(PluginOperationResult {
        success: true,
        message: format!("Installed {} to {}", skill_id, target_file.display()),
    })
}

// ── Tests ──

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_to_local_slug_simple() {
        assert_eq!(
            to_local_slug("react-best-practices").unwrap(),
            "react-best-practices"
        );
    }

    #[test]
    fn test_to_local_slug_with_path() {
        assert_eq!(
            to_local_slug("vercel-labs/agent-skills/react-best-practices").unwrap(),
            "react-best-practices"
        );
    }

    #[test]
    fn test_to_local_slug_with_colon() {
        assert_eq!(
            to_local_slug("react:components").unwrap(),
            "react-components"
        );
    }

    #[test]
    fn test_to_local_slug_with_path_and_colon() {
        assert_eq!(
            to_local_slug("google-labs/stitch-skills/react:components").unwrap(),
            "react-components"
        );
    }

    #[test]
    fn test_to_local_slug_empty() {
        assert!(to_local_slug("").is_err());
    }

    #[test]
    fn test_to_local_slug_only_special_chars() {
        assert!(to_local_slug("///").is_err());
    }

    #[test]
    fn test_validate_query_normal() {
        assert!(validate_query("react").is_ok());
        assert!(validate_query("c++").is_ok());
        assert!(validate_query("node.js").is_ok());
    }

    #[test]
    fn test_validate_query_too_short() {
        assert!(validate_query("a").is_err());
    }

    #[test]
    fn test_validate_query_control_chars() {
        assert!(validate_query("test\x00").is_err());
    }

    #[test]
    fn test_validate_query_too_long() {
        let long = "a".repeat(201);
        assert!(validate_query(&long).is_err());
    }

    #[test]
    fn test_validate_skill_id_normal() {
        assert!(validate_skill_id("vercel-labs/agent-skills/react-best-practices").is_ok());
    }

    #[test]
    fn test_validate_skill_id_with_dots() {
        assert!(validate_skill_id("my-skill.v2").is_ok());
    }

    #[test]
    fn test_validate_skill_id_empty() {
        assert!(validate_skill_id("").is_err());
    }

    #[test]
    fn test_validate_skill_id_special() {
        assert!(validate_skill_id("test skill!").is_err()); // space and ! not allowed
    }

    #[test]
    fn test_validate_url_https_allowed() {
        assert!(validate_url("https://skills.sh/api/search").is_ok());
        assert!(validate_url("https://api.skyll.app/skills/foo/bar").is_ok());
        assert!(validate_url("https://raw.githubusercontent.com/owner/repo/main/file").is_ok());
    }

    #[test]
    fn test_validate_url_http_rejected() {
        assert!(validate_url("http://skills.sh/api/search").is_err());
    }

    #[test]
    fn test_validate_url_unknown_host() {
        assert!(validate_url("https://evil.com/api").is_err());
    }

    #[test]
    fn test_validate_scope() {
        assert!(validate_scope("user").is_ok());
        assert!(validate_scope("project").is_ok());
        assert!(validate_scope("global").is_err());
        assert!(validate_scope("").is_err());
    }

    #[test]
    fn test_skillssh_response_deserialization() {
        let json = r#"{
            "skills": [
                {
                    "id": "vercel-labs/agent-skills/vercel-react-best-practices",
                    "name": "vercel-react-best-practices",
                    "installs": 130847,
                    "source": "vercel-labs/agent-skills"
                },
                {
                    "id": "google-labs/stitch-skills/react:components",
                    "skillId": "react-components",
                    "name": "react-components",
                    "installs": 5000,
                    "source": "google-labs/stitch-skills"
                }
            ],
            "count": 2,
            "searchType": "keyword"
        }"#;

        let resp: SkillsShResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.skills.len(), 2);
        assert_eq!(resp.skills[0].name, "vercel-react-best-practices");
        assert_eq!(resp.skills[0].installs, 130847);
        assert_eq!(resp.skills[1].name, "react-components");
    }

    #[test]
    fn test_skillssh_response_missing_fields() {
        let json = r#"{"skills": [{"id": "test", "name": "test"}]}"#;
        let resp: SkillsShResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.skills.len(), 1);
        assert_eq!(resp.skills[0].installs, 0); // default
        assert_eq!(resp.skills[0].source, ""); // default
        assert_eq!(resp.count, 0); // default
    }

    #[test]
    fn test_skyll_response_deserialization() {
        let json = r##"{
            "id": "vercel-react-best-practices",
            "title": "React Best Practices",
            "description": "Best practices for React development",
            "source": "vercel-labs/agent-skills",
            "install_count": 130847,
            "content": "# React Best Practices\n\nUse hooks.",
            "refs": {
                "raw": "https://raw.githubusercontent.com/vercel-labs/agent-skills/main/skills/vercel-react-best-practices/SKILL.md",
                "skills_sh": "https://skills.sh/skills/vercel-react-best-practices",
                "github": "https://github.com/vercel-labs/agent-skills"
            }
        }"##;

        let resp: SkyllResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.id, "vercel-react-best-practices");
        assert_eq!(resp.install_count, 130847);
        assert!(resp.content.is_some());
        assert!(resp.refs.is_some());
        let refs = resp.refs.unwrap();
        assert!(refs.raw.is_some());
    }

    #[test]
    fn test_skyll_response_missing_fields() {
        let json = r#"{"id": "test"}"#;
        let resp: SkyllResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.description, "");
        assert_eq!(resp.install_count, 0);
        assert!(resp.content.is_none());
        assert!(resp.refs.is_none());
    }

    #[test]
    fn test_path_construction_user_scope() {
        let slug = to_local_slug("vercel-react-best-practices").unwrap();
        let path = std::path::PathBuf::from("/home/user")
            .join(".claude")
            .join("skills")
            .join(&slug)
            .join("SKILL.md");
        assert_eq!(
            path.to_str().unwrap(),
            "/home/user/.claude/skills/vercel-react-best-practices/SKILL.md"
        );
    }

    #[test]
    fn test_path_construction_project_scope() {
        let slug = to_local_slug("react:components").unwrap();
        let path = std::path::PathBuf::from("/project")
            .join(".claude")
            .join("skills")
            .join(&slug)
            .join("SKILL.md");
        assert_eq!(
            path.to_str().unwrap(),
            "/project/.claude/skills/react-components/SKILL.md"
        );
    }

    #[test]
    fn test_slug_sanitizes_traversal() {
        // "../../../etc/passwd" → last segment "passwd"
        let slug = to_local_slug("../../../etc/passwd").unwrap();
        assert_eq!(slug, "passwd");
    }
}
