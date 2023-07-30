use {
    crate::{project::ProjectData, registry::error::RegistryError},
    async_trait::async_trait,
    reqwest::{
        header::{self, HeaderValue},
        StatusCode,
    },
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
    async fn project_data(&self, project_id: &str) -> RegistryResult<Option<ProjectData>> {
        if !is_valid_project_id(project_id) {
            return Ok(None);
        }

        let url = format!("{}/internal/project/key/{project_id}", self.base_url);
        let resp = self.http_client.get(url).send().await?;

        parse_http_response(resp).await
    }
}

/// Checks if the project ID is formatted properly. It must be 32 hex
/// characters.
fn is_valid_project_id(project_id: &str) -> bool {
    project_id.len() == 32 && is_hex_string(project_id)
}

fn is_hex_string(string: &str) -> bool {
    string.chars().all(|c| c.is_ascii_hexdigit())
}

async fn parse_http_response(resp: reqwest::Response) -> RegistryResult<Option<ProjectData>> {
    match resp.status() {
        code if code.is_success() => Ok(Some(resp.json().await?)),
        StatusCode::FORBIDDEN => Err(RegistryError::Config(INVALID_TOKEN_ERROR)),
        StatusCode::NOT_FOUND => Ok(None),
        _ => Err(RegistryError::Response(format!(
            "status={} body={:?}",
            resp.status(),
            resp.text().await
        ))),
    }
}
