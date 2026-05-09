//! Remote filesystem browsing over SSH.
//!
//! Lists directories on a configured RemoteHost via `ssh ... ls`. No new
//! dependencies — uses the system `ssh` binary like the rest of the SSH path.
//!
//! Known limitation: filenames containing `\n` will break line-based parsing.
//! Acceptable for a directory picker; if needed later, switch to NUL-terminated
//! output via `find -print0` (GNU only) or `ls --quoting-style=shell-escape`.

use crate::agent::ssh::{build_ssh_command, shell_escape_path};
use crate::models::{DirEntry, DirListing, RemoteHost};
use crate::process_ext::HideConsole;
use crate::storage;
use std::time::Duration;

const REMOTE_LS_TIMEOUT: Duration = Duration::from_secs(15);

fn resolve_host(name: &str) -> Result<RemoteHost, String> {
    let settings = storage::settings::get_user_settings();
    settings
        .remote_hosts
        .into_iter()
        .find(|h| h.name == name)
        .ok_or_else(|| format!("Remote host '{}' not found in settings", name))
}

/// Run a remote shell command via SSH and return (stdout, stderr) as trimmed strings.
async fn run_remote(host: &RemoteHost, remote_shell: &str) -> Result<(String, String), String> {
    let mut cmd = build_ssh_command(host, remote_shell);
    cmd.stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .hide_console()
        .kill_on_drop(true);

    let output = tokio::time::timeout(REMOTE_LS_TIMEOUT, cmd.output())
        .await
        .map_err(|_| "SSH command timed out (15s)".to_string())?
        .map_err(|e| format!("SSH spawn failed: {}", e))?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();

    if !output.status.success() {
        let msg = if stderr.is_empty() {
            format!("Remote command failed (exit {:?})", output.status.code())
        } else {
            stderr
        };
        return Err(msg);
    }

    Ok((stdout, stderr))
}

/// List a directory on a remote host.
///
/// Uses `cd PATH && pwd && ls -1ApL[A] -- .`:
/// - `cd` handles `~/...` tilde expansion on the remote shell
/// - `pwd` returns the canonicalized absolute path (logical, not -P)
/// - `-1` one entry per line; `-p` trailing `/` for directories; `-L` follow symlinks
/// - `-A` (when `show_hidden`) shows dotfiles but excludes `./..`
/// - `LC_ALL=C` inline (not env) for stable sort/messages
/// - `--` and `.` so filenames starting with `-` aren't parsed as flags
#[tauri::command]
pub async fn list_remote_directory(
    host_name: String,
    path: String,
    show_hidden: Option<bool>,
) -> Result<DirListing, String> {
    let host = resolve_host(&host_name)?;
    let show_hidden = show_hidden.unwrap_or(false);
    let path_input = if path.is_empty() {
        "~".to_string()
    } else {
        path
    };

    log::debug!(
        "[remote_fs] list_remote_directory: host={}, path={}, show_hidden={}",
        host_name,
        path_input,
        show_hidden
    );

    // POSIX ls flags: -1 one-per-line, -p trailing slash for dirs, -L follow symlinks,
    // -A show hidden files (excludes ./..). When hiding, omit -A entirely.
    let ls_flags = if show_hidden { "-1ApL" } else { "-1pL" };
    let remote_shell = format!(
        "cd {} && pwd && LC_ALL=C ls {} -- .",
        shell_escape_path(&path_input),
        ls_flags
    );

    let (stdout, _stderr) = run_remote(&host, &remote_shell).await?;
    let listing = parse_listing(&stdout)?;

    log::debug!(
        "[remote_fs] list_remote_directory: canonical={}, entries={}",
        listing.path,
        listing.entries.len()
    );

    Ok(listing)
}

/// Parse the `pwd && ls -1pL` stdout shape: first line is the canonical path,
/// remaining lines are entries with a trailing `/` marking directories.
/// Sort: directories first, then case-insensitive name ascending.
fn parse_listing(stdout: &str) -> Result<DirListing, String> {
    let mut lines = stdout.lines();
    let canonical = lines
        .next()
        .ok_or_else(|| "Empty response from remote shell".to_string())?
        .to_string();

    let mut entries: Vec<DirEntry> = Vec::new();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        let is_dir = line.ends_with('/');
        let name = if is_dir {
            line.trim_end_matches('/').to_string()
        } else {
            line.to_string()
        };
        if name.is_empty() {
            continue;
        }
        entries.push(DirEntry {
            name,
            is_dir,
            size: 0,
        });
    }

    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then(a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });

    Ok(DirListing {
        path: canonical,
        entries,
    })
}

/// Resolve the remote shell's home directory (`cd ~ && pwd`).
#[tauri::command]
pub async fn resolve_remote_home(host_name: String) -> Result<String, String> {
    let host = resolve_host(&host_name)?;
    log::debug!("[remote_fs] resolve_remote_home: host={}", host_name);

    let (stdout, _stderr) = run_remote(&host, "cd ~ && pwd").await?;
    let home = stdout.lines().next().unwrap_or("").trim().to_string();
    if home.is_empty() {
        return Err("Empty pwd response from remote shell".to_string());
    }
    Ok(home)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_listing_empty_stdout_errors() {
        assert!(parse_listing("").is_err());
    }

    #[test]
    fn parse_listing_canonical_only_no_entries() {
        let listing = parse_listing("/home/user\n").unwrap();
        assert_eq!(listing.path, "/home/user");
        assert!(listing.entries.is_empty());
    }

    #[test]
    fn parse_listing_separates_dirs_from_files() {
        let stdout = "/home/user\nDocuments/\nReadme.md\nProjects/\nnotes.txt\n";
        let listing = parse_listing(stdout).unwrap();
        assert_eq!(listing.path, "/home/user");
        // Directories first (case-insensitive sort), then files (case-insensitive).
        let names: Vec<_> = listing
            .entries
            .iter()
            .map(|e| (e.name.as_str(), e.is_dir))
            .collect();
        assert_eq!(
            names,
            vec![
                ("Documents", true),
                ("Projects", true),
                ("notes.txt", false),
                ("Readme.md", false),
            ]
        );
    }

    #[test]
    fn parse_listing_skips_blank_and_lone_slash_lines() {
        // Blank lines from trailing newlines, and a stray "/" entry (e.g. `pwd` of /
        // followed by `ls /` returns nothing useful) are dropped, not surfaced as
        // empty-named entries.
        let stdout = "/\n\nbin/\n\netc/\n/\n";
        let listing = parse_listing(stdout).unwrap();
        assert_eq!(listing.path, "/");
        assert_eq!(listing.entries.len(), 2);
        assert_eq!(listing.entries[0].name, "bin");
        assert_eq!(listing.entries[1].name, "etc");
    }

    #[test]
    fn parse_listing_dotfiles_preserved_when_present() {
        // `ls -A` includes dotfiles; parser should accept them as-is (no filtering here).
        let stdout = "/home/user\n.config/\n.bashrc\nbin/\n";
        let listing = parse_listing(stdout).unwrap();
        let names: Vec<_> = listing.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec![".config", "bin", ".bashrc"]);
    }

    #[test]
    fn parse_listing_case_insensitive_sort_stable_within_group() {
        let stdout = "/x\nzeta/\nAlpha/\nbeta/\nGamma\nApple\n";
        let listing = parse_listing(stdout).unwrap();
        let names: Vec<_> = listing.entries.iter().map(|e| e.name.as_str()).collect();
        // Dirs (case-insensitive): Alpha, beta, zeta — then files: Apple, Gamma.
        assert_eq!(names, vec!["Alpha", "beta", "zeta", "Apple", "Gamma"]);
    }
}
