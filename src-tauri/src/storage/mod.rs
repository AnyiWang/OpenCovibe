pub mod artifacts;
pub mod changelog;
pub mod claude_usage;
pub mod cli_config;
pub mod cli_sessions;
pub mod community_skills;
pub mod events;
pub mod favorites;
pub mod mcp_registry;
pub mod plugins;
pub mod prompt_index;
pub mod runs;
pub mod settings;
pub mod teams;

use std::path::PathBuf;

pub fn data_dir() -> PathBuf {
    let home = dirs_next().expect("Could not determine home directory");
    home.join(".opencovibe")
}

pub fn runs_dir() -> PathBuf {
    data_dir().join("runs")
}

pub fn run_dir(run_id: &str) -> PathBuf {
    runs_dir().join(run_id)
}

pub(crate) fn dirs_next() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        std::env::var("HOME").ok().map(PathBuf::from)
    }
    #[cfg(not(target_os = "macos"))]
    {
        std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .ok()
            .map(PathBuf::from)
    }
}

pub fn ensure_dir(path: &std::path::Path) -> std::io::Result<()> {
    if !path.exists() {
        std::fs::create_dir_all(path)?;
    }

    // Restrict directory permissions — data dir may contain sensitive data
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700));
    }

    Ok(())
}
