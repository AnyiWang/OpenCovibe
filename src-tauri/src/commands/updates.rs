use reqwest::Client;
use std::sync::LazyLock;
use std::time::Duration;

// ── Constants ──

const GITHUB_API_URL: &str = "https://api.github.com/repos/AnyiWang/OpenCovibe/releases/latest";

// ── HTTP client (reuse across requests) ──

static CLIENT: LazyLock<Client> = LazyLock::new(|| {
    Client::builder()
        .timeout(Duration::from_secs(15))
        .connect_timeout(Duration::from_secs(10))
        .user_agent(format!("OpenCovibe/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .unwrap_or_default()
});

// ── Types ──

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    pub has_update: bool,
    pub latest_version: String,
    pub current_version: String,
    pub download_url: String,
}

// ── Version comparison ──

/// Compare two semver-like version strings. Returns true if `latest` is newer than `current`.
/// Strips leading 'v' prefix. Pre-release versions (e.g. "1.0.0-beta.1") are considered
/// older than the same version without pre-release suffix.
/// Returns false on any parse failure (safe degradation).
fn parse_version(s: &str) -> Option<([u64; 3], bool)> {
    let s = s.strip_prefix('v').unwrap_or(s);
    let (main, has_pre) = if let Some(idx) = s.find('-') {
        (&s[..idx], true)
    } else {
        (s, false)
    };
    let parts: Vec<&str> = main.split('.').collect();
    if parts.len() != 3 {
        return None;
    }
    let major = parts[0].parse::<u64>().ok()?;
    let minor = parts[1].parse::<u64>().ok()?;
    let patch = parts[2].parse::<u64>().ok()?;
    Some(([major, minor, patch], has_pre))
}

fn is_newer(current: &str, latest: &str) -> bool {
    let (cur_ver, cur_pre) = match parse_version(current) {
        Some(v) => v,
        None => return false,
    };
    let (lat_ver, lat_pre) = match parse_version(latest) {
        Some(v) => v,
        None => return false,
    };

    // Compare major.minor.patch
    if lat_ver > cur_ver {
        return true;
    }
    if lat_ver < cur_ver {
        return false;
    }

    // Same version number: pre-release < release
    // If latest has pre-release suffix, it's not newer than current release
    match (cur_pre, lat_pre) {
        (true, false) => true, // current is pre-release, latest is release → upgrade
        _ => false,            // equal release, or latest is pre-release → no upgrade
    }
}

// ── Tauri command ──

#[tauri::command]
pub async fn check_for_updates(app: tauri::AppHandle) -> Result<UpdateInfo, String> {
    let current_version = app.package_info().version.to_string();
    log::debug!(
        "[updates] checking for updates, current={}",
        current_version
    );

    let resp = match CLIENT
        .get(GITHUB_API_URL)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            log::warn!("[updates] network error (offline/timeout): {}", e);
            return Ok(UpdateInfo {
                has_update: false,
                latest_version: String::new(),
                current_version,
                download_url: String::new(),
            });
        }
    };

    let status = resp.status();
    if !status.is_success() {
        log::warn!("[updates] GitHub API returned HTTP {}", status);
        return Ok(UpdateInfo {
            has_update: false,
            latest_version: String::new(),
            current_version,
            download_url: String::new(),
        });
    }

    let body: serde_json::Value = match resp.json().await {
        Ok(v) => v,
        Err(e) => {
            log::warn!("[updates] failed to parse response: {}", e);
            return Ok(UpdateInfo {
                has_update: false,
                latest_version: String::new(),
                current_version,
                download_url: String::new(),
            });
        }
    };

    let tag = body["tag_name"].as_str().unwrap_or("");
    let html_url = body["html_url"].as_str().unwrap_or("");

    if tag.is_empty() {
        log::warn!("[updates] empty tag_name in response");
        return Ok(UpdateInfo {
            has_update: false,
            latest_version: String::new(),
            current_version,
            download_url: String::new(),
        });
    }

    let latest_version = tag.strip_prefix('v').unwrap_or(tag).to_string();
    let has_update = is_newer(&current_version, tag);

    log::debug!(
        "[updates] current={} latest={} has_update={}",
        current_version,
        latest_version,
        has_update
    );

    Ok(UpdateInfo {
        has_update,
        latest_version,
        current_version,
        download_url: html_url.to_string(),
    })
}

// ── Tests ──

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normal_upgrade() {
        assert!(is_newer("0.1.2", "v0.1.3"));
    }

    #[test]
    fn test_equal_versions() {
        assert!(!is_newer("0.1.3", "0.1.3"));
    }

    #[test]
    fn test_latest_is_prerelease() {
        assert!(!is_newer("0.1.3", "0.1.3-beta.1"));
    }

    #[test]
    fn test_current_is_prerelease() {
        assert!(is_newer("0.1.3-beta.1", "0.1.3"));
    }

    #[test]
    fn test_downgrade() {
        assert!(!is_newer("0.2.0", "0.1.9"));
    }

    #[test]
    fn test_v_prefix() {
        assert!(is_newer("v0.1.0", "v0.1.1"));
    }

    #[test]
    fn test_invalid_semver() {
        assert!(!is_newer("abc", "0.1.0"));
    }
}
