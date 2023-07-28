use {
    crate::{project::ProjectData, registry::error::RegistryError},
    async_trait::async_trait,
    reqwest::header::{self, HeaderValue},
    std::{fmt::Debug, time::Duration},
};

const INVALID_TOKEN_ERROR: &str = "invalid auth token";

pub type RegistryResult<T> = Result<T, RegistryError>;

#[async_trait]
pub trait RegistryClient: 'static + Send + Sync + Debug {
    async fn project_data(&self, id: &str) -> RegistryResult<Option<ProjectData>>;
}

/// HTTP client configuration.
#[derive(Debug, Clone)]
pub struct HttpClientConfig {
    /// Connection keep-alive timeout after being returned to the pool.
    ///
    /// `None` disables the timeout. Default is 90 seconds.
    pub pool_idle_timeout: Option<Duration>,

    /// Maximum number of idle connections to keep alive.
    ///
    /// Default is unlimited.
    pub pool_max_idle: usize,

    /// Enables a request timeout.
    ///
    /// The timeout is applied for both the connect phase of a `Client`, and for
    /// fully receiving response body.
    ///
    /// Default is no timeout.
    pub timeout: Option<Duration>,
}

impl Default for HttpClientConfig {
    fn default() -> Self {
        // These defaults are taken from `reqwest` default config.
        Self {
            pool_idle_timeout: Some(Duration::from_secs(90)),
            pool_max_idle: usize::MAX,
            timeout: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct RegistryHttpClient {
    base_url: String,
    http_client: reqwest::Client,
}

impl RegistryHttpClient {
    pub fn new(base_url: impl Into<String>, auth_token: &str) -> RegistryResult<Self> {
        Self::with_config(base_url, auth_token, Default::default())
    }

    pub fn with_config(
        base_url: impl Into<String>,
        auth_token: &str,
        config: HttpClientConfig,
    ) -> RegistryResult<Self> {
        let mut auth_value = HeaderValue::from_str(&format!("Bearer {}", auth_token))
            .map_err(|_| RegistryError::Config(INVALID_TOKEN_ERROR))?;

        // Make sure we're not leaking auth token in debug output.
        auth_value.set_sensitive(true);

        let mut headers = header::HeaderMap::new();
        headers.insert(header::AUTHORIZATION, auth_value);

        let mut http_client = reqwest::Client::builder()
            .default_headers(headers)
            .pool_idle_timeout(config.pool_idle_timeout)
            .pool_max_idle_per_host(config.pool_max_idle);

        if let Some(timeout) = config.timeout {
            http_client = http_client.connect_timeout(timeout).timeout(timeout);
        }

        Ok(Self {
            base_url: base_url.into(),
            http_client: http_client.build()?,
        })
    }
}

#[async_trait]
impl RegistryClient for RegistryHttpClient {
    async fn project_data(&self, id: &str) -> RegistryResult<Option<ProjectData>> {
        let resp = self
            .http_client
            .get(format!("{}/internal/project/key/{id}", self.base_url))
            .send()
            .await?;

        parse_http_response(resp).await
    }
}

async fn parse_http_response(resp: reqwest::Response) -> RegistryResult<Option<ProjectData>> {
    let status = resp.status();
    match status.as_u16() {
        200..=299 => Ok(Some(resp.json().await?)),
        403 => Err(RegistryError::Config(INVALID_TOKEN_ERROR)),
        404 => Ok(None),
        400 if resp
            .text()
            .await
            // https://github.com/WalletConnect/explorer-api/blob/316f07d4657ccba9ef8d67e2ab06b75a33d4e3ed/src/controllers/internal/getProjectData.ts#L29C27-L29C44
            .is_ok_and(|text| text.contains("Invalid projectId")) =>
        {
            Ok(None)
        }
        _ => Err(RegistryError::Response(status.to_string())),
    }
}
