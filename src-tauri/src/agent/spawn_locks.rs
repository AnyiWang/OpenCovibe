use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

/// Per-run lifecycle serialization.
///
/// Protects actor lifecycle operations (start/stop/fork/approve) from
/// concurrent execution on the same run_id. Data operations (send_message,
/// send_control) go through the actor's channel and don't need this lock.
pub struct SpawnLocks {
    inner: Mutex<HashMap<String, Arc<Mutex<()>>>>,
}

impl Default for SpawnLocks {
    fn default() -> Self {
        Self::new()
    }
}

impl SpawnLocks {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
        }
    }

    /// Acquire a per-run lock. Returns an owned guard that releases on drop.
    /// Periodically cleans up entries for runs that are no longer referenced.
    pub async fn acquire(&self, run_id: &str) -> tokio::sync::OwnedMutexGuard<()> {
        let lock = {
            let mut map = self.inner.lock().await;

            // GC: remove entries where strong_count == 1 (only the map holds a ref).
            // Do this opportunistically on every acquire — cheap for typical map sizes.
            map.retain(|_, v| Arc::strong_count(v) > 1);

            map.entry(run_id.to_string())
                .or_insert_with(|| Arc::new(Mutex::new(())))
                .clone()
        };
        lock.lock_owned().await
    }
}
