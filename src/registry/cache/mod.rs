pub use error::*;
use {
    crate::project::ProjectData,
    common::storage::{KeyValueStorage, StorageResult},
    serde::{Deserialize, Serialize},
    std::{sync::Arc, time::Duration},
};
#[cfg(feature = "metrics")]
use {crate::registry::metrics::ProjectDataMetrics, common::metrics, std::time::Instant};

mod error;

#[derive(Clone, Debug)]
pub(crate) struct ProjectStorage {
    pub(crate) cache: Arc<dyn KeyValueStorage<CachedProject>>,
    pub(crate) cache_ttl: Duration,
    #[cfg(feature = "metrics")]
    pub(crate) metrics: Option<ProjectDataMetrics>,
}

impl ProjectStorage {
    #[cfg(feature = "metrics")]
    pub fn with_metrics(&mut self, metrics: &metrics::AppMetrics) -> &Self {
        self.metrics = Some(ProjectDataMetrics::new(metrics));
        self
    }

    pub async fn fetch(&self, id: &str) -> StorageResult<Option<CachedProject>> {
        #[cfg(feature = "metrics")]
        let time = Instant::now();

        let cache_key = build_cache_key(id);
        let data = self.cache.get(&cache_key).await?;
        // .tap_err(|err| warn!(?err, "error fetching data from project data cache"))?;

        #[cfg(feature = "metrics")]
        {
            if let Some(metrics) = self.metrics.as_ref() {
                metrics.cache_time(time.elapsed());
            }
        }

        #[allow(clippy::let_and_return)]
        Ok(data)
    }

    pub async fn set(&self, id: &str, data: CachedProject) {
        let cache_key = build_cache_key(id);

        let serialized = common::storage::serialize(&data).unwrap(); //?;
        let cache = self.cache.clone();
        let cache_ttl = self.cache_ttl;

        // Do not block on cache write.
        tokio::spawn(async move {
            cache
        .set_serialized(&cache_key, &serialized, Some(cache_ttl))
        .await
        // .tap_err(|err| warn!("failed to cache project data: {err:?}"))
        .ok();
        });
    }
}

fn build_cache_key(id: &str) -> String {
    format!("project-data/{id}")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CachedProject {
    Found(ProjectData),

    NotFound,
    RegistryConfigError,
}
