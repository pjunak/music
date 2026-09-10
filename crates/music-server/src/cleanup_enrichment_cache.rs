//! Bounded, process-local caches contain observations, never provider credentials.
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

#[derive(Debug)]
pub(super) struct ObservationCache<T> {
    entries: BTreeMap<String, (Instant, T)>,
}

impl<T> Default for ObservationCache<T> {
    fn default() -> Self {
        Self {
            entries: BTreeMap::new(),
        }
    }
}

impl<T: Clone> ObservationCache<T> {
    pub fn get(&self, key: &str) -> Option<T> {
        self.entries
            .get(key)
            .filter(|(time, _)| time.elapsed() < Duration::from_secs(3600))
            .map(|(_, value)| value.clone())
    }
    pub fn insert(&mut self, key: String, value: T) {
        if self.entries.len() >= 256
            && let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, (time, _))| *time)
                .map(|(key, _)| key.clone())
        {
            self.entries.remove(&oldest);
        }
        self.entries.insert(key, (Instant::now(), value));
    }
    pub fn clear(&mut self) {
        self.entries.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cache_is_bounded_expires_and_can_be_explicitly_refreshed() {
        let mut cache = ObservationCache::default();
        for i in 0..300 {
            cache.insert(i.to_string(), i);
        }
        assert_eq!(cache.entries.len(), 256);
        assert_eq!(cache.get("299"), Some(299));
        cache.entries.insert(
            "stale".into(),
            (Instant::now() - Duration::from_secs(3601), 42),
        );
        assert!(cache.get("stale").is_none());
        cache.clear();
        assert!(cache.entries.is_empty());
    }
}
