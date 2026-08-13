pub mod artifacts;
pub mod changelog;
pub mod claude_usage;
pub mod cli_config;
pub mod cli_sessions;
pub mod cli_sessions_common;
pub mod codex_sessions;
pub mod codex_usage;
pub mod community_skills;
pub mod events;
pub mod favorites;
pub mod history;
pub mod mcp_registry;
pub mod plugins;
pub mod prompt_index;
pub mod run_index;
pub mod runs;
pub mod settings;
pub mod teams;

use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};

/// Held for the complete application lifetime so every file under `~/.opencovibe` has one writer.
/// Process-local mutexes in the event and history modules rely on this external invariant.
pub struct DataDirLock {
    _file: File,
}

impl DataDirLock {
    pub fn acquire() -> Result<Self, String> {
        let dir = data_dir();
        ensure_dir(&dir).map_err(|e| format!("create data directory: {e}"))?;
        Self::acquire_at(&dir.join(".writer.lock"))
    }

    fn acquire_at(path: &Path) -> Result<Self, String> {
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;

            let file = OpenOptions::new()
                .create(true)
                .truncate(false)
                .read(true)
                .write(true)
                .open(path)
                .map_err(|e| format!("open data writer lock: {e}"))?;
            let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
            if result != 0 {
                return Err(format!(
                    "another OpenCovibe instance is already using {}",
                    data_dir().display()
                ));
            }
            Ok(Self { _file: file })
        }

        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;

            // A zero share mode makes the open file itself the lifetime lock. Windows releases it
            // on crash, so a stale marker can never block the next application start.
            let file = OpenOptions::new()
                .create(true)
                .truncate(false)
                .read(true)
                .write(true)
                .share_mode(0)
                .open(path)
                .map_err(|_| {
                    format!(
                        "another OpenCovibe instance is already using {}",
                        data_dir().display()
                    )
                })?;
            Ok(Self { _file: file })
        }

        #[cfg(not(any(unix, windows)))]
        {
            let file = OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(path)
                .map_err(|e| format!("acquire data writer lock: {e}"))?;
            Ok(Self { _file: file })
        }
    }
}

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

/// Resolve the user's home directory reliably.
/// Primary: `getpwuid()` system call (works even when `$HOME` is unset,
/// e.g. GUI apps launched from Finder/Dock on macOS 26+).
/// Fallback: `$HOME` (Unix) or `$USERPROFILE` (Windows).
pub fn home_dir() -> Option<String> {
    #[cfg(unix)]
    {
        let pwd_home = unsafe {
            let uid = libc::getuid();
            let pw = libc::getpwuid(uid);
            if !pw.is_null() {
                let dir = (*pw).pw_dir;
                if !dir.is_null() {
                    Some(std::ffi::CStr::from_ptr(dir).to_string_lossy().into_owned())
                } else {
                    None
                }
            } else {
                None
            }
        };
        if pwd_home.is_some() {
            return pwd_home;
        }
        std::env::var("HOME").ok()
    }
    #[cfg(not(unix))]
    {
        std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .or_else(|_| {
                let drive = std::env::var("HOMEDRIVE").unwrap_or_default();
                let path = std::env::var("HOMEPATH").unwrap_or_default();
                if !drive.is_empty() && !path.is_empty() {
                    Ok(format!("{}{}", drive, path))
                } else {
                    Err(std::env::VarError::NotPresent)
                }
            })
            .ok()
    }
}

pub(crate) fn dirs_next() -> Option<PathBuf> {
    home_dir().map(PathBuf::from)
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

#[cfg(test)]
mod data_dir_lock_tests {
    use super::DataDirLock;

    #[test]
    fn data_dir_lock_is_exclusive_and_released_on_drop() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("writer.lock");
        let first = DataDirLock::acquire_at(&path).unwrap();
        assert!(DataDirLock::acquire_at(&path).is_err());
        drop(first);
        DataDirLock::acquire_at(&path).unwrap();
    }
}
