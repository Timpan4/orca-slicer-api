use serde::Serialize;
use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, SystemTime},
};
use tokio::sync::RwLock;

#[derive(Clone, Debug, Serialize)]
pub struct Progress {
    pub stage: String,
    pub total_percent: Option<f32>,
    pub plate_percent: Option<f32>,
    pub plate_index: Option<u32>,
    pub plate_count: Option<u32>,
    pub updated_at: u64,
}
#[derive(Clone)]
pub struct ProgressStore {
    inner: Arc<RwLock<HashMap<String, (Progress, SystemTime)>>>,
}
impl ProgressStore {
    pub fn new() -> Self {
        Self { inner: Arc::new(RwLock::new(HashMap::new())) }
    }
    pub async fn reserve(&self, id: &str, mut p: Progress) -> bool {
        let mut g = self.inner.write().await;
        Self::prune(&mut g);
        if g.contains_key(id) {
            return false;
        }
        p.updated_at =
            SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default().as_secs();
        g.insert(id.into(), (p, SystemTime::now()));
        true
    }
    pub async fn set(&self, id: &str, mut p: Progress) {
        p.updated_at =
            SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default().as_secs();
        let mut g = self.inner.write().await;
        Self::prune(&mut g);
        g.insert(id.into(), (p, SystemTime::now()));
    }
    pub async fn get(&self, id: &str) -> Option<Progress> {
        let mut g = self.inner.write().await;
        Self::prune(&mut g);
        g.get(id).map(|(progress, _)| progress.clone())
    }
    fn prune(entries: &mut HashMap<String, (Progress, SystemTime)>) {
        entries.retain(|_, (progress, updated)| {
            !matches!(progress.stage.as_str(), "completed" | "failed")
                || !updated.elapsed().is_ok_and(|age| age > Duration::from_secs(30))
        });
    }
}
impl Default for ProgressStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn progress(stage: &str) -> Progress {
        Progress {
            stage: stage.into(),
            total_percent: None,
            plate_percent: None,
            plate_index: None,
            plate_count: None,
            updated_at: 0,
        }
    }

    #[tokio::test]
    async fn reservations_are_atomic_and_prune_expired_terminal_entries() {
        let store = ProgressStore::new();
        assert!(store.reserve("shared", progress("running")).await);
        assert!(!store.reserve("shared", progress("running")).await);

        store.set("shared", progress("completed")).await;
        store.inner.write().await.insert(
            "expired".into(),
            (progress("failed"), SystemTime::now() - Duration::from_secs(31)),
        );
        assert!(!store.reserve("shared", progress("running")).await);
        assert!(!store.inner.read().await.contains_key("expired"));
        store.inner.write().await.get_mut("shared").unwrap().1 =
            SystemTime::now() - Duration::from_secs(31);

        assert!(store.reserve("shared", progress("running")).await);
        assert_eq!(store.get("shared").await.unwrap().stage, "running");
    }
}
