//! Prompt-level favorites — independent of RunMeta.
//!
//! Storage: `~/.opencovibe/prompt-favorites.json`
//! Each favorite stores the prompt text redundantly so it survives run deletion.

use crate::models::PromptFavorite;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

#[derive(Serialize, Deserialize)]
struct FavoritesFile {
    version: u32,
    items: Vec<PromptFavorite>,
}

fn favorites_path() -> std::path::PathBuf {
    super::data_dir().join("prompt-favorites.json")
}

/// Atomically write JSON to `path` (write .tmp → set 0o600 → rename).
fn write_atomic_json<T: Serialize>(path: &Path, data: &T) -> Result<(), String> {
    let tmp = path.with_extension("json.tmp");
    let json = serde_json::to_string_pretty(data).map_err(|e| e.to_string())?;
    fs::write(&tmp, &json).map_err(|e| format!("write tmp: {e}"))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600));
    }

    fs::rename(&tmp, path).map_err(|e| format!("rename: {e}"))?;
    Ok(())
}

fn load() -> FavoritesFile {
    let path = favorites_path();
    if !path.exists() {
        return FavoritesFile {
            version: 1,
            items: vec![],
        };
    }
    match fs::read_to_string(&path) {
        Ok(content) => serde_json::from_str(&content).unwrap_or(FavoritesFile {
            version: 1,
            items: vec![],
        }),
        Err(_) => FavoritesFile {
            version: 1,
            items: vec![],
        },
    }
}

fn save(file: &FavoritesFile) -> Result<(), String> {
    let path = favorites_path();
    super::ensure_dir(path.parent().unwrap()).map_err(|e| e.to_string())?;
    write_atomic_json(&path, file)
}

pub fn list_favorites() -> Vec<PromptFavorite> {
    log::debug!("[favorites] listing favorites");
    load().items
}

pub fn add_favorite(run_id: &str, seq: u64, text: &str) -> Result<PromptFavorite, String> {
    log::debug!(
        "[favorites] adding favorite: run_id={}, seq={}",
        run_id,
        seq
    );
    let mut file = load();

    // Check for duplicate
    if file
        .items
        .iter()
        .any(|f| f.run_id == run_id && f.seq == seq)
    {
        return Err("Already favorited".to_string());
    }

    let now = chrono::Utc::now().to_rfc3339();
    let fav = PromptFavorite {
        run_id: run_id.to_string(),
        seq,
        text: text.to_string(),
        tags: vec![],
        note: String::new(),
        created_at: now,
    };

    file.items.push(fav.clone());
    save(&file)?;
    log::debug!("[favorites] added favorite: run_id={}, seq={}", run_id, seq);
    Ok(fav)
}

pub fn remove_favorite(run_id: &str, seq: u64) -> Result<(), String> {
    log::debug!(
        "[favorites] removing favorite: run_id={}, seq={}",
        run_id,
        seq
    );
    let mut file = load();
    let before = file.items.len();
    file.items.retain(|f| !(f.run_id == run_id && f.seq == seq));
    if file.items.len() == before {
        return Err("Favorite not found".to_string());
    }
    save(&file)?;
    Ok(())
}

pub fn update_favorite_tags(run_id: &str, seq: u64, tags: Vec<String>) -> Result<(), String> {
    log::debug!(
        "[favorites] updating tags: run_id={}, seq={}, tags={:?}",
        run_id,
        seq,
        tags
    );
    let mut file = load();
    let fav = file
        .items
        .iter_mut()
        .find(|f| f.run_id == run_id && f.seq == seq)
        .ok_or("Favorite not found")?;
    fav.tags = tags;
    save(&file)
}

pub fn update_favorite_note(run_id: &str, seq: u64, note: &str) -> Result<(), String> {
    log::debug!("[favorites] updating note: run_id={}, seq={}", run_id, seq);
    let mut file = load();
    let fav = file
        .items
        .iter_mut()
        .find(|f| f.run_id == run_id && f.seq == seq)
        .ok_or("Favorite not found")?;
    fav.note = note.to_string();
    save(&file)
}

pub fn list_all_tags() -> Vec<String> {
    let file = load();
    let mut tags: Vec<String> = file
        .items
        .iter()
        .flat_map(|f| f.tags.iter().cloned())
        .collect();
    tags.sort();
    tags.dedup();
    log::debug!("[favorites] all tags: {:?}", tags);
    tags
}
