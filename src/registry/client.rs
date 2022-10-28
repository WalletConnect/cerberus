#[cfg(feature = "cache")]
use {
    crate::registry::cache,
    crate::registry::cache::CachedProject,
    common::storage::KeyValueStorage,
    std::sync::Arc,
    std::time::Duration,
};
#[cfg(feature = "metrics")]
use {crate::registry::metrics::ProjectDataMetrics, common::metrics, std::time::Instant};
use {
    crate::{project::ProjectData, registry::error::RegistryError},
    async_trait::async_trait,
    reqwest::header::{self, HeaderValue},
    std::fmt::Debug,
};

const INVALID_TOKEN_ERROR: &str = "invalid auth token";

// #############################################################################

pub type RegistryResult<T> = Result<T, RegistryError>;

#[async_trait]
pub trait RegistryClient: 'static + Send + Sync + Debug {
    async fn project_data(&self, id: &str) -> RegistryResult<Option<ProjectData>>;
}

// #############################################################################

#[derive(Debug, Clone)]
pub struct RegistryHttpClient {
    base_url: String,
    http_client: reqwest::Client,

    #[cfg(feature = "cache")]
    cache: Option<cache::ProjectStorage>,

    #[cfg(feature = "metrics")]
    metrics: Option<ProjectDataMetrics>,
}

impl RegistryHttpClient {
    pub fn new(base_url: impl Into<String>, auth_token: &str) -> RegistryResult<Self> {
        let mut auth_value = HeaderValue::from_str(&format!("Bearer {}", auth_token))
            .map_err(|_| RegistryError::Config(INVALID_TOKEN_ERROR))?;

        // Make sure we're not leaking auth token in debug output.
        auth_value.set_sensitive(true);

        let mut headers = header::HeaderMap::new();
        headers.insert(header::AUTHORIZATION, auth_value);

        let http_client = reqwest::Client::builder()
            .default_headers(headers)
            .build()?;

        Ok(Self {
            base_url: base_url.into(),
            http_client,
            #[cfg(feature = "cache")]
            cache: None,
            #[cfg(feature = "metrics")]
            metrics: None,
        })
    }

    #[cfg(feature = "cache")]
    pub fn cached(
        mut self,
        cache: Arc<dyn KeyValueStorage<CachedProject>>,
        cache_ttl: Duration,
    ) -> Self {
        self.cache = Some(cache::ProjectStorage {
            cache,
            cache_ttl,
            #[cfg(feature = "metrics")]
            metrics: self.metrics.clone(),
        });

        self
    }

    #[cfg(feature = "metrics")]
    pub fn with_metrics(mut self, metrics: &metrics::AppMetrics) -> Self {
        self.metrics = Some(ProjectDataMetrics::new(metrics));

        #[cfg(feature = "cache")]
        if let Some(cache) = &mut self.cache {
            let mut cache = cache.clone();
            self.cache = Some(cache.with_metrics(metrics).to_owned());
        }

        self
    }
}

#[async_trait]
impl RegistryClient for RegistryHttpClient {
    async fn project_data(&self, id: &str) -> RegistryResult<Option<ProjectData>> {
        #[cfg(feature = "cache")]
        if let Some(cache) = &self.cache {
            #[cfg(feature = "metrics")]
            let time = Instant::now();

            let data = cache.fetch(id).await?;

            #[cfg(feature = "metrics")]
            if let Some(metrics) = self.metrics.as_ref() {
                metrics.cache_time(time.elapsed())
            }

            if let Some(data) = data {
                return match data {
                    CachedProject::Found(data) => Ok(Some(data)),
                    CachedProject::NotFound => Ok(None),
                    CachedProject::RegistryConfigError => Err(RegistryError::Cached),
                };
            }
        }

        #[cfg(feature = "metrics")]
        let time = Instant::now();

        let resp = self
            .http_client
            .get(format!("{}/internal/project/key/{id}", self.base_url))
            .send()
            .await?;

        let data = parse_http_response(resp).await;

        #[cfg(feature = "metrics")]
        if let Some(metrics) = self.metrics.as_ref() {
            metrics.registry_time(time.elapsed());
        }

        #[cfg(feature = "cache")]
        if let Some(cache) = &self.cache {
            // Cache all responses that we get, even errors.
            let cache_data = match &data {
                Ok(Some(data)) => CachedProject::Found(data.to_owned()),
                Ok(None) => CachedProject::NotFound,
                Err(RegistryError::Config(..)) => CachedProject::RegistryConfigError,

                // This is a retryable error, don't cache the result.
                _ => return data,
            };

            let _ = cache.set(id, cache_data);
        }

        #[allow(clippy::let_and_return)]
        data
    }
}

async fn parse_http_response(resp: reqwest::Response) -> RegistryResult<Option<ProjectData>> {
    match resp.status().as_u16() {
        200..=299 => Ok(Some(resp.json().await?)),
        403 => Err(RegistryError::Config(INVALID_TOKEN_ERROR)),
        404 => Ok(None),
        _ => Err(RegistryError::Response(resp.status().to_string())),
    }
}
